//! AArch64 page table implementation.
//!
//! # Overview
//!
//! AArch64 uses a 4-level page table with 48-bit virtual addresses
//! and 4 KiB granule size:
//!
//! ```text
//! VA [47:39] → L0 table → [38:30] → L1 table → [29:21] → L2 table → [20:12] → L3 table → PA
//!              512 entries            512 entries            512 entries           512 entries
//! ```
//!
//! Each level contains 512 64-bit descriptors. The root table address
//! is held in TTBR0_EL1 (user) or TTBR1_EL1 (kernel).
//!
//! # Descriptor format
//!
//! A valid descriptor at any level has bit[0]=1. Bit[1] distinguishes:
//! - `1` = table descriptor (points to next-level table)
//! - `0` = block/page descriptor (maps the VA range directly)
//!
//! Block mappings:
//! - L0: not supported on 4 KiB granule with 48-bit VA
//! - L1: 1 GiB block
//! - L2: 2 MiB block
//! - L3: 4 KiB page (only level that supports pages)

use core::fmt::{self, Debug, Formatter};
use super::PAGE_SHIFT;
use super::{free_page, VirtAddr, VirtPageNum};
use super::{alloc_page, PhysAddr, PhysPageNum};
use super::pte;

/// 一个物理页正好 4096 字节。
/// 包含一个 usize 的 count，一个 Option<PhysPageNum> 的 next，
/// 剩下的空间全部用来存 PhysPageNum (8 字节)。
/// (4096 - 8 - 8) / 8 = 510
const TRACKER_CAPACITY: usize = 509;

// ---------------------------------------------------------------------------
// PTEFlags — logical permission flags (hardware-independent layer)
// ---------------------------------------------------------------------------

/// Logical page-table-entry flags.
///
/// These represent the intent (readable, writable, user, …) independent
/// of the exact hardware encoding. The hardware descriptor is built by
/// combining `PTEFlags` with the output address in `PageTableEntry::new()`.
#[derive(Copy, Clone, PartialEq, Eq)]
pub struct PTEFlags {
    pub bits: u16,
}

impl PTEFlags {
    pub const V:  u16 = 1 << 0;  // Valid
    pub const R:  u16 = 1 << 1;  // Read
    pub const W:  u16 = 1 << 2;  // Write
    pub const X:  u16 = 1 << 3;  // Execute
    pub const U:  u16 = 1 << 4;  // User-accessible
    pub const G:  u16 = 1 << 5;  // Global mapping
    pub const A:  u16 = 1 << 6;  // Accessed
    pub const D:  u16 = 1 << 7;  // Dirty
    pub const COW: u16 = 1 << 8; // Copy-on-write
    pub const S:  u16 = 1 << 9;  // Shared

    pub fn empty() -> Self { PTEFlags { bits: 0 } }

    pub fn contains(&self, flag: u16) -> bool {
        self.bits & flag != 0
    }

    pub fn insert(&mut self, flag: u16) {
        self.bits |= flag;
    }

    pub fn remove(&mut self, flag: u16) {
        self.bits &= !flag;
    }

    pub fn readable_flags(&self) -> &'static str {
        // Using a static buffer for no_std compatibility
        "PTEFlags" // simple placeholder; extended in debug builds
    }
}

impl Debug for PTEFlags {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let mut first = true;
        let mut put = |s: &str| {
            if !first { write!(f, "|").ok(); } else { first = false; }
            write!(f, "{}", s).ok();
        };
        if self.contains(Self::V)   { put("V"); }
        if self.contains(Self::R)   { put("R"); }
        if self.contains(Self::W)   { put("W"); }
        if self.contains(Self::X)   { put("X"); }
        if self.contains(Self::U)   { put("U"); }
        if self.contains(Self::G)   { put("G"); }
        if self.contains(Self::A)   { put("A"); }
        if self.contains(Self::D)   { put("D"); }
        if self.contains(Self::COW) { put("COW"); }
        if self.contains(Self::S)   { put("S"); }
        if first { write!(f, "EMPTY")?; }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// PageTableEntry — a single 64-bit AArch64 descriptor
// ---------------------------------------------------------------------------

/// A single entry (descriptor) in an AArch64 page table.
#[derive(Copy, Clone)]
#[repr(C)]
pub struct PageTableEntry {
    pub bits: usize,
}

impl Debug for PageTableEntry {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let ppn = self.ppn().0;
        if self.is_valid() {
            if self.is_table() {
                write!(f, "PTE {{ next_table: PPN({:#x}) }}", ppn)
            } else {
                write!(f, "PTE {{ ppn: {:#x}, af:{} ap:{:02b} sh:{:02b} uxn:{} pxn:{} }}",
                    ppn,
                    self.contains(pte::AF),
                    (self.bits >> 6) & 0b11,
                    (self.bits >> 8) & 0b11,
                    self.contains(pte::UXN),
                    self.contains(pte::PXN),
                )
            }
        } else {
            write!(f, "PTE {{ INVALID }}")
        }
    }
}

