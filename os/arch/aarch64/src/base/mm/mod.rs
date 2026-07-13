//! AArch64 physical memory management.
//!
//! # Overview
//!
//! This module provides:
//!
//! | Item               | Purpose                                   |
//! |--------------------|-------------------------------------------|
//! | `PhysAddr`         | Physical address wrapper                  |
//! | `VirtAddr`         | Virtual address wrapper                   |
//! | `PhysPageNum`      | Physical page number                      |
//! | `VirtPageNum`      | Virtual page number                       |
//! | `FrameAllocator`   | Stack-based free-list page allocator      |
//! | `alloc_page()`     | Allocate one zeroed page → physical addr  |
//! | `free_page()`      | Return a page to the allocator            |
//! | `free_page_range()`| Add a contiguous range to the free list   |
//!
//! # Page table format (AArch64)
//!
//! AArch64 uses 4-level page tables with 48-bit VAs (4 × 9-bit index + 12-bit offset):
//!
//! ```text
//! [47:39]  [38:30]  [29:21]  [20:12]  [11:0]
//!   L0       L1       L2       L3      offset
//! ```
//!
//! Each level contains 512 descriptors. The root table address is held in
//! TTBR0_EL1 (user) or TTBR1_EL1 (kernel).

use super::config::{PAGE_OFFSET, PAGE_SHIFT, PAGE_SIZE};
use core::fmt;
use core::ptr;
use spin::Mutex;

pub mod page_table;

// ---------------------------------------------------------------------------
// Address types
// ---------------------------------------------------------------------------

/// Physical address.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord)]
#[repr(transparent)]
pub struct PhysAddr(pub usize);

/// Virtual address.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord)]
#[repr(transparent)]
pub struct VirtAddr(pub usize);

/// Physical page number.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord)]
#[repr(transparent)]
pub struct PhysPageNum(pub usize);

/// Virtual page number.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord)]
#[repr(transparent)]
pub struct VirtPageNum(pub usize);

// ---------------------------------------------------------------------------
// Conversions
// ---------------------------------------------------------------------------

impl From<usize> for PhysAddr {
    fn from(addr: usize) -> Self { PhysAddr(addr) }
}

impl From<usize> for VirtAddr {
    fn from(addr: usize) -> Self { VirtAddr(addr) }
}

impl From<usize> for PhysPageNum {
    fn from(n: usize) -> Self { PhysPageNum(n) }
}

impl From<usize> for VirtPageNum {
    fn from(n: usize) -> Self { VirtPageNum(n) }
}

impl From<PhysAddr> for PhysPageNum {
    fn from(addr: PhysAddr) -> Self { PhysPageNum(addr.0 >> PAGE_SHIFT) }
}

impl From<PhysPageNum> for PhysAddr {
    fn from(ppn: PhysPageNum) -> Self { PhysAddr(ppn.0 << PAGE_SHIFT) }
}

impl From<VirtAddr> for VirtPageNum {
    fn from(addr: VirtAddr) -> Self { VirtPageNum(addr.0 >> PAGE_SHIFT) }
}

impl From<VirtPageNum> for VirtAddr {
    fn from(vpn: VirtPageNum) -> Self { VirtAddr(vpn.0 << PAGE_SHIFT) }
}

impl VirtAddr {
    /// Round down to page-aligned.
    pub fn floor(&self) -> VirtAddr {
        VirtAddr(self.0 & !(PAGE_SIZE - 1))
    }

    /// Round up to page-aligned.
    pub fn ceil(&self) -> VirtAddr {
        VirtAddr((self.0 + PAGE_SIZE - 1) & !(PAGE_SIZE - 1))
    }

    /// Offset within the page.
    pub fn page_offset(&self) -> usize {
        self.0 & (PAGE_SIZE - 1)
    }

    /// Convert to physical address via identity mapping.
    ///
    /// With identity mapping enabled, virt == phys for all kernel addresses.
    pub fn to_phys(&self) -> Option<PhysAddr> {
        Some(PhysAddr(self.0))
    }
}

impl PhysPageNum {
    /// Return a shared reference to the page of PageTableEntry at this physical page.
    ///
    /// # Safety
    /// The physical page must be backed by real RAM.
    pub unsafe fn get_pte_array(&self) -> &[page_table::PageTableEntry; 512] {
        &*(PhysAddr::from(*self).to_kernel_virt().0 as *const [page_table::PageTableEntry; 512])
    }

