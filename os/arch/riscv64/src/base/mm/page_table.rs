//! RISC-V Sv39 页表实现。
//!
//! 接口与 aarch64 crate 完全一致（`PageTable` / `PageTableEntry` / `PTEFlags`），
//! 底层为 RISC-V Sv39 三级页表：VPN[2] → VPN[1] → VPN[0] → offset。

use core::fmt::{self, Debug, Formatter};
use super::{alloc_page, free_page, PhysAddr, PhysPageNum, VirtAddr, VirtPageNum, PAGE_SHIFT};
use super::pte;

const TRACKER_CAPACITY: usize = 509;

// ---------------------------------------------------------------------------
// PTEFlags — 逻辑权限标志（与硬件编码无关）
// ---------------------------------------------------------------------------

#[derive(Copy, Clone, PartialEq, Eq)]
pub struct PTEFlags {
    pub bits: u16,
}

impl PTEFlags {
    pub const V: u16 = 1 << 0; // Valid
    pub const R: u16 = 1 << 1; // Read
    pub const W: u16 = 1 << 2; // Write
    pub const X: u16 = 1 << 3; // Execute
    pub const U: u16 = 1 << 4; // User-accessible
    pub const G: u16 = 1 << 5; // Global mapping
    pub const A: u16 = 1 << 6; // Accessed
    pub const D: u16 = 1 << 7; // Dirty
    pub const COW: u16 = 1 << 8; // Copy-on-write
    pub const S: u16 = 1 << 9; // Shared

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
}

impl Debug for PTEFlags {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let mut first = true;
        if self.contains(Self::V) { write!(f, "V")?; first = false; }
        if self.contains(Self::R) { if !first { write!(f, "|")?; } write!(f, "R")?; first = false; }
        if self.contains(Self::W) { if !first { write!(f, "|")?; } write!(f, "W")?; first = false; }
        if self.contains(Self::X) { if !first { write!(f, "|")?; } write!(f, "X")?; first = false; }
        if self.contains(Self::U) { if !first { write!(f, "|")?; } write!(f, "U")?; first = false; }
        if self.contains(Self::G) { if !first { write!(f, "|")?; } write!(f, "G")?; first = false; }
        if self.contains(Self::A) { if !first { write!(f, "|")?; } write!(f, "A")?; first = false; }
        if self.contains(Self::D) { if !first { write!(f, "|")?; } write!(f, "D")?; first = false; }
        if self.contains(Self::COW) { if !first { write!(f, "|")?; } write!(f, "COW")?; first = false; }
        if self.contains(Self::S) { if !first { write!(f, "|")?; } write!(f, "S")?; }
        if first { write!(f, "EMPTY")?; }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// PageTableEntry — RISC-V Sv39 单个 PTE
// ---------------------------------------------------------------------------

#[derive(Copy, Clone)]
#[repr(C)]
pub struct PageTableEntry {
    pub bits: usize,
}

impl Debug for PageTableEntry {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        if self.is_valid() {
            if self.is_table() {
                write!(f, "PTE {{ next_table: PPN({:#x}) }}", self.ppn().0)
            } else {
                write!(f, "PTE {{ ppn: {:#x}, rwx:{}{}{} u:{} g:{} a:{} d:{} }}",
                    self.ppn().0,
                    if self.readable() { "r" } else { "-" },
                    if self.writable() { "w" } else { "-" },
                    if self.executable() { "x" } else { "-" },
                    self.is_user(),
                    self.contains(pte::G),
                    self.contains(pte::A),
                    self.contains(pte::D),
                )
            }
        } else {
            write!(f, "PTE {{ INVALID }}")
        }
    }
}

impl PageTableEntry {
    /// 构建指向下一级页表的描述符（V=1, R=W=X=0）。
    pub fn new_table(next_ppn: PhysPageNum) -> Self {
        PageTableEntry { bits: (next_ppn.0 << 10) | pte::V }
    }