impl PageTableEntry {
    /// Build a next-level table descriptor pointing to `next_ppn`.
    /// `next_ppn` is the physical address of the next-level table, page-aligned.
    pub fn new_table(next_ppn: PhysPageNum) -> Self {
        PageTableEntry {
            bits: (next_ppn.0 << PAGE_SHIFT) | pte::DESC_TABLE,
        }
    }

    /// Build a leaf page descriptor from `ppn` and logical `flags`.
    ///
    /// Maps the AArch64 hardware-independent flags to the real descriptor:
    /// - V  → bit[0]=1 (valid)
    /// - R  → AP=0b11 (EL0 RO) if no W, else AP=0b01 (EL0 RW)
    /// - W  → AP=0b01 (EL0+EL1 RW)
    /// - X  → UXN=0, PXN=0
    /// - U  → AP allows EL0 access; nG=1 (non-global)
    /// - G  → nG=0 (global)
    pub fn new(ppn: PhysPageNum, flags: PTEFlags) -> Self {
        let mut desc: usize = (ppn.0 << PAGE_SHIFT) | pte::DESC_PAGE;

        // Always set AF so the hardware doesn't fault on first access
        desc |= pte::AF;
        // Default cacheable memory
        desc |= pte::MAIR_NORMAL_WB;
        desc |= pte::SH_INNER_SHAREABLE;

        // Access permissions
        // AP[1]=1 → EL0 access; AP[2]=1 → read-only
        if flags.contains(PTEFlags::U) {
            // User-accessible
            if flags.contains(PTEFlags::W) {
                desc |= pte::AP_EL0_RW_EL1_RW; // 0b01: EL0 RW
            } else if flags.contains(PTEFlags::R) {
                desc |= pte::AP_EL0_RO_EL1_RO; // 0b11: EL0 RO
            } else {
                desc |= pte::AP_EL1_RW_EL0_NO; // 0b00: EL0 no access
            }
            desc |= pte::NG; // Non-global (process-specific)
        } else {
            // Kernel-only
            desc |= pte::AP_EL1_RW_EL0_NO;
        }

        // Execute permissions: default non-executable unless X is set
        if !flags.contains(PTEFlags::X) {
            desc |= pte::UXN | pte::PXN;
        }

        // Global mapping
        if flags.contains(PTEFlags::G) {
            // nG bit already set to 0 by default (global)
        }

        PageTableEntry { bits: desc }
    }

    /// Copy-on-write: from an existing writable PTE, clear W and mark COW.
    pub fn from_pte_cow(pte: PageTableEntry) -> Self {
        let mut desc = pte.bits;
        // Set AP to read-only for EL0 and EL1
        desc &= !(0b11 << 6);
        desc |= pte::AP_EL0_RO_EL1_RO;
        PageTableEntry { bits: desc }
    }

    /// An invalid (empty) descriptor.
    pub fn empty() -> Self {
        PageTableEntry { bits: 0 }
    }

    /// Extract the physical page number from the output address field.
    pub fn ppn(&self) -> PhysPageNum {
        PhysPageNum((self.bits >> PAGE_SHIFT) & ((1 << 36) - 1))
    }

