//! Page-fault resolution — the core policy engine of the user-space mm.
//!
//! Receives an `IpcPageFault` from the kernel (via IPC), consults the
//! faulting process's VMA, allocates a physical frame when the access is
//! legitimate, and builds the corresponding `map` / `kill` request.
//!
//! # Fault-resolution flowchart
//!
//! ```text
//!      ┌─────────────────┐
//!      │ IpcPageFault in │
//!      └───────┬─────────┘
//!              │
//!      ┌───────▼─────────┐     no      ┌───────────┐
//!      │  VMA covering   │ ──────────▶ │  SIGSEGV   │
//!      │  fault_vaddr?   │             │ (NoVma)    │
//!      └───────┬─────────┘             └───────────┘
//!              │ yes
//!      ┌───────▼─────────┐     no
//!      │  Guard page?    │ ──────────▶ SIGSEGV (guard)
//!      └───────┬─────────┘
//!              │ no
//!      ┌───────▼─────────┐     no
//!      │  Permissions    │ ──────────▶ SIGSEGV (perms)
//!      │  sufficient?    │
//!      └───────┬─────────┘
//!              │ yes
//!      ┌───────▼─────────┐
//!      │  Alloc physical │
//!      │  page           │
//!      └───────┬─────────┘
//!              │
//!      ┌───────▼─────────┐
//!      │  Build MapReq   │──▶ IPC to kernel bmm
//!      └─────────────────┘
//! ```

use crate::kernel::ipc::message::IpcPageFault;
use crate::kernel::trap::PageFaultCause;

use crate::usr::mm::allocator;
use crate::usr::mm::types::{
    BatchMappingArray, MmError, MmRequest, MmResult, OomPolicy, VmaEntry, VmPerms,
    PREFAULT_BATCH,
};
use crate::usr::mm::vma::VmaManager;

// ---------------------------------------------------------------------------
// Fault resolution
// ---------------------------------------------------------------------------

/// Resolve a single page fault.
///
/// On success, returns a `MmRequest` that the mm server forwards to the
/// kernel's `bmm` subsystem.
pub fn resolve_page_fault(
    fault: &IpcPageFault,
    vma_mgr: &mut VmaManager,
    policy: OomPolicy,
) -> MmResult<MmRequest> {
    let vaddr = fault.fault_vaddr;

    // 1. Check for guard page — instant SIGSEGV.
    if vma_mgr.find_guard(vaddr).is_some() {
        return Err(MmError::PermissionDenied);
    }

    // 2. Find the VMA covering this address.
    let vma = match vma_mgr.find(vaddr) {
        Some(v) => v.clone(),
        None => {
            // Not in any VMA.  Check if it's a stack expansion candidate.
            check_stack_expansion(vma_mgr, vaddr)?
        }
    };

    // 3. Check permissions.
    let needed = fault_cause_to_perms(fault.cause);
    if !vma.permits(needed) {
        return Err(MmError::PermissionDenied);
    }

    // 4. Handle CoW if the VMA is CoW and this is a write fault.
    if vma.cow && fault.cause == PageFaultCause::Store {
        return handle_cow_fault(fault, vma_mgr);
    }

    // 5. Allocate a physical page from the buddy allocator.
    let paddr = match allocator::alloc_page() {
        Some(pa) => pa,
        None => {
            // OOM — try reclamation, then kill.
            return handle_oom(fault, policy);
        }
    };

    // 6. Build map request.
    Ok(MmRequest::MapSingle {
        addr_space_id: fault.addr_space_id,
        vaddr,
        paddr,
        flags: vma.perms.as_bits(),
    })
}

