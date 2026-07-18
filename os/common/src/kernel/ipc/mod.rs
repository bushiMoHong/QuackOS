//! Inter-Process Communication (IPC) subsystem.
//!
//! # Architecture
//!
//! The IPC module is the backbone of this microkernel — every interaction
//! between user-space servers and the kernel goes through it.
//!
//! ```text
//! ┌──────────┐    syscall     ┌──────────────┐    channel     ┌──────────┐
//! │ Process A │ ────────────▶ │  ipc::mod    │ ────────────▶ │ Process B │
//! │ (client)  │               │  (this file) │               │ (server)  │
//! └──────────┘                └──────┬───────┘               └──────────┘
//!                                    │
//!               ┌────────────────────┼────────────────────┐
//!               │                    │                    │
//!               ▼                    ▼                    ▼
//!         channel.rs           transfer.rs        synchronization.rs
//!         (rendezvous)         (data movement)    (block / wake)
//! ```
//!
//! # Sub-modules
//!
//! | Module             | Purpose                                   |
//! |--------------------|-------------------------------------------|
//! | `message`          | Message types + bmm-facing request types  |
//! | `channel`          | Channel object, wait queues, matching     |
//! | `transfer`         | Fast path (register) / slow path (page remap) |
//! | `synchronization`  | Thread block / wake via TCB               |
//! | `capability`       | Permission checks (placeholder → cap module) |
//! | `error`            | Unified `IpcError` enum                   |
//!
//! # Syscall entry points
//!
//! These are the three IPC syscalls that the architecture's trap handler
//! dispatches to:
//!
//! * `sys_ipc_send(sender_pid, channel_id, msg, src_asid, dst_asid)`
//! * `sys_ipc_recv(receiver_pid, channel_id, dst_asid)`
//! * `sys_ipc_call(caller_pid, channel_id, msg, src_asid, dst_asid)`

pub mod capability;
pub mod channel;
pub mod error;
pub mod message;
pub mod synchronization;
pub mod transfer;

// Re-export everything public so that users only need `use crate::kernel::ipc`.
pub use capability::{
    check_call_right, check_grant_right, check_recv_right, check_send_right,
    derive_cap, mint_channel_cap, CapRights, Capability,
};
pub use channel::{
    channel_count, channel_exists, create_channel, destroy_channel, with_channel,
    BlockReason, ChannelId, ChannelInner, RecvMatch, SendMatch, WaitEntry,
};
// ThreadId is re-exported from kernel::sche via channel::ThreadId
pub use channel::ThreadId;
pub use error::IpcError;
pub use message::{
    GrantRequest, IpcPageFault, MapRequest, MemoryMapPayload, Message, MessageHeader,
    MessageType, ProcessId, ShortPayload, UnmapRequest,
};
pub use synchronization::{block_current, current_thread, wake, IpcState};
pub use transfer::{
    copy_capability, copy_memory_map, copy_short, deliver, get_ipc_buffer,
    IpcBuffer, IPC_BUFFER_SIZE,
};

// ---------------------------------------------------------------------------
// Initialisation
// ---------------------------------------------------------------------------

/// Initialise the IPC subsystem.
///
/// Must be called once during kernel boot, before any IPC syscalls are
/// serviced.  Currently a no-op (all data structures use const constructors),
/// but provides a hook for future per-CPU setup.
pub fn init() {
    log::info!("ipc: subsystem initialised");
}

// ---------------------------------------------------------------------------
// Syscall dispatchers
// ---------------------------------------------------------------------------