    /// 从物理页号和逻辑 flags 构建叶子页描述符。
    pub fn new(ppn: PhysPageNum, flags: PTEFlags) -> Self {
        let mut bits: usize = (ppn.0 << 10) | pte::V | pte::R;
        if flags.contains(PTEFlags::W) { bits |= pte::W; }
        if flags.contains(PTEFlags::X) { bits |= pte::X; }
        if flags.contains(PTEFlags::U) { bits |= pte::U; }
        if flags.contains(PTEFlags::G) { bits |= pte::G; }
        // 不做硬件 A/D 跟踪，直接置位，避免首次访问缺页。
        bits |= pte::A | pte::D;
        PageTableEntry { bits }
    }

    /// Copy-on-write：从可写 PTE 生成只读 PTE（清 W，保留 R）。
    pub fn from_pte_cow(pte: PageTableEntry) -> Self {
        PageTableEntry { bits: (pte.bits & !pte::W) | pte::A | pte::D }
    }

    pub fn empty() -> Self {
        PageTableEntry { bits: 0 }
    }

    pub fn ppn(&self) -> PhysPageNum {
        PhysPageNum((self.bits >> 10) & ((1 << 44) - 1))
    }

    /// 从硬件 PTE 解码出逻辑 flags。
    pub fn flags(&self) -> PTEFlags {
        let mut f = PTEFlags::empty();
        if self.is_valid() { f.insert(PTEFlags::V); }
        if self.readable() { f.insert(PTEFlags::R); }
        if self.writable() { f.insert(PTEFlags::W); }
        if self.executable() { f.insert(PTEFlags::X); }
        if self.is_user() { f.insert(PTEFlags::U); }
        if self.contains(pte::G) { f.insert(PTEFlags::G); }
        if self.contains(pte::A) { f.insert(PTEFlags::A); }
        if self.contains(pte::D) { f.insert(PTEFlags::D); }
        f
    }

    #[inline]
    pub fn contains(&self, field: usize) -> bool {
        self.bits & field != 0
    }

    pub fn is_valid(&self) -> bool {
        self.bits & pte::V != 0
    }

    /// 是否是指向下一级页表的描述符（V=1, R=W=X=0）。
    pub fn is_table(&self) -> bool {
        self.is_valid() && (self.bits & (pte::R | pte::W | pte::X)) == 0
    }

    /// 是否是叶子页描述符（V=1 且 R/W/X 至少一位为 1）。
    pub fn is_block_or_page(&self) -> bool {
        self.is_valid() && (self.bits & (pte::R | pte::W | pte::X)) != 0
    }

    pub fn readable(&self) -> bool {
        self.is_valid() && self.bits & pte::R != 0
    }

    pub fn writable(&self) -> bool {
        self.is_valid() && self.bits & pte::W != 0
    }

    pub fn executable(&self) -> bool {
        self.is_valid() && self.bits & pte::X != 0
    }

    pub fn is_user(&self) -> bool {
        self.bits & pte::U != 0
    }

    pub fn is_cow(&self) -> bool {
        // COW 页：可读但不可写（曾经可写，被清 W）。
        self.is_valid() && self.bits & pte::R != 0 && self.bits & pte::W == 0
    }

    pub fn is_shared(&self) -> bool {
        false
    }
}

// ---------------------------------------------------------------------------
// PageTable — 根页表
// ---------------------------------------------------------------------------

#[repr(C)]
struct TrackerPage {
    frames: [PhysPageNum; TRACKER_CAPACITY],
    count: usize,
    next: Option<PhysPageNum>,
}

pub struct PageTable {
    pub root_ppn: PhysPageNum,
    tracker_head: Option<PhysPageNum>,
    owns_root: bool,
}