    /// Return a mutable reference to the page of PageTableEntry at this physical page.
    ///
    /// # Safety
    /// The physical page must be backed by real RAM.
    pub unsafe fn get_pte_array_mut(&mut self) -> &mut [page_table::PageTableEntry; 512] {
        &mut *(PhysAddr::from(*self).to_kernel_virt().0 as *mut [page_table::PageTableEntry; 512])
    }
}

impl PhysAddr {
    /// Convert to kernel virtual address via identity mapping.
    pub fn to_kernel_virt(&self) -> VirtAddr {
        VirtAddr(self.0)
    }

    /// Return a mutable reference to the page-sized array of PTE descriptors.
    ///
    /// # Safety
    /// The physical address must be backed by real RAM.
    pub unsafe fn as_pte_array_mut(&self) -> &mut [usize; 512] {
        &mut *(self.to_kernel_virt().0 as *mut [usize; 512])
    }

    /// Return a shared reference to the page-sized array of PTE descriptors.
    ///
    /// # Safety
    /// The physical address must be backed by real RAM.
    pub unsafe fn as_pte_array(&self) -> &[usize; 512] {
        &*(self.to_kernel_virt().0 as *const [usize; 512])
    }
}

impl VirtPageNum {
    /// Returns the 4-level page-table indexes: `[l0, l1, l2, l3]`.
    pub fn indexes(&self) -> [usize; 4] {
        let vpn = self.0;
        [
            (vpn >> 27) & 0x1FF,  // L0: bits [47:39]
            (vpn >> 18) & 0x1FF,  // L1: bits [38:30]
            (vpn >> 9)  & 0x1FF,  // L2: bits [29:21]
            vpn          & 0x1FF,  // L3: bits [20:12]
        ]
    }
}

impl fmt::Debug for PhysAddr {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "PA({:#018x})", self.0)
    }
}

impl fmt::Debug for VirtAddr {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "VA({:#018x})", self.0)
    }
}

impl fmt::Debug for PhysPageNum {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "PPN({:#x})", self.0)
    }
}

impl fmt::Debug for VirtPageNum {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "VPN({:#x})", self.0)
    }
}

// ---------------------------------------------------------------------------
// AArch64 page-table descriptor (one entry at any level)
// ---------------------------------------------------------------------------

/// AArch64 page-table descriptor flags (lower 12 bits + upper attributes).
pub mod pte {
    // Descriptor type (bits [1:0])
    pub const DESC_INVALID: usize   = 0b00;
    pub const DESC_BLOCK: usize     = 0b01;  // L1/L2 block mapping
    pub const DESC_TABLE: usize     = 0b11;  // next-level table
    pub const DESC_PAGE: usize      = 0b11;  // L3 page (same encoding as table)

    // Memory attribute index (AttrIndx, bits [4:2])
    pub const MAIR_DEVICE_N_GN_RN_E: usize = 0 << 2;
    pub const MAIR_NORMAL_NC: usize     = 1 << 2;
    pub const MAIR_NORMAL_WB: usize     = 2 << 2;

    // Access permissions (AP, bits [7:6])
    // AP[1] (bit 6) = 1 → EL0 can access; AP[2] (bit 7) = 1 → read-only
    pub const AP_EL1_RW_EL0_NO: usize  = 0b00 << 6;  // EL1 RW, EL0 no access
    pub const AP_EL0_RW_EL1_RW: usize  = 0b01 << 6;  // EL0 RW, EL1 RW
    pub const AP_EL1_RO_EL0_NO: usize  = 0b10 << 6;  // EL1 RO, EL0 no access
    pub const AP_EL0_RO_EL1_RO: usize  = 0b11 << 6;  // EL0 RO, EL1 RO

    // Shareability (SH, bits [9:8])
    pub const SH_NON_SHAREABLE: usize = 0b00 << 8;
    pub const SH_OUTER_SHAREABLE: usize = 0b10 << 8;
    pub const SH_INNER_SHAREABLE: usize = 0b11 << 8;

    // Access flag (AF, bit 10)
    pub const AF: usize = 1 << 10;

