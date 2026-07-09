//! Capability Space — per-process "asset ledger".
//!
//! # Structure
//!
//! Each process has a `CSpace` containing a root `CNode` — a fixed-size
//! table of capability slots.  User-space refers to capabilities by
//! **CPtr** (capability pointer), which is simply an index into the root
//! CNode's slot array.
//!
//! ```text
//! CSpace[pid]
//!   └── root CNode
//!         ├─ slot[0]: Capability { obj_id=3, Endpoint, SEND|RECV }
//!         ├─ slot[1]: Capability { obj_id=5, Frame, READ|WRITE }
//!         ├─ slot[2]: None (empty)
//!         └─ …
//! ```
//!
//! # Future: multi-level CSpace
//!
//! In a full seL4-style kernel a CSpace is a tree of CNodes indexed by
//! guard-protected paths.  For now we use a single flat CNode per process;
//! the API (`CPtr`, `CNode`) is named to make the upgrade mechanical.

use super::error::CapError;
use super::{CapRights, CapType, Capability};
use crate::kernel::ipc::channel::ChannelId;
use crate::kernel::ipc::message::ProcessId;
use spin::Mutex;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Number of capability slots per CNode.
pub const CSLOT_COUNT: usize = 64;

/// Maximum number of processes that can have a CSpace.
const MAX_CSPACES: usize = 64;

// ---------------------------------------------------------------------------
// CPtr — capability pointer (user-space handle)
// ---------------------------------------------------------------------------

/// A capability pointer — the user-space handle for a capability.
///
/// Equivalent to a file descriptor in Unix: an opaque index that the kernel
/// resolves to a `Capability` via the calling process's CSpace.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CPtr(pub usize);

// ---------------------------------------------------------------------------
// CNode — capability table
// ---------------------------------------------------------------------------

/// A capability table with `CSLOT_COUNT` slots.
///
/// In seL4 a CSpace is a tree of CNodes (like page tables for capabilities).
/// For now we use a single-level CNode as the CSpace root.
pub struct CNode {
    pub slots: [Option<Capability>; CSLOT_COUNT],
    /// Guard value for path resolution (reserved for multi-level CSpace).
    pub guard: usize,
}

impl CNode {
    pub(crate) const fn new() -> Self {
        CNode {
            slots: [const { None }; CSLOT_COUNT],
            guard: 0,
        }
    }

    /// Insert a capability into an empty slot.  Returns the `CPtr`.
    pub(crate) fn insert(&mut self, cap: Capability) -> Result<CPtr, CapError> {
        for (i, slot) in self.slots.iter_mut().enumerate() {
            if slot.is_none() {
                *slot = Some(cap);
                return Ok(CPtr(i));
            }
        }
        Err(CapError::CNodeFull)
    }

    /// Remove a capability from a slot.
    pub(crate) fn remove(&mut self, cptr: CPtr) -> Result<Capability, CapError> {
        let slot = self
            .slots
            .get_mut(cptr.0)
            .ok_or(CapError::InvalidCPtr)?;
        slot.take().ok_or(CapError::EmptySlot)
    }

    /// Look up a capability by CPtr (read-only).
    pub(crate) fn lookup(&self, cptr: CPtr) -> Result<&Capability, CapError> {
        self.slots
            .get(cptr.0)
            .ok_or(CapError::InvalidCPtr)?
            .as_ref()
            .ok_or(CapError::EmptySlot)
    }

    /// Search for a capability matching the given object and type.
    pub(crate) fn search(&self, obj_id: usize, cap_type: CapType) -> Option<&Capability> {
        self.slots.iter().find_map(|slot| {
            slot.as_ref()
                .filter(|c| c.obj_id == obj_id && c.cap_type == cap_type)
        })
    }
}

// ---------------------------------------------------------------------------
// CSpace — per-process capability space
// ---------------------------------------------------------------------------

/// A process's capability space.
pub struct CSpace {
    /// Owning process ID.
    pub pid: ProcessId,
    /// Root capability table.
    pub root: CNode,
}

impl CSpace {
    pub const fn new(pid: ProcessId) -> Self {
        CSpace {
            pid,
            root: CNode::new(),
        }
    }

    /// Insert a capability and return its CPtr.
    pub fn insert(&mut self, cap: Capability) -> Result<CPtr, CapError> {
        self.root.insert(cap)
    }

    /// Remove and return a capability by CPtr.
    pub fn remove(&mut self, cptr: CPtr) -> Result<Capability, CapError> {
        self.root.remove(cptr)
    }

    /// Look up a capability by CPtr.
    pub fn lookup(&self, cptr: CPtr) -> Result<&Capability, CapError> {
        self.root.lookup(cptr)
    }

    /// Search for a capability matching `obj_id` and `cap_type`.
    pub fn search(&self, obj_id: usize, cap_type: CapType) -> Option<&Capability> {
        self.root.search(obj_id, cap_type)
    }
}

// ---------------------------------------------------------------------------
// Global CSpace table
// ---------------------------------------------------------------------------

struct CSpaceTable {
    spaces: [Option<CSpace>; MAX_CSPACES],
}

impl CSpaceTable {
    const fn new() -> Self {
        CSpaceTable {
            spaces: [const { None }; MAX_CSPACES],
        }
    }

    fn get(&self, pid: ProcessId) -> Option<&CSpace> {
        self.spaces.iter().find_map(|s| {
            s.as_ref().filter(|cs| cs.pid == pid)
        })
    }