    /// Return the logical flags decoded from the hardware descriptor.
    pub fn flags(&self) -> PTEFlags {
        let mut f = PTEFlags::empty();
        if self.is_valid() { f.insert(PTEFlags::V); }
        // Decode AP field (correct encoding: AP[1]=EL0 access, AP[2]=RO)
        let ap = (self.bits >> 6) & 0b11;
        match ap {
            0b00 => { f.insert(PTEFlags::R); f.insert(PTEFlags::W); }              // EL1 RW
            0b01 => { f.insert(PTEFlags::R); f.insert(PTEFlags::W); f.insert(PTEFlags::U); } // EL0 RW
            0b10 => { f.insert(PTEFlags::R); }                                     // EL1 RO
            0b11 => { f.insert(PTEFlags::R); f.insert(PTEFlags::U); }              // EL0 RO
            _ => {}
        }
        if !self.contains(pte::UXN) || !self.contains(pte::PXN) {
            f.insert(PTEFlags::X);
        }
        if !self.contains(pte::NG) { f.insert(PTEFlags::G); }
        if self.contains(pte::AF) { f.insert(PTEFlags::A); }
        // D (dirty) is managed by the Access Flag / AP combination on ARM;
        // we optimistically mark it set since we handle CoW at a higher layer
        f.insert(PTEFlags::D);
        f
    }

    /// Check whether a specific hardware bit/field is set.
    #[inline]
    pub fn contains(&self, field: usize) -> bool {
        self.bits & field != 0
    }

    /// True if this descriptor represents a valid mapping.
    pub fn is_valid(&self) -> bool {
        self.bits & 1 != 0
    }

    /// True if this is a table descriptor (points to next-level page table).
    pub fn is_table(&self) -> bool {
        (self.bits & 0b11) == pte::DESC_TABLE
    }

    /// True if this is a block or page descriptor (leaf mapping).
    pub fn is_block_or_page(&self) -> bool {
        (self.bits & 0b11) == pte::DESC_BLOCK || (self.bits & 0b11) == pte::DESC_PAGE
    }

    pub fn readable(&self) -> bool {
        self.is_valid() // Any valid mapping is readable by EL1 at minimum
    }

    pub fn writable(&self) -> bool {
        let ap = (self.bits >> 6) & 0b11;
        ap == 0b00 || ap == 0b01 // AP[2]=0 → writable
    }

    pub fn executable(&self) -> bool {
        !self.contains(pte::PXN) // Privileged execute never is clear
    }

    pub fn is_user(&self) -> bool {
        let ap = (self.bits >> 6) & 0b11;
        ap & 0b01 != 0 // AP[1]=1 → EL0 accessible
    }

    pub fn is_cow(&self) -> bool {
        // COW pages are readable but not writable at user level
        let ap = (self.bits >> 6) & 0b11;
        ap == 0b11 // AP=0b11: EL0 RO, EL1 RO (kernel writes via identity map)
    }

    pub fn is_shared(&self) -> bool {
        false // stub — shared detection needs higher-layer tracking
    }
}

// ---------------------------------------------------------------------------
// PageTable — root page table with methods
// ---------------------------------------------------------------------------

#[repr(C)]
struct TrackerPage {
    frames: [PhysPageNum; TRACKER_CAPACITY],
    count: usize,
    next: Option<PhysPageNum>,
}

pub struct PageTable {
    pub root_ppn: PhysPageNum,
    /// 指向第一个 TrackerPage 的物理页号
    tracker_head: Option<PhysPageNum>,
    /// Whether this PageTable owns the root page and should free it on drop.
    owns_root: bool,
}

impl PageTable {
    /// Register a newly-allocated intermediate page table frame so it is freed
    /// when the PageTable is dropped.
    pub fn track_frame(&mut self, ppn: PhysPageNum) {
        if self.tracker_head.is_none() {
            // 分配第一个 TrackerPage
            let tp_pa = alloc_page().expect("OOM tracking frame");
            let tracker = unsafe {
                &mut *(PhysAddr::from(tp_pa).to_kernel_virt().0 as *mut TrackerPage)
            };
            tracker.count = 0;
            tracker.next = None;
            self.tracker_head = Some(PhysPageNum::from(tp_pa >> PAGE_SHIFT));
        }

        let current_ppn = self.tracker_head.unwrap();
        let tracker = unsafe {
            &mut *(PhysAddr::from(current_ppn).to_kernel_virt().0 as *mut TrackerPage)
        };

        if tracker.count < TRACKER_CAPACITY {
            tracker.frames[tracker.count] = ppn;
            tracker.count += 1;
        } else {
            // 当前 TrackerPage 已满，分配一个新的并将其作为新的 Head (头插法最快)
            let new_tp_pa = alloc_page().expect("OOM tracking frame");
            let new_tracker = unsafe {
                &mut *(PhysAddr::from(new_tp_pa).to_kernel_virt().0 as *mut TrackerPage)
            };
            new_tracker.count = 1;
            new_tracker.frames[0] = ppn;
            new_tracker.next = Some(current_ppn);
            self.tracker_head = Some(PhysPageNum::from(new_tp_pa >> PAGE_SHIFT));
        }
    }
}