/// `sys_ipc_send` — send a message through a channel.
///
/// # Flow
///
/// 1. Capability check: does `sender_pid` hold SEND right on `channel_id`?
/// 2. Lock channel; try to match with a waiting receiver.
/// 3. Matched → drop lock → `transfer::deliver()` → `wake(receiver)` → return.
/// 4. Unmatched → park sender (msg cloned into queue) → drop lock → block.
pub fn sys_ipc_send(
    sender_pid: ProcessId,
    channel_id: ChannelId,
    msg: Message,
    src_asid: Option<crate::kernel::bmm::AddressSpaceId>,
    dst_asid: Option<crate::kernel::bmm::AddressSpaceId>,
) -> Result<(), IpcError> {
    check_send_right(sender_pid, channel_id)?;

    let tid = current_thread();

    // Matching under channel lock — msg is borrowed so the caller
    // retains ownership for the deliver path.
    let action = with_channel(channel_id, |inner| {
        inner.match_sender(tid, &msg)
    })?;

    match action {
        SendMatch::Matched(receiver_tid) => {
            // Receiver was waiting.  Deliver the message and wake it.
            // Lock was released by `with_channel` — safe to call transfer + wake.
            log::debug!("ipc_send: tid={} → matched receiver tid={}", tid, receiver_tid);
            deliver(&msg, receiver_tid, src_asid, dst_asid)?;
            wake(receiver_tid);
            Ok(())
        }
        SendMatch::Parked => {
            // No receiver yet — our msg was cloned into sender_queue
            // by match_sender().  Block until a receiver arrives.
            log::debug!("ipc_send: tid={} parked on ch={:?}", tid, channel_id);
            unsafe {
                block_current(IpcState::BlockedOnSend(channel_id));
            }
            // Woken by a receiver that already completed the transfer.
            Ok(())
        }
    }
}

/// `sys_ipc_recv` — receive a message from a channel.
///
/// # Flow
///
/// 1. Capability check: does `receiver_pid` hold RECV right on `channel_id`?
/// 2. Lock channel; try to match with a waiting sender.
/// 3. Matched → pop `WaitEntry` (carries msg) → drop lock → deliver to self
///    → `wake(sender)` → return msg.
/// 4. Unmatched → park receiver → drop lock → block → on wake, read from
///    own `IpcBuffer`.
pub fn sys_ipc_recv(
    receiver_pid: ProcessId,
    channel_id: ChannelId,
    dst_asid: Option<crate::kernel::bmm::AddressSpaceId>,
) -> Result<Message, IpcError> {
    check_recv_right(receiver_pid, channel_id)?;

    let tid = current_thread();

    let action = with_channel(channel_id, |inner| {
        inner.match_receiver(tid)
    })?;

    match action {
        RecvMatch::Matched(sender_entry) => {
            let sender_msg = sender_entry.msg.ok_or(IpcError::InvalidArgument)?;
            let sender_tid = sender_entry.thread_id;

            log::debug!(
                "ipc_recv: tid={} matched sender tid={}",
                tid,
                sender_tid
            );

            // Deliver the sender's message into our IPC buffer, then wake
            // the sender.  Order matters: data must be available before wake.
            deliver(&sender_msg, tid, None, dst_asid)?;
            wake(sender_tid);

            Ok(sender_msg)
        }
        RecvMatch::Parked => {
            log::debug!("ipc_recv: tid={} parked on ch={:?}", tid, channel_id);
            unsafe {
                block_current(IpcState::BlockedOnReceive(channel_id));
            }
            // Woken by a sender.  Message was delivered to our IPC buffer
            // by the sender before it called wake().
            let buf = get_ipc_buffer(tid)?;
            if let Some(payload) = buf.read_short() {
                Ok(Message::new_short(buf.sender, payload))
            } else {
                // MemoryMap / GrantCap — the transfer path handled the
                // payload; return a minimal reconstructed message.
                // TODO: store full Message metadata in IpcBuffer.
                Ok(Message::new_short(
                    buf.sender,
                    ShortPayload {
                        words: [0; 32],
                        len: 0,
                    },
                ))
            }
        }
    }
}

/// `sys_ipc_call` — atomic send + receive on the same channel.
///
/// The caller sends `msg` and blocks until the server replies.
/// The reply is routed back to this specific thread (the server uses
/// `sys_ipc_send` to reply, which matches the first blocked receiver).
pub fn sys_ipc_call(
    caller_pid: ProcessId,
    channel_id: ChannelId,
    msg: Message,
    src_asid: Option<crate::kernel::bmm::AddressSpaceId>,
    dst_asid: Option<crate::kernel::bmm::AddressSpaceId>,
) -> Result<Message, IpcError> {
    check_call_right(caller_pid, channel_id)?;

    // Send phase.
    sys_ipc_send(caller_pid, channel_id, msg, src_asid, dst_asid)?;

    // Receive reply.  The server replies via sys_ipc_send(), which sees us
    // as a blocked receiver (we were parked by sys_ipc_recv if no reply
    // was already waiting).
    sys_ipc_recv(caller_pid, channel_id, dst_asid)
}