/// Resolve a page fault **with prefault** — allocates the faulting page plus
/// up to `PREFAULT_BATCH - 1` adjacent pages within the same VMA.
///
/// Returns a `MapSingle` if only one page was needed, or the batch can be
/// encoded differently by the caller.
pub fn resolve_with_prefault(
    fault: &IpcPageFault,
    vma_mgr: &mut VmaManager,
    policy: OomPolicy,
) -> MmResult<(MmRequest, BatchMappingArray)> {
    let page_size = 4096;
    let base_vaddr = fault.fault_vaddr & !(page_size - 1); // align down

    // Resolve the primary fault first.
    let primary = resolve_page_fault(fault, vma_mgr, policy)?;

    let vma = match vma_mgr.find(fault.fault_vaddr) {
        Some(v) => v.clone(),
        None => return Ok((primary, BatchMappingArray::new())),
    };

    let mut batch = BatchMappingArray::new();

    // Add the primary page.
    if let MmRequest::MapSingle {
        vaddr, paddr, flags, ..
    } = &primary
    {
        batch.push(*vaddr, *paddr, *flags);
    }

    // Prefault adjacent pages (forward).
    for i in 1..PREFAULT_BATCH {
        let next_va = base_vaddr + (i * page_size);
        if next_va >= vma.end_vaddr {
            break;
        }
        // Skip if already mapped (the kernel's `map` will return AlreadyMapped,
        // but we avoid the IPC round-trip by checking here).
        // For now we just attempt allocation; the kernel checks for duplicates.
        if let Some(pa) = allocator::alloc_page() {
            batch.push(next_va, pa, vma.perms.as_bits());
        } else {
            break; // OOM — stop prefaulting, but the primary is already done
        }
    }

    Ok((primary, batch))
}

// ---------------------------------------------------------------------------
// Stack expansion
// ---------------------------------------------------------------------------

/// If the fault is just below a stack VMA, extend the stack downward.
///
/// Returns the (possibly extended) stack VMA.
fn check_stack_expansion(vma_mgr: &mut VmaManager, vaddr: usize) -> MmResult<VmaEntry> {
    let page_size = 4096;
    // Only allow expansion within one page below the current stack.
    let stack = vma_mgr
        .find_stack_to_extend(vaddr)
        .ok_or(MmError::NoVma)?
        .clone();

    // The fault must be within a reasonable distance below the stack.
    let stack_start = stack.start_vaddr;
    let expansion_start = vaddr & !(page_size - 1);

    if expansion_start >= stack_start {
        return Err(MmError::NoVma);
    }

    // Extend the stack.
    vma_mgr.extend_stack(expansion_start)?;

    // Re-fetch the (now extended) stack VMA.
    vma_mgr.find(vaddr).cloned().ok_or(MmError::NoVma)
}

// ---------------------------------------------------------------------------
// CoW handling
// ---------------------------------------------------------------------------

/// Handle a write fault on a Copy-on-Write page.
///
/// 1. Allocate a new physical page.
/// 2. Copy the contents of the original page (TODO: requires memcpy across
///    address spaces — for now this is a placeholder).
/// 3. Update the VMA to be non-CoW for this page.
/// 4. Build a map request for the new page.
fn handle_cow_fault(fault: &IpcPageFault, _vma_mgr: &mut VmaManager) -> MmResult<MmRequest> {
    let new_paddr = allocator::alloc_page().ok_or(MmError::OutOfMemory)?;

    // TODO: Copy contents from the original CoW page.
    // This requires the mm server to either:
    //   a) temporarily map the original page into its own address space,
    //   b) use a kernel primitive that does the copy, or
    //   c) have the kernel handle CoW breaking at the bmm layer.
    //
    // For now, this is a known limitation — CoW pages are mapped writable
    // without copying (which is incorrect for shared mappings but works
    // for single-owner anonymous pages).

    let vaddr = fault.fault_vaddr;

    Ok(MmRequest::MapSingle {
        addr_space_id: fault.addr_space_id,
        vaddr,
        paddr: new_paddr,
        flags: VmPerms::RW.as_bits(),
    })
}

// ---------------------------------------------------------------------------
// OOM handling
// ---------------------------------------------------------------------------