impl Drop for PageTable {
    fn drop(&mut self) {
        // 1. 释放根页表 (仅当 PageTable 拥有根页表时)
        if self.owns_root {
            free_page(PhysAddr::from(self.root_ppn).0);
        }

        // 2. 遍历 TrackerPage 链表，释放所有中间页表，最后释放 TrackerPage 本身
        let mut current_opt = self.tracker_head;
        while let Some(tp_ppn) = current_opt {
            let tracker = unsafe {
                &mut *(PhysAddr::from(tp_ppn).to_kernel_virt().0 as *mut TrackerPage)
            };

            // 释放追踪的所有帧
            for i in 0..tracker.count {
                free_page(PhysAddr::from(tracker.frames[i]).0);
            }

            // 获取下一个 TrackerPage 的 PPN
            current_opt = tracker.next;

            // 释放这个 TrackerPage 自身
            free_page(PhysAddr::from(tp_ppn).0);
        }
    }
}

// ---------------------------------------------------------------------------
// Construction
// ---------------------------------------------------------------------------

impl PageTable {
    /// Create a new empty page table with a freshly allocated root.
    pub fn new() -> Self {
        let frame = alloc_page().expect("PageTable::new: out of memory");
        // Zero the root table
        let root_va = PhysAddr::from(frame).to_kernel_virt().0;
        unsafe { core::ptr::write_bytes(root_va as *mut u8, 0, 4096); }
        PageTable {
            root_ppn: PhysPageNum::from(frame >> PAGE_SHIFT),
            tracker_head: None,
            owns_root: true,
        }
    }

    /// Reconstruct a PageTable handle from a raw TTBR value.
    ///
    /// The returned handle does **not** own the root table — dropping it
    /// will **not** free any memory.  Use this only for temporary lookups.
    pub fn from_token(ttbr: usize) -> Self {
        // TTBR0_EL1 / TTBR1_EL1 hold the physical address of the root table
        // (bits [47:1] of the register = PA[47:1] of the root).
        let root_pa = ttbr & !(4096 - 1); // mask off low bits (CnP, etc.)
        PageTable {
            root_ppn: PhysPageNum::from(root_pa >> PAGE_SHIFT),
            tracker_head: None,
            owns_root: false, // borrowed view — do not free the root on drop
        }
    }

    /// Return the token that should be loaded into TTBR0_EL1 or TTBR1_EL1.
    pub fn token(&self) -> usize {
        // AArch64 TTBR holds the physical address of the root table.
        // Bits [47:1] = base address[47:1]; bit[0] is CnP (Common not Private).
        PhysAddr::from(self.root_ppn).0
    }
}

// ---------------------------------------------------------------------------
// PTE walking
// ---------------------------------------------------------------------------

impl PageTable {
    /// Walk the page table to find or create the leaf PTE for `vpn`.
    ///
    /// Intermediate table levels (L0, L1, L2) are allocated on-demand.
    /// Returns `None` if memory allocation fails.
    pub fn find_pte_create(&mut self, vpn: VirtPageNum) -> Option<&mut PageTableEntry> {
        let idxs = vpn.indexes();
        let mut ppn = self.root_ppn;

        for (i, &idx) in idxs.iter().enumerate() {
            let table = unsafe { ppn.get_pte_array_mut() };
            let entry = &mut table[idx];
            if i == 3 {
                // L3 — leaf level
                return Some(unsafe { &mut *(entry as *mut PageTableEntry) });
            }
            // L0-L2 — walk or allocate
            if !entry.is_valid() {
                // Allocate a new intermediate table
                let pa = alloc_page()?;
                // Zero the new table
                let va = PhysAddr::from(pa).to_kernel_virt().0;
                unsafe { core::ptr::write_bytes(va as *mut u8, 0, 4096); }

                let new_ppn = PhysPageNum::from(pa >> PAGE_SHIFT);
                *entry = PageTableEntry::new_table(new_ppn);

                self.track_frame(new_ppn);
            }
            let next_ppn = PhysPageNum::from((entry.bits >> PAGE_SHIFT) & ((1 << 36) - 1));
            ppn = next_ppn;
        }
        None
    }

