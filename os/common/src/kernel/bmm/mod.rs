//! Basic Memory Manager (bmm) — microkernel-style page-table engine.
//!
//! # Responsibility
//!
//! `bmm` is the **only** kernel component that directly manipulates hardware
//! page tables.  It provides three groups of functionality:
//!
//! | Group              | Purpose                                         |
//! |--------------------|-------------------------------------------------|
//! | Address-space ops  | Create / destroy / switch address spaces        |
//! | Mapping primitives | `map`, `unmap`, `grant` — called by usermode mm |
//! | Page-fault catch   | Catch hardware #PF, package as IPC to usermode  |
//! | TLB maintenance    | Invalidate after every page-table mutation      |
//!
//! # Design (microkernel pattern)
//!
//! `bmm` does **not** allocate physical memory or decide policy.
//! When a page fault occurs it:
//!
//! 1. Records the faulting address and cause.
//! 2. Packages them into an `IpcPageFault` message.
//! 3. Sends the message to the user-space mm server (via the IPC subsystem).
//!
//! The user-space mm server then decides what to do and calls back into
//! the kernel via `map` / `unmap` / `grant` syscalls.

use aarch64::base::mm::page_table::PageTable;
use aarch64::base::mm::page_table::{tlb_invalidate, tlb_invalidate_addr};
use aarch64::base::mm::page_table::PTEFlags;
use aarch64::base::mm::VirtAddr;
use aarch64::base::mm::VirtPageNum;
use aarch64::base::mm::PhysAddr;
use aarch64::base::mm::PhysPageNum;
use aarch64::base::mm::free_page;
use spin::Mutex;

use crate::kernel::trap::PageFaultCause;
pub use crate::kernel::ipc::{IpcPageFault, MapRequest, GrantRequest, UnmapRequest};

// ---------------------------------------------------------------------------
// AddressSpace
// ---------------------------------------------------------------------------

/// Unique identifier for an address space.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct AddressSpaceId(pub usize);

/// A virtual address space, backed by an AArch64 page table.
pub struct AddressSpace {
    pub id: AddressSpaceId,
    pub page_table: PageTable,
}

impl AddressSpace {
    /// Create a new, empty address space with a freshly allocated root table.
    pub fn new(id: AddressSpaceId) -> Self {
        AddressSpace {
            id,
            page_table: PageTable::new(),
        }
    }

    /// Reconstruct an address space handle from a raw TTBR value.
    ///
    /// The returned handle does **not** own the root table — use this only
    /// for operations like page-fault handling where the address space is
    /// known to outlive the handle.
    pub fn from_token(id: AddressSpaceId, ttbr: usize) -> Self {
        AddressSpace {
            id,
            page_table: PageTable::from_token(ttbr),
        }
    }

    /// Return the hardware page-table root token (TTBR0_EL1 / TTBR1_EL1 value).
    pub fn token(&self) -> usize {
        self.page_table.token()
    }
}

// ---------------------------------------------------------------------------
// Map flags
// ---------------------------------------------------------------------------

/// Permission and caching flags for a virtual-memory mapping.
///
/// These map directly to `PTEFlags` but use friendlier names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MapFlags(pub(crate) u16);

impl MapFlags {
    pub const READ:    u16 = PTEFlags::R;
    pub const WRITE:   u16 = PTEFlags::W;
    pub const EXEC:    u16 = PTEFlags::X;
    pub const USER:    u16 = PTEFlags::U;
    pub const GLOBAL:  u16 = PTEFlags::G;
    pub const COW:     u16 = PTEFlags::COW;
    pub const SHARED:  u16 = PTEFlags::S;

    pub const RW:   u16 = Self::READ | Self::WRITE;
    pub const RWX:  u16 = Self::READ | Self::WRITE | Self::EXEC;
    pub const RX:   u16 = Self::READ | Self::EXEC;

    pub const fn empty() -> Self { MapFlags(0) }