impl PageTable {
    pub fn track_frame(&mut self, ppn: PhysPageNum) {
        if !self.owns_root {
            return;
        }
        if self.tracker_head.is_none() {
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

    /// 遍历整张页表，释放所有 U 位（用户可访问）置位的叶子数据页。
    fn free_user_data_pages(&self) {
        let root_ppn = self.root_ppn;
        let l2_table = unsafe { &*((PhysAddr::from(root_ppn).0) as *const [usize; 512]) };
        for l2_idx in 0..512usize {
            let l2_bits = l2_table[l2_idx];
            if l2_bits & pte::V == 0 || l2_bits & (pte::R | pte::W | pte::X) != 0 { continue; }
            let l1_ppn = PhysPageNum::from((l2_bits >> 10) & ((1 << 44) - 1));
            let l1_table = unsafe { &*((PhysAddr::from(l1_ppn).0) as *const [usize; 512]) };
            for l1_idx in 0..512usize {
                let l1_bits = l1_table[l1_idx];
                if l1_bits & pte::V == 0 || l1_bits & (pte::R | pte::W | pte::X) != 0 { continue; }
                let l0_ppn = PhysPageNum::from((l1_bits >> 10) & ((1 << 44) - 1));
                let l0_table = unsafe { &*((PhysAddr::from(l0_ppn).0) as *const [usize; 512]) };
                for l0_idx in 0..512usize {
                    let l0_bits = l0_table[l0_idx];
                    if l0_bits & pte::V == 0 || l0_bits & (pte::R | pte::W | pte::X) == 0 { continue; }
                    if l0_bits & pte::U == 0 { continue; }
                    free_page((l0_bits >> 10) << PAGE_SHIFT);
                }
            }
        }
    }
}

impl Drop for PageTable {
    fn drop(&mut self) {
        if !self.owns_root {
            return;
        }
        self.free_user_data_pages();
        free_page(PhysAddr::from(self.root_ppn).0);

        let mut current_opt = self.tracker_head;
        while let Some(tp_ppn) = current_opt {
            let tracker = unsafe {
                &mut *(PhysAddr::from(tp_ppn).to_kernel_virt().0 as *mut TrackerPage)
            };
            for i in 0..tracker.count {
                free_page(PhysAddr::from(tracker.frames[i]).0);
            }
            current_opt = tracker.next;
            free_page(PhysAddr::from(tp_ppn).0);
        }
    }
}

// ---------------------------------------------------------------------------
// Construction
// ---------------------------------------------------------------------------

impl PageTable {
    pub fn new() -> Self {
        let frame = alloc_page().expect("PageTable::new: out of memory");
        let root_va = PhysAddr::from(frame).to_kernel_virt().0;
        unsafe { core::ptr::write_bytes(root_va as *mut u8, 0, 4096); }
        PageTable {
            root_ppn: PhysPageNum::from(frame >> PAGE_SHIFT),
            tracker_head: None,
            owns_root: true,
        }
    }

    /// 从「根页表物理地址」重建 PageTable 句柄（借用视图，drop 时不释放）。
    /// 与 aarch64 的 `from_token(ttbr)` 语义一致：token 即根页表 PA。
    pub fn from_token(root_pa: usize) -> Self {
        let root_ppn = (root_pa & !((1 << PAGE_SHIFT) - 1)) >> PAGE_SHIFT;
        PageTable {
            root_ppn: PhysPageNum::from(root_ppn),
            tracker_head: None,
            owns_root: false,
        }
    }

    /// 返回根页表物理地址（token），与 aarch64 语义一致。
    pub fn token(&self) -> usize {
        PhysAddr::from(self.root_ppn).0
    }
}

// ---------------------------------------------------------------------------
// PTE walking
// ---------------------------------------------------------------------------

impl PageTable {
    /// 走到 `vpn` 对应的叶子 PTE，中间层按需分配。
    pub fn find_pte_create(&mut self, vpn: VirtPageNum) -> Option<&mut PageTableEntry> {
        let idxs = vpn.indexes();
        let mut ppn = self.root_ppn;

        for (i, &idx) in idxs.iter().enumerate() {
            let table = unsafe { ppn.get_pte_array_mut() };
            let entry = &mut table[idx];
            if i == 2 {
                // 叶子层（VPN[0]）
                return Some(unsafe { &mut *(entry as *mut PageTableEntry) });
            }
            if !entry.is_valid() || !entry.is_table() {
                let pa = alloc_page()?;
                let va = PhysAddr::from(pa).to_kernel_virt().0;
                unsafe { core::ptr::write_bytes(va as *mut u8, 0, 4096); }
                let new_ppn = PhysPageNum::from(pa >> PAGE_SHIFT);
                *entry = PageTableEntry::new_table(new_ppn);
                self.track_frame(new_ppn);
            }
            let next_ppn = entry.ppn();
            ppn = next_ppn;
        }
        None
    }

    /// 走到指定层级（0=根, 1=中间, 2=叶子）的 PTE。
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

            if !entry.is_valid() || !entry.is_table() {
                let pa = alloc_page()?;
                let va = PhysAddr::from(pa).to_kernel_virt().0;
                unsafe { core::ptr::write_bytes(va as *mut u8, 0, 4096); }
                let new_ppn = PhysPageNum::from(pa >> PAGE_SHIFT);
                *entry = PageTableEntry::new_table(new_ppn);
                self.track_frame(new_ppn);
            }
            ppn = entry.ppn();
        }
        None
    }