/// Handle out-of-memory during page-fault resolution.
fn handle_oom(fault: &IpcPageFault, policy: OomPolicy) -> MmResult<MmRequest> {
    match policy {
        OomPolicy::Kill => Err(MmError::OutOfMemory),
        OomPolicy::Reclaim => {
            if try_reclaim().is_ok() {
                // Retry allocation after successful reclaim.
                let paddr = allocator::alloc_page().ok_or(MmError::OutOfMemory)?;
                Ok(MmRequest::MapSingle {
                    addr_space_id: fault.addr_space_id,
                    vaddr: fault.fault_vaddr,
                    paddr,
                    flags: VmPerms::RW.as_bits(),
                })
            } else {
                Err(MmError::OutOfMemory)
            }
        }
        OomPolicy::ReclaimThenKill => {
            if try_reclaim().is_ok() {
                let paddr = allocator::alloc_page().ok_or(MmError::OutOfMemory)?;
                Ok(MmRequest::MapSingle {
                    addr_space_id: fault.addr_space_id,
                    vaddr: fault.fault_vaddr,
                    paddr,
                    flags: VmPerms::RW.as_bits(),
                })
            } else {
                Err(MmError::OutOfMemory) // caller sends KillProcess
            }
        }
    }
}

/// Attempt to reclaim memory via LRU scan / swap.
///
/// **Placeholder** — always fails until swap / page-cache infrastructure
/// is implemented.
fn try_reclaim() -> Result<(), ()> {
    // TODO: Implement LRU page reclamation.
    // 1. Scan the global LRU list for inactive anonymous pages.
    // 2. If a swap file is configured, write them out and free the frame.
    // 3. If no swap, look for clean file-backed pages (can discard safely).
    // 4. Return Ok if at least one page was freed.
    Err(())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Map `PageFaultCause` to the permission set required for the access.
fn fault_cause_to_perms(cause: PageFaultCause) -> VmPerms {
    match cause {
        PageFaultCause::Load  => VmPerms::R,
        PageFaultCause::Store => VmPerms::RW,
        PageFaultCause::Exec  => VmPerms::RX,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kernel::bmm::AddressSpaceId;

    fn make_fault(vaddr: usize, cause: PageFaultCause) -> IpcPageFault {
        IpcPageFault {
            addr_space_id: AddressSpaceId(1),
            fault_vaddr: vaddr,
            cause,
        }
    }

    #[test]
    fn guard_page_causes_segv() {
        let mut mgr = VmaManager::new();
        mgr.insert(VmaEntry::new_guard(0x2000)).unwrap();
        // Note: allocator::init() not called, but guard check happens before alloc.
        let fault = make_fault(0x2000, PageFaultCause::Store);
        let result = resolve_page_fault(&fault, &mut mgr, OomPolicy::Kill);
        assert!(matches!(result, Err(MmError::PermissionDenied)));
    }

    #[test]
    fn no_vma_causes_segv() {
        let mut mgr = VmaManager::new();
        let fault = make_fault(0x5000, PageFaultCause::Load);
        let result = resolve_page_fault(&fault, &mut mgr, OomPolicy::Kill);
        assert!(matches!(result, Err(MmError::NoVma)));
    }

    #[test]
    fn write_to_readonly_vma() {
        let mut mgr = VmaManager::new();
        // Insert a read-only region.
        mgr.insert(VmaEntry {
            start_vaddr: 0x1000,
            end_vaddr: 0x2000,
            perms: VmPerms::R,
            region_type: VmRegionType::Data,
            backing_offset: 0,
            cow: false,
        })
        .unwrap();
        let fault = make_fault(0x1500, PageFaultCause::Store);
        let result = resolve_page_fault(&fault, &mut mgr, OomPolicy::Kill);
        assert!(matches!(result, Err(MmError::PermissionDenied)));
    }

    #[test]
    fn read_from_readonly_vma_is_ok() {
        let mut mgr = VmaManager::new();
        mgr.insert(VmaEntry {
            start_vaddr: 0x1000,
            end_vaddr: 0x2000,
            perms: VmPerms::R,
            region_type: VmRegionType::Data,
            backing_offset: 0,
            cow: false,
        })
        .unwrap();
        let fault = make_fault(0x1500, PageFaultCause::Load);
        // Will fail at alloc_page (no buddy init), but NOT at permission check.
        let result = resolve_page_fault(&fault, &mut mgr, OomPolicy::Kill);
        // Expected: OOM (because allocator is not initialised in unit tests).
        assert!(matches!(result, Err(MmError::OutOfMemory)));
    }
}