    pub fn contains(&self, f: u16) -> bool { self.0 & f != 0 }

    pub fn to_pte_flags(&self) -> PTEFlags {
        let mut f = PTEFlags { bits: self.0 };
        f.insert(PTEFlags::V);
        f.insert(PTEFlags::A);
        f.insert(PTEFlags::D);
        f
    }
}

// ---------------------------------------------------------------------------
// Mapping primitives — called by user-space mm via syscalls
// ---------------------------------------------------------------------------

/// Map a virtual page to a physical frame in `addr_space`.
///
/// # Arguments
/// * `addr_space` — target address space.
/// * `vaddr`      — virtual address (will be page-aligned).
/// * `paddr`      — physical address of the frame to map.
/// * `flags`      — access permissions.
///
/// # Returns
/// `Ok(())` on success, or an error code.
pub fn map(
    addr_space: &mut AddressSpace,
    vaddr: usize,
    paddr: usize,
    flags: MapFlags,
) -> Result<(), MapError> {
    let va = VirtAddr::from(vaddr);
    let vpn = VirtPageNum::from(va.floor());
    let ppn = PhysPageNum::from(PhysAddr::from(paddr));

    // Reject double-mapping
    if addr_space.page_table.translate_vpn_to_pte(vpn).is_some_and(|p| p.is_valid()) {
        return Err(MapError::AlreadyMapped);
    }

    addr_space.page_table.map(vpn, ppn, flags.to_pte_flags());
    tlb_invalidate_addr(va);
    Ok(())
}

/// Unmap a virtual page in `addr_space`.
///
/// The physical frame is **not** freed — that decision belongs to the
/// user-space mm server.  Use `unmap_and_free()` to release the frame.
pub fn unmap(addr_space: &mut AddressSpace, vaddr: usize) -> Result<usize, MapError> {
    let va = VirtAddr::from(vaddr);
    let vpn = VirtPageNum::from(va.floor());

    let pte = addr_space
        .page_table
        .translate_vpn_to_pte(vpn)
        .ok_or(MapError::NotMapped)?;

    if !pte.is_valid() {
        return Err(MapError::NotMapped);
    }

    let paddr = PhysAddr::from(PhysPageNum::from(pte.ppn().0)).0;

    addr_space.page_table.unmap(vpn);
    tlb_invalidate_addr(va);
    Ok(paddr)
}

/// Unmap and immediately free the backing physical page.
pub fn unmap_and_free(addr_space: &mut AddressSpace, vaddr: usize) -> Result<(), MapError> {
    let paddr = unmap(addr_space, vaddr)?;
    free_page(paddr);
    Ok(())
}

/// Grant (transfer) a physical frame from one address space to another.
///
/// Unmaps `vaddr` in `src` and maps the same physical frame at `vaddr`
/// in `dst` with the given flags.
///
/// This is the kernel-side implementation of the microkernel `Grant` IPC.
pub fn grant(
    src: &mut AddressSpace,
    dst: &mut AddressSpace,
    vaddr: usize,
    flags: MapFlags,
) -> Result<(), MapError> {
    let paddr = unmap(src, vaddr)?;
    map(dst, vaddr, paddr, flags)
}

/// Remap (change permissions on) an already-mapped page.
pub fn remap(addr_space: &mut AddressSpace, vaddr: usize, flags: MapFlags) -> Result<(), MapError> {
    let va = VirtAddr::from(vaddr);
    let vpn = VirtPageNum::from(va.floor());

    if !addr_space.page_table.translate_vpn_to_pte(vpn).is_some_and(|p| p.is_valid()) {
        return Err(MapError::NotMapped);
    }

    addr_space.page_table.remap(vpn, flags.to_pte_flags());
    tlb_invalidate_addr(va);
    Ok(())
}

// ---------------------------------------------------------------------------
// Page-fault handling
// ---------------------------------------------------------------------------

