//! Capability derivation — mint, derive, revoke.
//!
//! # Derivation tree
//!
//! Every capability is either **minted** (root — no parent) or **derived**
//! (child — rights ≤ parent).  The derivation tree is tracked in a global
//! table keyed by a unique capability ID assigned at creation time.
//!
//! ```text
//! mint(A, full) → cap_id=1
//!   └─ derive(1, SEND) → cap_id=2
//!        └─ derive(2, SEND) → cap_id=3
//! ```
//!
//! Revoking cap_id=1 invalidates all three.  `check_grant_chain()` walks
//! from a leaf up to the root; if any ancestor is revoked, the chain is broken.
//!
//! # Capability ID
//!
//! Each `Capability` in a CSpace slot carries a unique `cap_id` embedded in
//! its `parent_id`-adjacent metadata.  The global `DERIVE_TABLE` maps
//! `cap_id → (parent_id, revoked_flag, children_ids)`.

use super::error::CapError;
use super::{CapRights, CapType, Capability};
use crate::kernel::ipc::message::ProcessId;
use spin::Mutex;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum number of tracked capability derivations.
const MAX_DERIVATIONS: usize = 256;

/// Maximum children per capability.
const MAX_CHILDREN: usize = 16;

// ---------------------------------------------------------------------------
// DeriveEntry — metadata for one capability in the derivation tree
// ---------------------------------------------------------------------------

/// A node in the global derivation tree.
#[derive(Debug, Clone)]
#[allow(dead_code)]
struct DeriveEntry {
    /// The capability itself.
    cap: Capability,
    /// Index of the parent entry (`None` = root).
    parent: Option<usize>,
    /// Whether this capability (and thus all children) has been revoked.
    revoked: bool,
    /// Indices of direct children.
    children: [Option<usize>; MAX_CHILDREN],
    child_count: usize,
    /// Which process's CSpace holds this capability (for revoke lookup).
    owner: ProcessId,
}

impl DeriveEntry {
    fn new(cap: Capability, parent: Option<usize>, owner: ProcessId) -> Self {
        DeriveEntry {
            cap,
            parent,
            revoked: false,
            children: [const { None }; MAX_CHILDREN],
            child_count: 0,
            owner,
        }
    }
}

// ---------------------------------------------------------------------------
// Global derivation table
// ---------------------------------------------------------------------------

struct DeriveTable {
    entries: [Option<DeriveEntry>; MAX_DERIVATIONS],
    next_id: usize,
}

impl DeriveTable {
    const fn new() -> Self {
        DeriveTable {
            entries: [const { None }; MAX_DERIVATIONS],
            next_id: 0,
        }
    }

    fn alloc_id(&mut self) -> usize {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        id
    }

    fn insert(&mut self, entry: DeriveEntry) -> Option<usize> {
        let id = self.alloc_id();
        if id >= MAX_DERIVATIONS {
            return None;
        }
        self.entries[id] = Some(entry);
        Some(id)
    }

    fn get(&self, id: usize) -> Option<&DeriveEntry> {
        self.entries.get(id).and_then(|e| e.as_ref())
    }

    fn get_mut(&mut self, id: usize) -> Option<&mut DeriveEntry> {
        self.entries.get_mut(id).and_then(|e| e.as_mut())
    }

    /// Add a child reference to a parent entry.
    fn add_child(&mut self, parent_id: usize, child_id: usize) -> Result<(), CapError> {
        let parent = self
            .get_mut(parent_id)
            .ok_or(CapError::InvalidCPtr)?;
        if parent.child_count >= MAX_CHILDREN {
            return Err(CapError::CNodeFull);
        }
        parent.children[parent.child_count] = Some(child_id);
        parent.child_count += 1;
        Ok(())
    }
}

static DERIVE_TABLE: Mutex<DeriveTable> = Mutex::new(DeriveTable::new());

// ---------------------------------------------------------------------------
// Mint — create a root capability
// ---------------------------------------------------------------------------

/// Mint a brand-new capability (no parent).
///
/// This is the "root" operation — the returned capability has full rights
/// over the kernel object and can be used to `derive_cap` restricted children.
///
/// Returns the capability with its `cap_id` set (stored in `parent_id` as a
/// side-channel; `parent_id == None` for roots).
pub fn mint_cap(
    obj_id: usize,
    cap_type: CapType,
    rights: CapRights,
    owner: ProcessId,
) -> Result<Capability, CapError> {
    let mut table = DERIVE_TABLE.lock();

    let mut cap = Capability::new(obj_id, cap_type, rights);
    let entry = DeriveEntry::new(cap, None, owner);
    let cap_id = table.insert(entry).ok_or(CapError::CNodeFull)?;

    // Store the cap_id back into the capability for later chain walking.
    // We re-use `parent_id` encoding: for root caps, store the cap_id
    // negated so we can distinguish root from derived.
    // Actually, `parent_id` is already None for root.  We need a separate
    // way to map from a Capability back to its DeriveEntry.
    //
    // Simplification: the caller is responsible for remembering which
    // DeriveEntry ID a capability belongs to.  The CSpace slot stores
    // the Capability; cap-level operations pass the entry ID separately.
    //
    // For the common case (IPC), capability identity is tracked by the
    // CSpace (pid, cptr) pair.  Derive / revoke operations are rare
    // and can be driven by the syscall layer which knows the CPtr.
    //
    // For now, we encode the cap_id in the parent_id field:
    // - `parent_id == None` → root (minted)
    // - `parent_id == Some(id)` → derived from `id`
    // And we add a hidden `self_id` concept …
    //
    // Cleanest for now: store `cap_id` as the capability's `obj_id` high bits
    // or use a separate lookup.  Let's keep it simple: the derive table is
    // indexed by (owner_pid, cptr) in practice.  We'll add an explicit
    // `cap_id` field to Capability.
    //
    // Actually, let's just use `cap.parent_id` cleverly:
    // For minted caps: parent_id = None, we store the cap_id nowhere extra.
    // Lookup for grant chain: we need cap_id → DeriveEntry.  Instead,
    // we scan the DeriveTable for matching (obj_id, cap_type, owner).
    // This is O(N) but N ≤ 256, fine for now.

    cap.parent_id = Some(cap_id); // root cap stores its own derive-table index
    Ok(cap)
}