    /// Walk the page table to find or create a PTE at the given level.
    ///
    /// Intermediate tables are allocated on-demand (as in `find_pte_create`).
    /// `target_level` is the desired walk depth:
    ///   0 = L0 (root), 1 = L1, 2 = L2, 3 = L3 (leaf).
    ///
    /// Returns `None` if memory allocation fails.
    pub fn get_entry_at_level_mut(
        &mut self,
        vpn: VirtPageNum,
        target_level: usize,
    ) -> Option<&mut PageTableEntry> {
        let idxs = vpn.indexes();
        let mut ppn = self.root_ppn;

        for (i, &idx) in idxs.iter().enumerate() {
            let table = unsafe { ppn.get_pte_array_mut() };
            let entry = &mut table[idx];

            if i == target_level {
                return Some(unsafe { &mut *(entry as *mut PageTableEntry) });
            }

            // Intermediate level — ensure table descriptor exists
            if !entry.is_valid() {
                let pa = alloc_page()?;
                let va = PhysAddr::from(pa).to_kernel_virt().0;
                unsafe { core::ptr::write_bytes(va as *mut u8, 0, 4096); }
                let new_ppn = PhysPageNum::from(pa >> PAGE_SHIFT);
                *entry = PageTableEntry::new_table(new_ppn);
                self.track_frame(new_ppn);
            }

            let next_ppn = PhysPageNum::from((entry.bits >> PAGE_SHIFT) & ((1 << 36) - 1));
            ppn = next_ppn;
        }
        None // walked past target_level
    }

    /// Walk the page table to find an existing leaf PTE for `vpn`.
    /// Returns `None` if any level is missing.
    pub fn find_pte(&self, vpn: VirtPageNum) -> Option<&mut PageTableEntry> {
        let idxs = vpn.indexes();
        let mut ppn = self.root_ppn;

        for (i, &idx) in idxs.iter().enumerate() {
            let table = unsafe { ppn.get_pte_array_mut() };
            let entry = &mut table[idx];
            if i == 3 {
                // L3 — leaf level
                return Some(unsafe { &mut *(entry as *mut PageTableEntry) });
            }
            // L0-L2 — must be valid table descriptors
            if !entry.is_valid() || !entry.is_table() {
                return None;
            }
            let next_ppn = PhysPageNum::from((entry.bits >> PAGE_SHIFT) & ((1 << 36) - 1));
            ppn = next_ppn;
        }
        None
    }

    /// Map `vpn` → `ppn` with the given logical flags.
    pub fn map(&mut self, vpn: VirtPageNum, ppn: PhysPageNum, flags: PTEFlags) {
        let pte = self.find_pte_create(vpn).expect("PageTable::map: out of memory");
        *pte = PageTableEntry::new(ppn, flags);
        tlb_invalidate_addr(VirtAddr::from(vpn));
    }

    /// Remap (change permissions on) an already-mapped `vpn`.
    pub fn remap(&mut self, vpn: VirtPageNum, flags: PTEFlags) {
        let pte = self.find_pte(vpn).expect("PageTable::remap: vpn not mapped");
        assert!(pte.is_valid(), "PageTable::remap: vpn not mapped");
        *pte = PageTableEntry::new(pte.ppn(), flags);
        tlb_invalidate_addr(VirtAddr::from(vpn));
    }

    /// Unmap `vpn`.
    pub fn unmap(&mut self, vpn: VirtPageNum) {
        let pte = self.find_pte(vpn).expect("PageTable::unmap: vpn not mapped");
        assert!(pte.is_valid(), "PageTable::unmap: vpn not mapped");
        *pte = PageTableEntry::empty();
        tlb_invalidate_addr(VirtAddr::from(vpn));
    }
}

// ---------------------------------------------------------------------------
// Address translation
// ---------------------------------------------------------------------------