    /// 查找已存在的叶子 PTE，任一层级缺失则返回 `None`。
    pub fn find_pte(&self, vpn: VirtPageNum) -> Option<&mut PageTableEntry> {
        let idxs = vpn.indexes();
        let mut ppn = self.root_ppn;

        for (i, &idx) in idxs.iter().enumerate() {
            let table = unsafe { ppn.get_pte_array_mut() };
            let entry = &mut table[idx];
            if i == 2 {
                return Some(unsafe { &mut *(entry as *mut PageTableEntry) });
            }
            if !entry.is_valid() || !entry.is_table() {
                return None;
            }
            ppn = entry.ppn();
        }
        None
    }

    pub fn map(&mut self, vpn: VirtPageNum, ppn: PhysPageNum, flags: PTEFlags) {
        super::check_watch_pa(PhysAddr::from(ppn).0, "map");
        let pte = self.find_pte_create(vpn).expect("PageTable::map: out of memory");
        *pte = PageTableEntry::new(ppn, flags);
        tlb_invalidate_addr(VirtAddr::from(vpn));
    }

    pub fn remap(&mut self, vpn: VirtPageNum, flags: PTEFlags) {
        let pte = self.find_pte(vpn).expect("PageTable::remap: vpn not mapped");
        assert!(pte.is_valid(), "PageTable::remap: vpn not mapped");
        *pte = PageTableEntry::new(pte.ppn(), flags);
        tlb_invalidate_addr(VirtAddr::from(vpn));
    }

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
    pub fn translate_va_to_pa(&self, va: VirtAddr) -> Option<usize> {
        self.find_pte(VirtPageNum::from(va.floor())).map(|pte| {
            PhysAddr::from(pte.ppn()).0 + va.page_offset()
        })
    }

    pub fn translate_vpn_to_pte(&self, vpn: VirtPageNum) -> Option<PageTableEntry> {
        self.find_pte(vpn).map(|pte| *pte)
    }
}

// ---------------------------------------------------------------------------
// TLB maintenance
// ---------------------------------------------------------------------------

pub fn tlb_invalidate() {
    unsafe {
        core::arch::asm!("sfence.vma x0, x0");
    }
}

pub fn tlb_invalidate_addr(va: VirtAddr) {
    unsafe {
        core::arch::asm!("sfence.vma x0, {}", in(reg) va.0);
    }
}

/// 读取当前 satp（内核页表根）。
pub fn current_token() -> usize {
    let token: usize;
    unsafe {
        core::arch::asm!("csrr {}, satp", out(reg) token);
    }
    token
}
