//! Data-movement engine for IPC.
//!
//! Two paths depending on the message type:
//!
//! | Path               | Trigger              | Mechanism                   |
//! |--------------------|----------------------|-----------------------------|
//! | Register fast path | `MessageType::Short` | Copy words into `IpcBuffer` |
//! | Page remap path    | `MessageType::MemoryMap` | `bmm::grant()` — zero-copy |
//! | Capability path    | `MessageType::GrantCapability` | Stub (TODO: cap module) |
//!
//! # Future: true register fast path
//!
//! Currently the fast path writes into the target thread's `IpcBuffer`.
//! In a production microkernel (seL4-style) the kernel writes directly into
//! the receiving thread's trap-frame registers and restarts it with the values
//! already in place — eliminating the buffer copy entirely.  This can be
//! upgraded later by replacing `write_short()` with direct trap-frame injection.

use super::channel::ThreadId;
use super::error::IpcError;
use super::message::{
    CapabilityToken, MemoryMapPayload, Message, MessageType, ProcessId, ShortPayload,
};
use crate::kernel::bmm::AddressSpaceId;
use spin::Mutex;

// ---------------------------------------------------------------------------
// IPC receive buffer
// ---------------------------------------------------------------------------

/// Size of the per-thread IPC receive buffer (bytes).
pub const IPC_BUFFER_SIZE: usize = 256;

/// Per-thread receive buffer — holds the decoded message after delivery.
///
/// In the final kernel this lives inside the TCB.  For now it is stored in
/// a global table indexed by `ThreadId`.
#[derive(Debug, Clone)]
pub struct IpcBuffer {
    pub sender: ProcessId,
    pub msg_type: MessageType,
    pub payload: [u8; IPC_BUFFER_SIZE],
    pub payload_len: usize,
}

impl IpcBuffer {
    pub const fn empty() -> Self {
        IpcBuffer {
            sender: 0,
            msg_type: MessageType::ShortInfo,
            payload: [0u8; IPC_BUFFER_SIZE],
            payload_len: 0,
        }
    }

    /// Write a `ShortPayload` into the buffer as raw bytes.
    pub fn write_short(&mut self, sender: ProcessId, p: &ShortPayload) {
        self.sender = sender;
        self.msg_type = MessageType::ShortInfo;
        self.payload_len = 0;

        for (i, &word) in p.as_slice().iter().enumerate() {
            let bytes = word.to_ne_bytes();
            let start = i * core::mem::size_of::<usize>();
            let end = start + core::mem::size_of::<usize>();
            if end <= IPC_BUFFER_SIZE {
                self.payload[start..end].copy_from_slice(&bytes);
                self.payload_len = end;
            }
        }
    }

    /// Read back a `ShortPayload` from the buffer.
    pub fn read_short(&self) -> Option<ShortPayload> {
        let word_size = core::mem::size_of::<usize>();
        if self.payload_len == 0 || self.payload_len % word_size != 0 {
            return None;
        }
        let word_count = self.payload_len / word_size;
        if word_count > 8 {
            return None;
        }
        let mut words = [0usize; 8];
        for i in 0..word_count {
            let start = i * word_size;
            let mut bytes = [0u8; core::mem::size_of::<usize>()];
            bytes.copy_from_slice(&self.payload[start..start + word_size]);
            words[i] = usize::from_ne_bytes(bytes);
        }
        Some(ShortPayload {
            words,
            len: word_count as u8,
        })
    }
}

// ---------------------------------------------------------------------------
// Global IPC buffer table (placeholder — will move into TCB)
// ---------------------------------------------------------------------------

const MAX_THREAD_BUFFERS: usize = 32;

struct IpcBufferTable {
    buffers: [Option<IpcBuffer>; MAX_THREAD_BUFFERS],
}

impl IpcBufferTable {
    const fn new() -> Self {
        IpcBufferTable {
            buffers: [const { None }; MAX_THREAD_BUFFERS],
        }
    }
}

static IPC_BUFFERS: Mutex<IpcBufferTable> = Mutex::new(IpcBufferTable::new());

/// Get a mutable reference to a thread's IPC buffer.
///
/// # Placeholder
///
/// Currently uses a fixed-size global table.  When TCBs are implemented
/// the buffer will be a field in `ThreadControlBlock`.
pub fn get_ipc_buffer(tid: ThreadId) -> Result<IpcBuffer, IpcError> {
    let table = IPC_BUFFERS.lock();
    let idx = tid.index() as usize;
    if idx >= MAX_THREAD_BUFFERS {
        return Err(IpcError::InvalidThreadState);
    }
    table.buffers[idx]
        .clone()
        .ok_or(IpcError::InvalidThreadState)
}

/// Write the IPC buffer for a thread.
pub fn put_ipc_buffer(tid: ThreadId, buf: IpcBuffer) -> Result<(), IpcError> {
    let mut table = IPC_BUFFERS.lock();
    let idx = tid.index() as usize;
    if idx >= MAX_THREAD_BUFFERS {
        return Err(IpcError::InvalidThreadState);
    }
    table.buffers[idx] = Some(buf);
    Ok(())
}

