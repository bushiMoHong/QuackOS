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
use aarch64::base::mm::page_table::PageTableEntry;
use aarch64::base::mm::VirtAddr;
use aarch64::base::mm::VirtPageNum;
use aarch64::base::mm::PhysAddr;
use aarch64::base::mm::PhysPageNum;
use aarch64::base::mm::{alloc_page, free_page};
use aarch64::base::config::PAGE_SIZE;
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

// ---------------------------------------------------------------------------
// Kernel-mapped PageTable factory
// ---------------------------------------------------------------------------

/// Create a fresh PageTable with kernel identity mappings already installed.
///
/// The new table contains:
/// - L0[0] → L1 table
/// - L1[0] → L2_LO table
/// - L1[1] = 1GB RAM block (identity maps PA 0x40000000-0x7FFFFFFF)
/// - L2_LO device blocks for UART (0x09000000) and VirtIO (0x0A000000)
///
/// User-space pages (L2_LO[0]-L2_LO[0x47]) start empty.
/// All intermediate pages are tracked via `PageTable::track_frame` and
/// will be freed when the PageTable is dropped.
pub fn create_kernel_mapped_page_table() -> Result<PageTable, MapError> {
    let mut pt = PageTable::new();

    // Allocate L1 table
    let l1_pa = alloc_page().ok_or(MapError::OutOfMemory)?;
    let mut l1_ppn = PhysPageNum::from(l1_pa >> 12);
    unsafe { core::ptr::write_bytes((l1_pa) as *mut u8, 0, PAGE_SIZE); }
    pt.track_frame(l1_ppn);

    // L0[0] → table descriptor for L1
    let mut root_ppn = pt.root_ppn;
    let root_table = unsafe { root_ppn.get_pte_array_mut() };
    root_table[0] = PageTableEntry::new_table(l1_ppn);

    // Allocate L2_LO table
    let l2_lo_pa = alloc_page().ok_or(MapError::OutOfMemory)?;
    let mut l2_lo_ppn = PhysPageNum::from(l2_lo_pa >> 12);
    unsafe { core::ptr::write_bytes((l2_lo_pa) as *mut u8, 0, PAGE_SIZE); }
    pt.track_frame(l2_lo_ppn);

    // L1[0] → table descriptor for L2_LO
    let l1_table = unsafe { l1_ppn.get_pte_array_mut() };
    l1_table[0] = PageTableEntry::new_table(l2_lo_ppn);

    // L1[1] = 1GB RAM block (identity map 0x40000000-0x7FFFFFFF)
    l1_table[1] = PageTableEntry {
        bits: (0x40000000usize)
            | (2 << 2)     // AttrIndx 2 = normal WB
            | (1 << 5)     // NS
            | (0b11 << 8)  // inner shareable
            | (0b00 << 6)  // AP = EL1 RW, EL0 RW
            | (1 << 10)    // AF
            | (1 << 54)    // UXN
            | 0b01,        // block, valid
    };

    // Copy device block entries from kernel L2_LO
    let kernel_l2_lo_pa = *crate::KERNEL_L2_LOW_PA.lock();
    let kernel_l2_lo = unsafe {
        &*(kernel_l2_lo_pa as *const [u64; 512])
    };
    let our_l2_lo = unsafe { l2_lo_ppn.get_pte_array_mut() };

    // Copy the exact PTE values from the kernel table
    our_l2_lo[0x48] = PageTableEntry { bits: kernel_l2_lo[0x48] as usize };
    our_l2_lo[0x50] = PageTableEntry { bits: kernel_l2_lo[0x50] as usize };

    Ok(pt)
}

// ---------------------------------------------------------------------------
// Clone user-space mappings from one PageTable to another
// ---------------------------------------------------------------------------