// ---------------------------------------------------------------------------
// Derive — create a child with reduced rights
// ---------------------------------------------------------------------------

/// Error returned by derive operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeriveError {
    /// The requested rights exceed the parent's.
    RightsEscalation,
    /// The parent capability was revoked.
    ParentRevoked,
    /// The derivation table is full.
    TableFull,
}

/// Derive a child capability with reduced rights.
///
/// # Rules
///
/// 1. `new_rights` must be a subset of the parent's rights (no escalation).
/// 2. The parent must not have been revoked.
/// 3. The child shares the same `obj_id` and `cap_type` as the parent.
///
/// Returns the new child capability.
pub fn derive_cap(
    parent: &Capability,
    new_rights: CapRights,
    owner: ProcessId,
) -> Result<Capability, DeriveError> {
    // 1. Rights check — can only reduce, never escalate.
    if !parent.rights.contains(new_rights.0) {
        return Err(DeriveError::RightsEscalation);
    }

    let parent_id = parent.parent_id.ok_or(DeriveError::ParentRevoked)?;

    let mut table = DERIVE_TABLE.lock();

    // 2. Check parent not revoked.
    let parent_entry = table.get(parent_id).ok_or(DeriveError::ParentRevoked)?;
    if parent_entry.revoked {
        return Err(DeriveError::ParentRevoked);
    }

    // 3. Create child.
    let mut child = Capability::derived(parent.obj_id, parent.cap_type, new_rights, parent_id);
    let child_entry = DeriveEntry::new(child, Some(parent_id), owner);
    let child_id = table.insert(child_entry).ok_or(DeriveError::TableFull)?;

    // 4. Link parent → child.
    table.add_child(parent_id, child_id).map_err(|_| DeriveError::TableFull)?;

    // Encode child's own derive-table index.
    child.parent_id = Some(child_id);

    Ok(child)
}

// ---------------------------------------------------------------------------
// Revoke
// ---------------------------------------------------------------------------

/// Revoke a capability and all of its descendants.
///
/// After revocation:
/// - The capability and all children are marked revoked in the derive table.
/// - Any subsequent `check_grant_chain()` on a descendant will fail.
/// - The CSpace slot is NOT automatically cleared (caller does that).
///
/// Returns the number of capabilities revoked.
pub fn revoke(cap: &Capability) -> Result<usize, CapError> {
    let cap_id = cap.parent_id.ok_or(CapError::InvalidCPtr)?;
    let mut table = DERIVE_TABLE.lock();
    revoke_recursive(&mut table, cap_id)
}

/// Recursively revoke a node and all its children.
fn revoke_recursive(table: &mut DeriveTable, id: usize) -> Result<usize, CapError> {
    let entry = match table.get_mut(id) {
        Some(e) => e,
        None => return Ok(0),
    };

    if entry.revoked {
        return Ok(0); // already revoked
    }

    entry.revoked = true;
    let mut count = 1;

    // Revoke all children recursively.
    let child_ids: [Option<usize>; MAX_CHILDREN] = entry.children;
    let child_count = entry.child_count;

    for i in 0..child_count {
        if let Some(child_id) = child_ids[i] {
            count += revoke_recursive(table, child_id)?;
        }
    }

    Ok(count)
}

// ---------------------------------------------------------------------------
// Grant-chain check
// ---------------------------------------------------------------------------

/// Verify that the capability's derivation chain is intact.
///
/// Walks from the given capability up to the root, checking that no
/// ancestor (including this capability) has been revoked.
pub fn check_grant_chain(cap: &Capability) -> Result<(), CapError> {
    let cap_id = cap.parent_id.ok_or(CapError::InvalidCPtr)?;
    let table = DERIVE_TABLE.lock();
    check_chain_recursive(&table, cap_id)
}

/// Walk up the chain from `id` to root, checking for revocations.
fn check_chain_recursive(table: &DeriveTable, id: usize) -> Result<(), CapError> {
    let mut current_id = Some(id);
    while let Some(cid) = current_id {
        let entry = table.get(cid).ok_or(CapError::InvalidCPtr)?;
        if entry.revoked {
            return Err(CapError::GrantChainBroken);
        }
        current_id = entry.parent;
    }
    Ok(())
}

/// Return the number of tracked derivations (for debugging).
pub fn derivation_count() -> usize {
    DERIVE_TABLE
        .lock()
        .entries
        .iter()
        .filter(|e| e.is_some())
        .count()
}
