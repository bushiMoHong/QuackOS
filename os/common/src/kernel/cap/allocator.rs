//! Untyped-memory allocator — retype raw memory into typed kernel objects.
//!
//! # Microkernel philosophy
//!
//! The kernel does **not** dynamically allocate memory for kernel objects
//! (no `malloc`).  Instead, all free physical memory is given to user-space
//! as **Untyped** capabilities at boot.  When user-space wants to create a
//! new kernel object (thread, page table, endpoint, …) it calls `retype()`
//! to "reshape" a portion of an Untyped capability into the desired type.
//!
//! ```text
//! Untyped(0x8000_0000, 1 MiB)
//!   │
//!   └─ retype(Endpoint) → Capability { obj_id=3, Endpoint, full_rights }
//!   └─ retype(Thread)   → Capability { obj_id=1, Thread, full_rights }
//!   └─ retype(Frame, 4K) → Capability { obj_id=0x8000_1000, Frame, RW }
//! ```
//!
//! # Current state: placeholder
//!
//! The memory subsystem is not yet complete.  This file provides the
//! interface stubs so that `cap::derive::mint_cap` can be used for
//! endpoint / channel capabilities without going through Untyped allocation.
//!
//! When `bmm` gains physical-memory tracking and `task` lands, the stubs
//! here will be replaced with real Untyped → typed conversions.

use super::error::CapError;
use super::{CapRights, CapType, Capability};
use crate::kernel::ipc::message::ProcessId;

// ---------------------------------------------------------------------------
// Retype error
// ---------------------------------------------------------------------------

/// Errors specific to the `retype` operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetypeError {
    /// The source capability is not Untyped.
    NotUntyped,
    /// The untyped region is too small for the requested type.
    TooSmall,
    /// The untyped region is not aligned to the requested type's requirements.
    BadAlignment,
    /// The untyped region has already been fully consumed.
    Exhausted,
    /// The target type does not consume unt
    /// yped memory (e.g. CNode).
    InvalidTargetType,
}

// ---------------------------------------------------------------------------
// Size constants for kernel objects (bytes)
// ---------------------------------------------------------------------------

/// Minimum size required to retype each kernel-object type.
pub mod obj_size {
    pub const ENDPOINT:    usize = core::mem::size_of::<crate::kernel::ipc::channel::Channel>();
    pub const THREAD:      usize = 4096; // TCB + kernel stack
    pub const PAGE_TABLE:  usize = 4096; // one page-table page
    pub const FRAME:       usize = 4096; // typical base page
    pub const NOTIFICATION: usize = 64;
    pub const CNODE:       usize = 4096; // one CNode table page
}

// ---------------------------------------------------------------------------
// Placeholder retype
// ---------------------------------------------------------------------------

/// Retype a portion of an Untyped capability into a typed capability.
///
/// # Arguments
///
/// * `untyped` — the Untyped capability to consume from.
/// * `target_type` — what kind of object to create.
/// * `rights` — initial rights for the new capability.
/// * `owner` — process that will receive the new capability.
///
/// # Placeholder
///
/// Currently returns `Err(RetypeError::NotUntyped)` for all calls.
/// The real implementation will:
///
/// 1. Verify `untyped.cap_type == CapType::Untyped`.
/// 2. Check the untyped region has enough remaining space.
/// 3. Carve out the requested portion.
/// 4. Create the kernel object (allocate a Channel, Thread, …).
/// 5. Call `derive::mint_cap()` to create the resulting capability.
pub fn retype(
    _untyped: &Capability,
    _target_type: CapType,
    _rights: CapRights,
    _owner: ProcessId,
) -> Result<Capability, RetypeError> {
    // TODO: when bmm / physical memory tracking lands:
    //
    // if _untyped.cap_type != CapType::Untyped {
    //     return Err(RetypeError::NotUntyped);
    // }
    //
    // let size = obj_size_for(_target_type);
    // let base = carve_untyped(_untyped, size)?;
    // let obj_id = allocate_kernel_object(_target_type, base, size);
    // let cap = derive::mint_cap(obj_id, _target_type, _rights, _owner);
    // Ok(cap)

    log::warn!("cap::allocator::retype: not yet implemented (placeholder)");
    Err(RetypeError::NotUntyped)
}

/// Create a bootstrap Untyped capability covering a physical memory region.
///
/// Called during kernel initialisation to hand all free memory to the
/// root server as Untyped capabilities.
///
/// # Placeholder
///
/// Returns a capability with `NotImplemented` error for now.
pub fn create_untyped(
    _base_paddr: usize,
    _size_bytes: usize,
    _owner: ProcessId,
) -> Result<Capability, CapError> {
    // TODO: carve out an Untyped capability from the boot memory map.
    //
    // let cap = derive::mint_cap(base_paddr, CapType::Untyped, CapRights::full(), owner);
    // Ok(cap)

    log::warn!("cap::allocator::create_untyped: not yet implemented (placeholder)");
    Err(CapError::NotImplemented)
}