    fn get_mut(&mut self, pid: ProcessId) -> Option<&mut CSpace> {
        self.spaces.iter_mut().find_map(|s| {
            s.as_mut().filter(|cs| cs.pid == pid)
        })
    }

    fn insert(&mut self, cspace: CSpace) -> Result<(), CapError> {
        for slot in self.spaces.iter_mut() {
            if slot.is_none() {
                *slot = Some(cspace);
                return Ok(());
            }
        }
        Err(CapError::CSpaceFull)
    }

    fn remove(&mut self, pid: ProcessId) -> Result<(), CapError> {
        for slot in self.spaces.iter_mut() {
            if slot.as_ref().is_some_and(|cs| cs.pid == pid) {
                *slot = None;
                return Ok(());
            }
        }
        Err(CapError::InvalidProcess)
    }
}

static CSPACES: Mutex<CSpaceTable> = Mutex::new(CSpaceTable::new());

// ---------------------------------------------------------------------------
// Public API — CSpace lifecycle
// ---------------------------------------------------------------------------

/// Create a new CSpace for the given process.
///
/// # Placeholder
///
/// When the task module lands this will be called automatically during
/// process creation.  For now it must be called manually (e.g. during
/// kernel init for the root server).
pub fn create_cspace(pid: ProcessId) -> Result<(), CapError> {
    let mut table = CSPACES.lock();
    if table.get(pid).is_some() {
        // CSpace already exists — idempotent.
        return Ok(());
    }
    table.insert(CSpace::new(pid))
}

/// Destroy the CSpace for a process (called on process exit).
pub fn destroy_cspace(pid: ProcessId) -> Result<(), CapError> {
    CSPACES.lock().remove(pid)
}

/// Return `true` if a CSpace exists for `pid`.
pub fn cspace_exists(pid: ProcessId) -> bool {
    CSPACES.lock().get(pid).is_some()
}

// ---------------------------------------------------------------------------
// Operations on a CSpace (accessed via closure)
// ---------------------------------------------------------------------------

/// Run `f` with a mutable reference to the CSpace of `pid`.
///
/// If no CSpace exists for `pid`, one is automatically created.
fn with_cspace_mut<R>(
    pid: ProcessId,
    f: impl FnOnce(&mut CSpace) -> Result<R, CapError>,
) -> Result<R, CapError> {
    let mut table = CSPACES.lock();
    // Auto-create if missing (placeholder behaviour; real kernel would fail).
    if table.get(pid).is_none() {
        table.insert(CSpace::new(pid))?;
    }
    let cspace = table.get_mut(pid).ok_or(CapError::InvalidProcess)?;
    f(cspace)
}

/// Run `f` with a shared reference to the CSpace of `pid`.
fn with_cspace<R>(
    pid: ProcessId,
    f: impl FnOnce(&CSpace) -> Result<R, CapError>,
) -> Result<R, CapError> {
    let table = CSPACES.lock();
    let cspace = table.get(pid).ok_or(CapError::InvalidProcess)?;
    f(cspace)
}

/// Insert a capability into a process's CSpace and return its CPtr.
pub fn insert_cap(pid: ProcessId, cap: Capability) -> Result<CPtr, CapError> {
    with_cspace_mut(pid, |cs| cs.insert(cap))
}

/// Remove a capability from a process's CSpace.
pub fn remove_cap(pid: ProcessId, cptr: CPtr) -> Result<Capability, CapError> {
    with_cspace_mut(pid, |cs| cs.remove(cptr))
}

/// Look up a capability by CPtr.
pub fn lookup_cap(pid: ProcessId, cptr: CPtr) -> Result<Capability, CapError> {
    with_cspace(pid, |cs| cs.lookup(cptr).copied())
}

// ---------------------------------------------------------------------------
// IPC integration — right-check lookups
// ---------------------------------------------------------------------------

/// Internal helper: search for an Endpoint capability matching `channel_id`,
/// then verify it holds `required_rights`.
fn lookup_endpoint_rights(
    pid: ProcessId,
    channel_id: ChannelId,
    required_rights: u16,
) -> Result<(), CapError> {
    with_cspace(pid, |cs| {
        let cap = cs
            .search(channel_id.0 as usize, CapType::Endpoint)
            .ok_or(CapError::EmptySlot)?;
        if cap.rights.contains(required_rights) {
            Ok(())
        } else {
            Err(CapError::RightsEscalation)
        }
    })
}

/// Check that `pid` holds an Endpoint capability for `channel_id` with SEND right.
pub fn lookup_send_right(pid: ProcessId, channel_id: ChannelId) -> Result<(), CapError> {
    lookup_endpoint_rights(pid, channel_id, CapRights::SEND)
}

/// Check that `pid` holds an Endpoint capability for `channel_id` with RECV right.
pub fn lookup_recv_right(pid: ProcessId, channel_id: ChannelId) -> Result<(), CapError> {
    lookup_endpoint_rights(pid, channel_id, CapRights::RECV)
}

/// Check that `pid` holds an Endpoint capability for `channel_id` with CALL right (SEND | RECV).
pub fn lookup_call_right(pid: ProcessId, channel_id: ChannelId) -> Result<(), CapError> {
    lookup_endpoint_rights(pid, channel_id, CapRights::CALL)
}