/// Copy every mapped user page from `src` to `dst` with fresh physical pages.
///
/// Walks L2_LO[0..0x48) (VA 0x0 through 0x8F_FFFF), and for each valid L3
/// page or L2 block, allocates a new physical page, copies the data, and maps
/// it at the same VA with the same permission flags.
///
/// Kernel identity mappings (L1[1]) and device blocks (L2_LO[0x48], [0x50])
/// are left untouched — `dst` is assumed to already contain them (i.e. was
/// created via `create_kernel_mapped_page_table`).
pub fn clone_user_mappings(
    src: &PageTable,
    dst: &mut PageTable,
) -> Result<(), MapError> {
    // Read src L2_LO table: L0[0] → L1 → L1[0] → L2_LO
    let l2_lo_ppn = get_l2_lo_ppn(src).ok_or(MapError::InvalidArgument)?;

    for l2_idx in 0..0x48usize {
        let l2_bits = unsafe { read_pte_raw(l2_lo_ppn, l2_idx) };
        if l2_bits & 1 == 0 {
            continue; // not valid
        }

        let is_table = (l2_bits >> 1) & 1 == 1;

        if is_table {
            let l3_ppn = PhysPageNum::from((l2_bits >> 12) & ((1usize << 36) - 1));
            for l3_idx in 0..512 {
                let l3_bits = unsafe { read_pte_raw(l3_ppn, l3_idx) };
                if l3_bits & 1 == 0 {
                    continue;
                }

                let src_pa = (l3_bits >> 12) << 12; // output address (bits [47:12])
                let dst_pa = alloc_page().ok_or(MapError::OutOfMemory)?;
                unsafe {
                    core::ptr::copy_nonoverlapping(
                        (src_pa & 0x0000_FFFF_FFFF_F000) as *const u8,
                        dst_pa as *mut u8,
                        PAGE_SIZE,
                    );
                }

                let va = (l2_idx << 21) | (l3_idx << 12);
                let vpn = VirtPageNum::from(va >> 12);
                let ppn = PhysPageNum::from(dst_pa >> 12);

                // Reconstruct PTEFlags from the source L3 PTE bits.
                let pte = PageTableEntry { bits: l3_bits };
                dst.map(vpn, ppn, pte.flags());
            }
        } else {
            // L2 block entry (2 MiB).  Rare with the current loader, but handle
            // it for correctness.  Allocate 512 × 4 KiB pages independently so
            // the destination stays page-granular for later CoW or mprotect.
            let src_base = (l2_bits >> 12) << 12;
            let block_pte = PageTableEntry { bits: l2_bits };
            let flags = block_pte.flags();

            for i in 0..512 {
                let dst_pa = alloc_page().ok_or(MapError::OutOfMemory)?;
                let offset = i * PAGE_SIZE;
                unsafe {
                    core::ptr::copy_nonoverlapping(
                        ((src_base & 0x0000_FFFF_FFFF_F000) + offset) as *const u8,
                        dst_pa as *mut u8,
                        PAGE_SIZE,
                    );
                }
                let va = (l2_idx << 21) | (i << 12);
                let vpn = VirtPageNum::from(va >> 12);
                let ppn = PhysPageNum::from(dst_pa >> 12);
                dst.map(vpn, ppn, flags);
            }
        }
    }

    Ok(())
}

/// Read a raw u64 PTE from a page table at the given index.
#[inline]
unsafe fn read_pte_raw(ppn: PhysPageNum, idx: usize) -> usize {
    let pa = PhysAddr::from(ppn).0;
    let table = &*(pa as *const [usize; 512]);
    table[idx]
}

/// Walk L0[0] → L1 → L1[0] → L2_LO and return the PhysPageNum of the L2_LO table.
fn get_l2_lo_ppn(pt: &PageTable) -> Option<PhysPageNum> {
    let l0_bits = unsafe { read_pte_raw(pt.root_ppn, 0) };
    if l0_bits & 1 == 0 {
        return None;
    }
    let l1_ppn = PhysPageNum::from((l0_bits >> 12) & ((1usize << 36) - 1));

    let l1_bits = unsafe { read_pte_raw(l1_ppn, 0) };
    if l1_bits & 1 == 0 {
        return None;
    }
    let l2_ppn = PhysPageNum::from((l1_bits >> 12) & ((1usize << 36) - 1));

    Some(l2_ppn)
}

// ---------------------------------------------------------------------------
// Global address-space table
// ---------------------------------------------------------------------------

const MAX_AS: usize = 128;

struct AsEntry {
    id: AddressSpaceId,
    page_table: PageTable,
    generation: u16,
}

struct AsTable {
    slots: [Option<AsEntry>; MAX_AS],
    count: usize,
    generations: [u16; MAX_AS],
}

impl AsTable {
    const fn new() -> Self {
        AsTable {
            slots: [const { None }; MAX_AS],
            count: 0,
            generations: [0; MAX_AS],
        }
    }