    // Not Global (nG, bit 11) — 0 = global, 1 = non-global (process-specific)
    pub const NG: usize = 1 << 11;

    // Execution permissions (UXN/PXN, bits 53-54)
    pub const UXN: usize = 1 << 54;  // Unprivileged execute-never
    pub const PXN: usize = 1 << 53;  // Privileged execute-never

    // Access permission for block/page (AP, same bits [7:6])

    /// Build a next-level table descriptor pointing to `next_ppn` (physical addr >> 12).
    #[inline]
    pub fn table_descriptor(next_ppn: usize) -> usize {
        (next_ppn << 12) | DESC_TABLE
    }

    /// Build a block/page descriptor (valid, normal WB cacheable, RW at EL1, inner shareable).
    #[inline]
    pub fn block_page_descriptor(output_ppn: usize, is_page: bool) -> usize {
        let mut desc = (output_ppn << 12) | AF;
        desc |= MAIR_NORMAL_WB;
        desc |= SH_INNER_SHAREABLE;
        desc |= AP_EL0_RW_EL1_RW;
        if is_page {
            desc |= DESC_PAGE;
        } else {
            desc |= DESC_BLOCK;
        }
        desc
    }

    /// Build a valid block/page descriptor for kernel use only (EL1 access, no EL0).
    #[inline]
    pub fn kernel_block_page_descriptor(output_ppn: usize, is_page: bool) -> usize {
        let mut desc = (output_ppn << 12) | AF;
        desc |= MAIR_NORMAL_WB;
        desc |= SH_INNER_SHAREABLE;
        desc |= AP_EL1_RW_EL0_NO; // kernel-only
        desc |= UXN | PXN;        // non-executable by default
        if is_page {
            desc |= DESC_PAGE;
        } else {
            desc |= DESC_BLOCK;
        }
        desc
    }
}

// ---------------------------------------------------------------------------
// Global page allocator
// ---------------------------------------------------------------------------

/// Node stored inside each free physical page, forming the free list.
struct FreePage {
    next: *mut FreePage,
}

/// Send-safe wrapper around a raw pointer.
///
/// Required because `spin::Mutex<T>` needs `T: Send` for the static to be `Sync`.
/// In a bare-metal kernel the memory is directly mapped and always accessible.
struct FreeListPtr(*mut FreePage);
unsafe impl Send for FreeListPtr {}

/// The global free-page list, protected by a spinlock for SMP safety.
static FREE_LIST: Mutex<FreeListPtr> = Mutex::new(FreeListPtr(core::ptr::null_mut()));
static FREE_COUNT: Mutex<usize> = Mutex::new(0);

/// Allocate a single zeroed physical page.
///
/// Returns `None` if the allocator is exhausted.
pub fn alloc_page() -> Option<usize> {
    let mut list = FREE_LIST.lock();
    let head = list.0;
    if head.is_null() {
        return None;
    }
    unsafe {
        list.0 = (*head).next;
        *FREE_COUNT.lock() -= 1;

        // Zero the page
        let ptr = head as *mut u8;
        ptr::write_bytes(ptr, 0, PAGE_SIZE);

        Some(head as usize)
    }
}

/// Return a previously allocated page to the free list.
///
/// # Safety
///
/// The page must have been obtained from `alloc_page()` and not already freed.
pub fn free_page(phys_addr: usize) {
    let addr = phys_addr & !(PAGE_SIZE - 1); // align down
    let mut list = FREE_LIST.lock();
    unsafe {
        let node = addr as *mut FreePage;
        (*node).next = list.0;
        list.0 = node;
        *FREE_COUNT.lock() += 1;
    }
}

/// Add a contiguous range of physical memory `[start, end)` to the free list.
///
/// Both `start` and `end` are automatically page-aligned before use.
pub fn free_page_range(start: usize, end: usize) {
    let start = (start + PAGE_SIZE - 1) & !(PAGE_SIZE - 1); // align up
    let end = end & !(PAGE_SIZE - 1); // align down

    let mut addr = start;
    while addr + PAGE_SIZE <= end {
        free_page(addr);
        addr += PAGE_SIZE;
    }
}

/// Return the number of pages currently available.
pub fn free_page_count() -> usize {
    *FREE_COUNT.lock()
}