impl PageTable {
    /// Translate a virtual address to a physical address.
    /// Returns `None` if the VA is not mapped.
    pub fn translate_va_to_pa(&self, va: VirtAddr) -> Option<usize> {
        self.find_pte(VirtPageNum::from(va.floor())).map(|pte| {
            let pa_aligned: PhysAddr = PhysPageNum::from(PhysAddr::from(pte.ppn().0 << PAGE_SHIFT)).into();
            pa_aligned.0 + va.page_offset()
        })
    }

    /// Return a copy of the PTE for `vpn`, if mapped.
    pub fn translate_vpn_to_pte(&self, vpn: VirtPageNum) -> Option<PageTableEntry> {
        self.find_pte(vpn).map(|pte| *pte)
    }
}

// ---------------------------------------------------------------------------
// Bulk mapping
// ---------------------------------------------------------------------------

impl PageTable {
    /// Return a mutable reference to the L3 page table that covers `vpn`,
    /// allocating intermediate levels as needed.
    pub fn find_pte_array_mut(&mut self, vpn: VirtPageNum) -> Option<&mut [PageTableEntry]> {
        let idxs = vpn.indexes();
        let mut ppn = self.root_ppn;

        for (i, &idx) in idxs.iter().enumerate() {
            let table = unsafe { ppn.get_pte_array_mut() };
            let entry = &mut table[idx];

            if i == 2 {
                // Arrived at L2 — next hop is the L3 table
                if !entry.is_valid() {
                    let pa = alloc_page()?;
                    let va = PhysAddr::from(pa).to_kernel_virt().0;
                    unsafe { core::ptr::write_bytes(va as *mut u8, 0, 4096); }
                    let new_ppn = PhysPageNum::from(pa >> PAGE_SHIFT);
                    *entry = PageTableEntry::new_table(new_ppn);
                    self.track_frame(new_ppn);
                }
                let l3_ppn = PhysPageNum::from((entry.bits >> PAGE_SHIFT) & ((1 << 36) - 1));
                return Some(unsafe {
                    &mut *(PhysAddr::from(l3_ppn).to_kernel_virt().0 as *mut [PageTableEntry; 512])
                });
            }

            if !entry.is_valid() {
                let pa = alloc_page()?;
                let va = PhysAddr::from(pa).to_kernel_virt().0;
                unsafe { core::ptr::write_bytes(va as *mut u8, 0, 4096); }
                let new_ppn = PhysPageNum::from(pa >> PAGE_SHIFT);
                *entry = PageTableEntry::new_table(new_ppn);
                self.track_frame(new_ppn);
            }

            let next_ppn = PhysPageNum::from((entry.bits >> PAGE_SHIFT) & ((1 << 36) - 1));
            ppn = next_ppn;
        }
        None
    }

    /// Map a range of anonymous virtual pages, allocating physical pages
    /// as needed (one per page in `[start_vpn, end_vpn)`).
    pub fn map_anony_range(
        &mut self,
        start_vpn: VirtPageNum,
        end_vpn: VirtPageNum,
        flags: PTEFlags,
    ) {
        assert!(start_vpn.0 <= end_vpn.0);
        let mut vpn = start_vpn.0;

        let mut pte_arr = self.find_pte_array_mut(start_vpn)
            .expect("PageTable::map_anony_range: out of memory");

        while vpn < end_vpn.0 {
            let cur_vpn = VirtPageNum(vpn);
            let idxs = cur_vpn.indexes();

            let pa = alloc_page().expect("PageTable::map_anony_range: out of memory");
            let ppn = PhysPageNum::from(pa >> PAGE_SHIFT);

            pte_arr[idxs[3]] = PageTableEntry::new(ppn, flags);
            tlb_invalidate_addr(VirtAddr::from(VirtPageNum(vpn)));
            vpn += 1;

            // Crossed into a new L3 table
            if idxs[3] >= 511 && vpn < end_vpn.0 {
                pte_arr = self.find_pte_array_mut(VirtPageNum(vpn))
                    .expect("PageTable::map_anony_range: out of memory");
            }
        }
    }

