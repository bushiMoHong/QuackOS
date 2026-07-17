//! IPC message types.
//!
//! Three message variants are supported:
//!
//! | Variant          | Path          | Description                            |
//! |------------------|---------------|----------------------------------------|
//! | `ShortInfo`      | Register fast | ≤8-word control messages               |
//! | `MemoryMap`      | Page remap    | Transfer physical frames (zero-copy)   |
//! | `GrantCapability`| Cap transfer  | Hand a capability to another process   |
//!
//! This file also defines the per-request types that the memory-manager
//! subsystem (`kernel::bmm`) needs, such as `IpcPageFault`, `MapRequest`,
//! `GrantRequest`, and `UnmapRequest`.

use crate::kernel::bmm::AddressSpaceId;
use crate::kernel::trap::PageFaultCause;

// ---------------------------------------------------------------------------
// Placeholder type aliases (will be replaced by task / cap modules)
// ---------------------------------------------------------------------------

/// Process identifier — placeholder, will be replaced by `task::ProcessId`.
pub type ProcessId = u32;

/// Capability token — placeholder, will be replaced by `cap::Capability`.
pub type CapabilityToken = u32;

// ---------------------------------------------------------------------------
// Message classification
// ---------------------------------------------------------------------------

/// Semantic tag that determines which transfer path the IPC engine takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageType {
    /// Short control message — fits in CPU registers, fast-path delivery.
    ShortInfo,
    /// Memory-mapping message — physical frame transfer via page-table remap.
    MemoryMap,
    /// Capability grant — hand a capability token to the receiving process.
    GrantCapability,
}

// ---------------------------------------------------------------------------
// Message header
// ---------------------------------------------------------------------------

/// Every IPC message carries a header that identifies the sender, the payload
/// kind, and the payload length.
#[derive(Debug, Clone, Copy)]
pub struct MessageHeader {
    pub sender: ProcessId,
    pub msg_type: MessageType,
    /// Payload length in bytes (for `ShortInfo`: number of valid *words* × 8).
    pub length: u32,
}

// ---------------------------------------------------------------------------
// Short (register) payload
// ---------------------------------------------------------------------------

/// Short-message payload — at most 8 machine words, delivered through the
/// register fast path (currently via `IpcBuffer`; future: direct register write).
#[derive(Debug, Clone, Copy)]
pub struct ShortPayload {
    pub words: [usize; 8],
    /// Number of valid words (1..8).
    pub len: u8,
}

impl ShortPayload {
    /// Construct a short payload from a slice of words.
    ///
    /// Returns `None` if the slice is empty or exceeds 8 words.
    /// `len` is set to the **byte count** (words × word_size).
    pub fn from_slice(words: &[usize]) -> Option<Self> {
        if words.is_empty() || words.len() > 8 {
            return None;
        }
        let mut arr = [0usize; 8];
        arr[..words.len()].copy_from_slice(words);
        Some(ShortPayload {
            words: arr,
            len: (words.len() * core::mem::size_of::<usize>()) as u8,
        })
    }

    /// Return the valid portion as a slice.
    /// `len` is the byte count (0..64); convert to word count for slicing.
    pub fn as_slice(&self) -> &[usize] {
        let word_count = ((self.len as usize) + 7) / 8;
        &self.words[..word_count]
    }
}

// ---------------------------------------------------------------------------
// Memory-map payload
// ---------------------------------------------------------------------------

/// Memory-map payload — describes a physical frame to be transferred from
/// the sender's address space to the receiver's via `bmm::grant()`.
#[derive(Debug, Clone, Copy)]
pub struct MemoryMapPayload {
    /// Physical address of the frame to transfer.
    pub paddr: usize,
    /// Frame size in bytes.
    pub size: usize,
    /// Mapping permission flags (arch-independent bitmask).
    pub flags: usize,
}

// ---------------------------------------------------------------------------
// Unified message enum
// ---------------------------------------------------------------------------

/// A complete IPC message — either a short control message, a memory-map
/// transfer, or a capability grant.
#[derive(Debug, Clone)]
pub enum Message {
    Short(MessageHeader, ShortPayload),
    MemoryMap(MessageHeader, MemoryMapPayload),
    GrantCap(MessageHeader, CapabilityToken),
}

impl Message {
    /// Convenience: create a ShortInfo message.
    /// `payload.len` is the byte count.
    pub fn new_short(sender: ProcessId, payload: ShortPayload) -> Self {
        let len = payload.len as u32;
        Message::Short(
            MessageHeader {
                sender,
                msg_type: MessageType::ShortInfo,
                length: len,
            },
            payload,
        )
    }

    /// Convenience: create a MemoryMap message.
    pub fn new_memory_map(sender: ProcessId, payload: MemoryMapPayload) -> Self {
        Message::MemoryMap(
            MessageHeader {
                sender,
                msg_type: MessageType::MemoryMap,
                length: payload.size as u32,
            },
            payload,
        )
    }

    /// Convenience: create a GrantCapability message.
    pub fn new_grant_cap(sender: ProcessId, token: CapabilityToken) -> Self {
        Message::GrantCap(
            MessageHeader {
                sender,
                msg_type: MessageType::GrantCapability,
                length: core::mem::size_of::<CapabilityToken>() as u32,
            },
            token,
        )
    }

    /// Return a reference to the header.
    pub fn header(&self) -> &MessageHeader {
        match self {
            Message::Short(h, _) => h,
            Message::MemoryMap(h, _) => h,
            Message::GrantCap(h, _) => h,
        }
    }

    /// Return the message type.
    pub fn msg_type(&self) -> MessageType {
        self.header().msg_type
    }
}

// ---------------------------------------------------------------------------
// Types required by `kernel::bmm`
// ---------------------------------------------------------------------------

/// Page-fault IPC message — delivered from the kernel's page-fault handler
/// to the user-space memory-manager server.
///
/// Constructed by `bmm::handle_page_fault()` and dequeued by the IPC delivery
/// path for forwarding to the mm server.
#[derive(Debug, Clone, Copy)]
pub struct IpcPageFault {
    pub addr_space_id: AddressSpaceId,
    pub fault_vaddr: usize,
    pub cause: PageFaultCause,
}

/// Request from user-space mm to the kernel: map a physical frame into an
/// address space.
#[derive(Debug, Clone, Copy)]
pub struct MapRequest {
    pub addr_space_id: AddressSpaceId,
    pub vaddr: usize,
    pub paddr: usize,
    /// Arch-independent permission bitmask (see `bmm::MapFlags`).
    pub flags: usize,
}

/// Request from user-space mm: transfer a physical frame from one address
/// space to another (unmap from src, map into dst).
#[derive(Debug, Clone, Copy)]
pub struct GrantRequest {
    pub src_addr_space_id: AddressSpaceId,
    pub dst_addr_space_id: AddressSpaceId,
    pub vaddr: usize,
    pub flags: usize,
}

/// Request from user-space mm: unmap a virtual address.
#[derive(Debug, Clone, Copy)]
pub struct UnmapRequest {
    pub addr_space_id: AddressSpaceId,
    pub vaddr: usize,
}
