//! Capability Management — the kernel's access-control backbone.
//!
//! # Design (seL4-style)
//!
//! Every kernel object (Channel, Thread, PageTable, Frame, …) is accessed
//! through a **capability** — an unforgeable reference with attached rights.
//! User-space processes hold capabilities in their **CSpace** (capability
//! space), a tree of `CNode` tables indexed by capability pointers (CPtr).
//!
//! | Module       | Purpose                                         |
//! |--------------|-------------------------------------------------|
//! | `cspace`     | CSpace / CNode management + IPC right lookups   |
//! | `derive`     | Mint / derive / revoke + grant-chain checks     |
//! | `allocator`  | Untyped-memory → typed-object retype (placeholder) |
//! | `error`      | `CapError` enum                                 |
//!
//! # Integration with IPC
//!
//! `kernel::ipc::capability` delegates to `cap::cspace` for permission
//! checks and to `cap::derive` for capability lifecycle operations.
//!
//! ```text
//! ipc::sys_ipc_send(pid, ch_id, msg)
//!   → ipc::capability::check_send_right(pid, ch_id)
//!     → cap::cspace::lookup_send_right(pid, ch_id)
//!       → find CSpace[pid] → search for Capability(ch_id, SEND) → Ok / Err
//! ```

pub mod allocator;
pub mod cspace;
pub mod derive;
pub mod error;

pub use allocator::{retype, RetypeError};
pub use cspace::{
    create_cspace, destroy_cspace, cspace_exists,
    lookup_call_right, lookup_recv_right, lookup_send_right,
    CNode, CSpace, CPtr, CSLOT_COUNT,
};
pub use derive::{
    check_grant_chain, derive_cap, mint_cap, revoke, DeriveError,
};
pub use error::CapError;

use crate::kernel::ipc::channel::ChannelId;

// ---------------------------------------------------------------------------
// Capability Type
// ---------------------------------------------------------------------------

/// All kernel-object types that can be referenced by a capability.
///
/// `Untyped` is the root of all capabilities — raw physical memory that
/// can be *retyped* into any other type via `allocator::retype()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapType {
    /// Raw untyped memory — the "mother of all capabilities".
    Untyped,
    /// IPC endpoint / channel.
    Endpoint,
    /// Thread control block.
    Thread,
    /// Page-table root.
    PageTable,
    /// Physical memory frame.
    Frame,
    /// Asynchronous notification object.
    Notification,
    /// Capability-space CNode (for multi-level CSpace).
    CNode,
}

// ---------------------------------------------------------------------------
// Capability Rights
// ---------------------------------------------------------------------------

/// Access-rights bitmask attached to every capability.
///
/// These bits are checked on every capability invocation.  Rights can only
/// be **removed** when deriving a child capability, never added.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CapRights(pub u16);

impl CapRights {
    pub const NONE:  u16 = 0;
    pub const READ:  u16 = 1 << 0;
    pub const WRITE: u16 = 1 << 1;
    pub const GRANT: u16 = 1 << 2;
    pub const SEND:  u16 = 1 << 3;
    pub const RECV:  u16 = 1 << 4;
    /// Combined SEND | RECV — required for `sys_ipc_call`.
    pub const CALL:  u16 = Self::SEND | Self::RECV;

    pub const fn empty() -> Self { CapRights(0) }
    pub const fn full() -> Self {
        CapRights(Self::READ | Self::WRITE | Self::GRANT | Self::SEND | Self::RECV)
    }

    /// `true` if this rights set contains all bits in `other`.
    pub fn contains(&self, other: u16) -> bool {
        self.0 & other == other
    }

    /// Return a new `CapRights` with the given bits removed.
    pub fn without(&self, bits: u16) -> Self {
        CapRights(self.0 & !bits)
    }
}

// ---------------------------------------------------------------------------
// Capability
// ---------------------------------------------------------------------------

/// A kernel-verified reference to a kernel object with attached rights.
///
/// # Fields
///
/// * `obj_id` — index / identifier of the target kernel object.
/// * `cap_type` — what kind of object this capability references.
/// * `rights` — access rights (bitmask).
/// * `parent_id` — for derivation chain; `None` for root (minted) capabilities.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Capability {
    /// Kernel-object identifier (e.g. `ChannelId.0`, thread index, …).
    pub obj_id: usize,
    /// Object type.
    pub cap_type: CapType,
    /// Access rights.
    pub rights: CapRights,
    /// Index of the parent capability in the derivation table (`None` = root).
    pub parent_id: Option<usize>,
}

impl Capability {
    /// Create a new root capability (no parent).
    pub const fn new(obj_id: usize, cap_type: CapType, rights: CapRights) -> Self {
        Capability {
            obj_id,
            cap_type,
            rights,
            parent_id: None,
        }
    }

    /// Create a child capability derived from a parent.
    pub const fn derived(
        obj_id: usize,
        cap_type: CapType,
        rights: CapRights,
        parent_id: usize,
    ) -> Self {
        Capability {
            obj_id,
            cap_type,
            rights,
            parent_id: Some(parent_id),
        }
    }

    /// Return `true` if this capability grants at least `bits` rights.
    pub fn has_rights(&self, bits: u16) -> bool {
        self.rights.contains(bits)
    }
}

// ---------------------------------------------------------------------------
// Initialisation
// ---------------------------------------------------------------------------

/// Initialise the capability subsystem.
pub fn init() {
    log::info!("cap: subsystem initialised");
    // Future:
    // - Bootstrap CSpace for the init process.
    // - Create untyped capabilities covering all free physical memory.
}

// ---------------------------------------------------------------------------
// Convenience: create an IPC Endpoint capability from a ChannelId
// ---------------------------------------------------------------------------

/// Wrap a `ChannelId` into an endpoint `Capability` with full rights.
///
/// Called by `channel::create_channel()` so the resulting channel
/// can immediately be distributed via capability derivation.
pub fn make_endpoint_cap(channel_id: ChannelId) -> Capability {
    Capability::new(
        channel_id.0 as usize,
        CapType::Endpoint,
        CapRights::full(),
    )
}