    /// Map a pre-allocated contiguous range of physical pages to `[start_vpn, end_vpn)`.
    pub fn map_range_continuous(
        &mut self,
        start_vpn: VirtPageNum,
        end_vpn: VirtPageNum,
        start_ppn: PhysPageNum,
        flags: PTEFlags,
    ) {
        assert!(start_vpn.0 <= end_vpn.0);
        let mut vpn = start_vpn.0;
        let mut ppn = start_ppn.0;

        let mut pte_arr = self.find_pte_array_mut(start_vpn)
            .expect("PageTable::map_range_continuous: out of memory");

        while vpn < end_vpn.0 {
            let cur_vpn = VirtPageNum(vpn);
            let idxs = cur_vpn.indexes();

            pte_arr[idxs[3]] = PageTableEntry::new(PhysPageNum(ppn), flags);
            vpn += 1;
            ppn += 1;

            if idxs[3] >= 511 && vpn < end_vpn.0 {
                pte_arr = self.find_pte_array_mut(VirtPageNum(vpn))
                    .expect("PageTable::map_range_continuous: out of memory");
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Debug / dump
// ---------------------------------------------------------------------------

impl PageTable {
    /// Dump all user-space mappings recursively.
    pub fn dump_all_user_mapping(&self) {
        log::info!("[dump_all] PageTable root_ppn: {:?}", self.root_ppn);
        self.dump_level(
            self.root_ppn,
            0,   // level (0 = L0)
            0,   // accumulated VA
            39,  // starting shift for L0 (VA bits [47:39])
        );
        log::info!("[dump_all] end");
    }

    fn dump_level(&self, ppn: PhysPageNum, level: usize, va_base: usize, shift: usize) {
        let table = unsafe { ppn.get_pte_array() };
        for (idx, entry) in table.iter().enumerate() {
            if !entry.is_valid() {
                continue;
            }
            let va = va_base | (idx << shift);

            if level == 3 {
                // Leaf page
                log::info!("--- VA({:#018x}): {:?}", va, entry);
            } else if entry.is_table() {
                // Intermediate table — recurse
                let next_ppn = PhysPageNum::from((entry.bits >> PAGE_SHIFT) & ((1 << 36) - 1));
                let next_shift = shift - 9;
                self.dump_level(next_ppn, level + 1, va, next_shift);
            } else if entry.is_block_or_page() {
                // Block mapping at this level
                log::info!("--- VA({:#018x}) [BLOCK L{}]: {:?}", va, level, entry);
            }
        }
    }

    /// Walk and dump the page-table path for a specific VA.
    pub fn dump_with_va(&self, va: usize) {
        log::info!("[dump_with_va] VA({:#018x})", va);
        let vpn = VirtPageNum::from(va >> PAGE_SHIFT);
        let idxs = vpn.indexes();
        let mut ppn = self.root_ppn;

        let level_names = ["L0", "L1", "L2", "L3"];

        for (i, &idx) in idxs.iter().enumerate() {
            let table = unsafe { ppn.get_pte_array() };
            let entry = table[idx];

            if !entry.is_valid() {
                log::info!("  {}[{}]: INVALID", level_names[i], idx);
                return;
            }
            log::info!("  {}[{}]: {:?}", level_names[i], idx, entry);

            if i == 3 {
                return; // leaf
            }
            if entry.is_table() {
                let next_ppn = PhysPageNum::from((entry.bits >> PAGE_SHIFT) & ((1 << 36) - 1));
                ppn = next_ppn;
            } else if entry.is_block_or_page() {
                log::info!("  -> block mapping at {}", level_names[i]);
                return;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// TLB maintenance
// ---------------------------------------------------------------------------

/// Invalidate the entire TLB for the current VMID/ASID context.
///
/// On AArch64 this executes `tlbi vmalle1is` (inner shareable).
pub fn tlb_invalidate() {
    unsafe {
        core::arch::asm!("tlbi vmalle1is; dsb ish; isb");
    }
}

/// Invalidate a single VA in the TLB.
///
/// On AArch64 this executes `tlbi vaae1is` (by VA, all ASID, EL1, inner shareable).
pub fn tlb_invalidate_addr(va: VirtAddr) {
    unsafe {
        core::arch::asm!(
            "tlbi vaae1is, {}; dsb ish; isb",
            in(reg) (va.0 >> 12) & ((1 << 36) - 1),
        );
    }
}

/// Read the current TTBR1_EL1 (kernel page table root).
pub fn current_token() -> usize {
    let token: usize;
    unsafe {
        core::arch::asm!("mrs {}, ttbr1_el1", out(reg) token);
    }
    token
}