/// Ensure a thread has an IPC buffer allocated (idempotent — does not
/// overwrite an existing buffer).
pub fn ensure_ipc_buffer(tid: ThreadId) -> Result<(), IpcError> {
    let mut table = IPC_BUFFERS.lock();
    let idx = tid.index() as usize;
    if idx >= MAX_THREAD_BUFFERS {
        return Err(IpcError::InvalidThreadState);
    }
    if table.buffers[idx].is_none() {
        table.buffers[idx] = Some(IpcBuffer::empty());
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Fast path — register / short-message delivery
// ---------------------------------------------------------------------------

/// Deliver a short message by writing into the target thread's `IpcBuffer`.
///
/// In the future this will be upgraded to write directly into the target
/// thread's trap-frame registers (the true seL4 fast path).
pub fn copy_short(
    payload: &ShortPayload,
    sender: ProcessId,
    target_tid: ThreadId,
) -> Result<(), IpcError> {
    // Ensure target has a buffer.
    ensure_ipc_buffer(target_tid)?;

    // Read the current buffer, update it, write it back.
    let mut buf = get_ipc_buffer(target_tid)?;
    buf.write_short(sender, payload);
    put_ipc_buffer(target_tid, buf)?;

    log::debug!(
        "short ipc: pid={} → tid={}, {} words",
        sender,
        target_tid,
        payload.len
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// Slow path — memory-map / page remap delivery (zero-copy)
// ---------------------------------------------------------------------------

/// Deliver a memory-map message by granting the physical frame from the
/// source address space to the destination.
///
/// This is a **zero-copy** path — `bmm::grant()` unamps the frame from the
/// source page table and maps it into the destination page table.  No data
/// is moved; only page-table entries are modified.
///
/// # Arguments
///
/// * `payload`    — describes the physical frame and mapping flags.
/// * `src_asid`   — source address space (sender's).
/// * `dst_asid`   — destination address space (receiver's).
/// * `target_vaddr` — virtual address in the destination space.
///                     `None` means re-use the same VA as the source.
pub fn copy_memory_map(
    payload: &MemoryMapPayload,
    src_asid: AddressSpaceId,
    dst_asid: AddressSpaceId,
    target_vaddr: Option<usize>,
) -> Result<(), IpcError> {
    // The sender's VMM must have called `bmm::map()` on the source side
    // before sending the IPC.  We now call `bmm::grant()` to transfer the
    // physical frame from src to dst.
    //
    // We use `target_vaddr.unwrap_or(payload.vaddr)` — by default map at the
    // same virtual address, but allow the receiver's VMM to specify a
    // different one.
    let dst_vaddr = target_vaddr.unwrap_or(0); // TODO: resolve mapping VA

    // Build address-space handles from IDs.
    // NOTE: `AddressSpace::from_token()` reconstructs a handle that does
    // NOT own the page-table root.  This is safe because both address spaces
    // are known to outlive this call (they belong to running processes).
    //
    // TODO: implement `bmm::get_address_space(id)` or similar lookup.
    // For now we log the intent; the actual grant call is gated on the
    // bmm module exposing address-space lookup by ID.
    let _ = src_asid;
    let _ = dst_asid;
    let _ = dst_vaddr;
    let _ = payload;

    log::debug!(
        "memory-map ipc: paddr=0x{:x} size=0x{:x}, src_as={:?} → dst_as={:?}",
        payload.paddr,
        payload.size,
        src_asid,
        dst_asid,
    );

    // Placeholder: when bmm exposes `lookup_address_space()`:
    //
    // let mut src_as = bmm::lookup_address_space(src_asid)
    //     .ok_or(IpcError::InvalidArgument)?;
    // let mut dst_as = bmm::lookup_address_space(dst_asid)
    //     .ok_or(IpcError::InvalidArgument)?;
    //
    // bmm::grant(&mut src_as, &mut dst_as, dst_vaddr, flags)
    //     .map_err(|_| IpcError::GrantFailed)?;

    Err(IpcError::NotImplemented)
}

// ---------------------------------------------------------------------------
// Capability transfer path (placeholder)
// ---------------------------------------------------------------------------

/// Deliver a capability grant by transferring a capability token from the
/// sender to the receiver.
///
/// # Placeholder
///
/// This is a stub — the real implementation will call into the `kernel::cap`
/// module to:
///
/// 1. Look up the sender's CSpace.
/// 2. Find the Capability referenced by `token`.
/// 3. Check `GRANT` rights on the Capability.
/// 4. Derive (or move) the Capability into the receiver's CSpace.
pub fn copy_capability(
    _token: CapabilityToken,
    _sender: ProcessId,
    _target_tid: ThreadId,
) -> Result<(), IpcError> {
    log::debug!(
        "cap ipc: token={}, pid={} → tid={} (stub)",
        _token,
        _sender,
        _target_tid,
    );
    // TODO: cap::derive::transfer_capability(token, sender, target)
    Err(IpcError::NotImplemented)
}

// ---------------------------------------------------------------------------
// Unified delivery dispatch
// ---------------------------------------------------------------------------

/// Deliver an IPC message to the target thread.
///
/// Dispatches on `MessageType` to the correct delivery path.
///
/// # Arguments
///
/// * `msg`       — the message to deliver.
/// * `target_tid` — receiving thread.
/// * `src_asid`   — sender's address space (for MemoryMap).
/// * `dst_asid`   — receiver's address space (for MemoryMap).
pub fn deliver(
    msg: &Message,
    target_tid: ThreadId,
    src_asid: Option<AddressSpaceId>,
    dst_asid: Option<AddressSpaceId>,
) -> Result<(), IpcError> {
    match msg {
        Message::Short(hdr, payload) => {
            copy_short(payload, hdr.sender, target_tid)
        }
        Message::MemoryMap(_hdr, payload) => {
            let src = src_asid.ok_or(IpcError::InvalidArgument)?;
            let dst = dst_asid.ok_or(IpcError::InvalidArgument)?;
            copy_memory_map(payload, src, dst, None)
        }
        Message::GrantCap(hdr, token) => {
            copy_capability(*token, hdr.sender, target_tid)
        }
    }
}