/// Global fault queue — in a real microkernel this would be a channel to
/// the user-space mm server.  For now we use a fixed-size ring buffer.
static FAULT_QUEUE: Mutex<FaultQueue> = Mutex::new(FaultQueue::new());

const FAULT_QUEUE_CAPACITY: usize = 64;

struct FaultQueue {
    items: [Option<IpcPageFault>; FAULT_QUEUE_CAPACITY],
    head: usize,
    tail: usize,
    count: usize,
}

impl FaultQueue {
    const fn new() -> Self {
        FaultQueue {
            items: [const { None }; FAULT_QUEUE_CAPACITY],
            head: 0,
            tail: 0,
            count: 0,
        }
    }

    fn push(&mut self, fault: IpcPageFault) -> Result<(), IpcPageFault> {
        if self.count >= FAULT_QUEUE_CAPACITY {
            return Err(fault);
        }
        self.items[self.tail] = Some(fault);
        self.tail = (self.tail + 1) % FAULT_QUEUE_CAPACITY;
        self.count += 1;
        Ok(())
    }

    fn pop(&mut self) -> Option<IpcPageFault> {
        if self.count == 0 {
            return None;
        }
        let fault = self.items[self.head].take();
        self.head = (self.head + 1) % FAULT_QUEUE_CAPACITY;
        self.count -= 1;
        fault
    }
}

/// Called from the arch trap handler when a page fault occurs.
///
/// Packages the fault into an `IpcPageFault` and queues it for delivery
/// to the user-space mm server.  Does **not** allocate memory — that is
/// the mm server's responsibility.
///
/// Returns `true` if the fault was successfully queued, `false` if the
/// queue is full (which indicates the mm server is overloaded / dead).
pub fn handle_page_fault(
    addr_space_id: AddressSpaceId,
    fault_vaddr: usize,
    cause: PageFaultCause,
) -> bool {
    let fault = IpcPageFault {
        addr_space_id,
        fault_vaddr,
        cause,
    };
    FAULT_QUEUE.lock().push(fault).is_ok()
}

/// Dequeue the oldest pending page fault.
///
/// Called by the IPC delivery path to forward faults to the mm server.
pub fn dequeue_fault() -> Option<IpcPageFault> {
    FAULT_QUEUE.lock().pop()
}

/// Return the number of pending page faults.
pub fn pending_fault_count() -> usize {
    FAULT_QUEUE.lock().count
}

// ---------------------------------------------------------------------------
// TLB maintenance
// ---------------------------------------------------------------------------

/// Invalidate the entire TLB (all ASIDs).
pub fn tlb_flush_all() {
    tlb_invalidate();
}

/// Invalidate a single virtual address in the TLB.
pub fn tlb_flush_addr(vaddr: usize) {
    tlb_invalidate_addr(VirtAddr::from(vaddr));
}

// ---------------------------------------------------------------------------
// Helper — translate a VA in the given address space
// ---------------------------------------------------------------------------

/// Translate a virtual address to a physical address in the given space.
pub fn translate(addr_space: &AddressSpace, vaddr: usize) -> Option<usize> {
    addr_space.page_table.translate_va_to_pa(VirtAddr::from(vaddr))
}

/// Return true if `vaddr` is currently mapped in `addr_space`.
pub fn is_mapped(addr_space: &AddressSpace, vaddr: usize) -> bool {
    let vpn = VirtPageNum::from(VirtAddr::from(vaddr).floor());
    addr_space.page_table.translate_vpn_to_pte(vpn)
        .is_some_and(|p| p.is_valid())
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MapError {
    /// The virtual address is already mapped.
    AlreadyMapped,
    /// The virtual address is not mapped.
    NotMapped,
    /// Out of physical memory (page-table allocation failed).
    OutOfMemory,
    /// Invalid argument (e.g., null pointer, bad alignment).
    InvalidArgument,
}