    fn alloc(&mut self, pt: PageTable) -> Option<AddressSpaceId> {
        for (i, slot) in self.slots.iter_mut().enumerate() {
            if slot.is_none() {
                let gen = if self.generations[i] == 0 { 1 }
                          else { self.generations[i].wrapping_add(1) };
                self.generations[i] = gen;
                let id = AddressSpaceId(((gen as usize) << 16) | i);
                *slot = Some(AsEntry { id, page_table: pt, generation: gen });
                self.count += 1;
                return Some(id);
            }
        }
        None
    }

    fn free(&mut self, id: AddressSpaceId) -> Option<PageTable> {
        let idx = id.0 & 0xFFFF;
        if idx >= MAX_AS { return None; }
        let entry = self.slots[idx].take()?;
        if entry.id != id {
            self.slots[idx] = Some(entry);
            return None;
        }
        self.count -= 1;
        Some(entry.page_table)
    }

    fn get_mut(&mut self, id: AddressSpaceId) -> Option<&mut AsEntry> {
        let idx = id.0 & 0xFFFF;
        let slot = self.slots.get_mut(idx)?;
        slot.as_mut().filter(|e| e.id == id)
    }
}

pub static AS_TABLE: Mutex<AsTable> = Mutex::new(AsTable::new());

/// Allocate a new address space with kernel identity mappings.
pub fn create_address_space() -> Result<AddressSpaceId, MapError> {
    let pt = create_kernel_mapped_page_table()?;
    AS_TABLE.lock().alloc(pt).ok_or(MapError::OutOfMemory)
}

/// Register a pre-built PageTable into the AS table.
///
/// Returns the newly allocated `AddressSpaceId` together with the TTBR0 token.
/// The PageTable is moved into the table and must not be used afterwards.
pub fn register_page_table(pt: PageTable) -> Option<(AddressSpaceId, usize)> {
    let mut table = AS_TABLE.lock();
    let id = table.alloc(pt)?;
    let ttbr0 = table.get_mut(id).unwrap().page_table.token();
    Some((id, ttbr0))
}

/// Remove an address space from the table, freeing its page-table pages.
/// Does nothing if the ASID does not exist.
pub fn unregister_address_space(id: AddressSpaceId) {
    let mut table = AS_TABLE.lock();
    table.free(id);
}

/// Destroy an address space, freeing all its page-table pages.
pub fn destroy_address_space(id: AddressSpaceId) -> Result<(), MapError> {
    let mut table = AS_TABLE.lock();
    let _pt = table.free(id).ok_or(MapError::InvalidArgument)?;
    // PageTable Drop frees all tracked pages
    Ok(())
}

/// Get the TTBR0 token (physical page-table root) for `id`.
pub fn get_ttbr0(id: AddressSpaceId) -> Option<usize> {
    let mut table = AS_TABLE.lock();
    table.get_mut(id).map(|e| e.page_table.token())
}

/// Execute a closure with mutable access to an address space's PageTable.
pub fn with_page_table_mut<R>(
    id: AddressSpaceId,
    f: impl FnOnce(&mut PageTable) -> R,
) -> Option<R> {
    let mut table = AS_TABLE.lock();
    let entry = table.get_mut(id)?;
    Some(f(&mut entry.page_table))
}

/// Map a page into a specific address space (allocating a physical page).
/// Similar to sys_map_page but targets a specific AS.
pub fn map_page_in_as(
    asid: AddressSpaceId,
    vaddr: usize,
    paddr: usize,
    prot: u16,
) -> Result<(), MapError> {
    let va = VirtAddr::from(vaddr);
    let vpn = VirtPageNum::from(va.floor());
    let ppn = PhysPageNum::from(PhysAddr::from(paddr));

    let mut flags = crate::kernel::bmm::MapFlags::empty();
    flags.0 |= MapFlags::USER;
    if prot & 1 != 0 { flags.0 |= MapFlags::READ; }
    if prot & 2 != 0 { flags.0 |= MapFlags::WRITE; }
    if prot & 4 != 0 { flags.0 |= MapFlags::EXEC; }

    let pte_flags = flags.to_pte_flags();
    let mut table = AS_TABLE.lock();
    let entry = table.get_mut(asid).ok_or(MapError::InvalidArgument)?;
    entry.page_table.map(vpn, ppn, pte_flags);
    Ok(())
}
