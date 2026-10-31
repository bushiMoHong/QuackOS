//! RISC-V physical memory management.
//!
//! 提供与 aarch64 crate 完全一致的接口：
//! - `PhysAddr` / `VirtAddr` / `PhysPageNum` / `VirtPageNum` 地址类型
//! - 基于空闲链表的物理页分配器：`alloc_page` / `free_page` / `free_page_range` 等
//! - `page_table` 子模块（Sv39 页表）

use super::config::{PAGE_OFFSET, PAGE_SHIFT, PAGE_SIZE};
use core::fmt;
use core::ptr;
use spin::Mutex;

// ---------------------------------------------------------------------------
// PA watch — 检测对被监视物理页范围的操作（调试内核栈损坏 / 别名 bug）
// ---------------------------------------------------------------------------

const UART_DR: *mut u8 = 0x10000000 as *mut u8; // QEMU virt UART0

fn uart_putc(c: u8) {
    unsafe { ptr::write_volatile(UART_DR, c); }
}

fn uart_puts(s: &str) {
    for &b in s.as_bytes() {
        uart_putc(b);
    }
}

fn uart_puthex(mut v: u64) {
    uart_puts("0x");
    if v == 0 {
        uart_putc(b'0');
        return;
    }
    let mut started = false;
    for shift in (0..16).rev() {
        let nibble = ((v >> (shift * 4)) & 0xF) as u8;
        if nibble != 0 || started {
            started = true;
            uart_putc(if nibble < 10 { b'0' + nibble } else { b'a' + (nibble - 10) });
        }
    }
}

struct WatchRange {
    start: usize,
    end: usize,
}

const EMPTY_WATCH: Option<WatchRange> = None;
static WATCH_RANGES: Mutex<[Option<WatchRange>; 4]> = Mutex::new([EMPTY_WATCH; 4]);

/// 注册一个要监视的物理地址范围。
pub fn watch_pa_range(start: usize, end: usize) {
    let mut slots = WATCH_RANGES.lock();
    for slot in slots.iter_mut() {
        if slot.is_none() {
            *slot = Some(WatchRange { start, end });
            return;
        }
    }
    uart_puts("[WATCH] no free slot!\n");
}

/// 检查 `pa` 是否落在被监视范围内。
pub fn check_watch_pa(pa: usize, op: &str) {
    let slots = WATCH_RANGES.lock();
    for (i, slot) in slots.iter().enumerate() {
        if let Some(ref r) = *slot {
            let page_start = pa & !(PAGE_SIZE - 1);
            if page_start >= r.start && page_start < r.end {
                uart_puts("\n[WATCH#");
                uart_puthex(i as u64);
                uart_puts("] ");
                uart_puts(op);
                uart_puts(" pa=");
                uart_puthex(pa as u64);
                uart_puts(" range=[");
                uart_puthex(r.start as u64);
                uart_puts(",");
                uart_puthex(r.end as u64);
                uart_puts(")\n");
            }
        }
    }
}

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
    pub fn floor(&self) -> VirtAddr {
        VirtAddr(self.0 & !(PAGE_SIZE - 1))
    }

    pub fn ceil(&self) -> VirtAddr {
        VirtAddr((self.0 + PAGE_SIZE - 1) & !(PAGE_SIZE - 1))
    }

    pub fn page_offset(&self) -> usize {
        self.0 & (PAGE_SIZE - 1)
    }

    /// 通过恒等映射转换到物理地址（virt == phys）。
    pub fn to_phys(&self) -> Option<PhysAddr> {
        Some(PhysAddr(self.0))
    }
}

impl PhysPageNum {
    pub unsafe fn get_pte_array(&self) -> &[page_table::PageTableEntry; 512] {
        &*(PhysAddr::from(*self).to_kernel_virt().0 as *const [page_table::PageTableEntry; 512])
    }

    pub unsafe fn get_pte_array_mut(&mut self) -> &mut [page_table::PageTableEntry; 512] {
        &mut *(PhysAddr::from(*self).to_kernel_virt().0 as *mut [page_table::PageTableEntry; 512])
    }
}

impl PhysAddr {
    /// 通过恒等映射转换到内核虚拟地址。
    pub fn to_kernel_virt(&self) -> VirtAddr {
        VirtAddr(self.0)
    }

    pub unsafe fn as_pte_array_mut(&self) -> &mut [usize; 512] {
        &mut *(self.to_kernel_virt().0 as *mut [usize; 512])
    }

    pub unsafe fn as_pte_array(&self) -> &[usize; 512] {
        &*(self.to_kernel_virt().0 as *const [usize; 512])
    }
}

impl VirtPageNum {
    /// 返回 Sv39 三级页表的索引：`[vpn2, vpn1, vpn0]`。
    pub fn indexes(&self) -> [usize; 3] {
        let vpn = self.0;
        [
            (vpn >> 18) & 0x1FF, // VPN[2]（根）
            (vpn >> 9) & 0x1FF,  // VPN[1]
            vpn & 0x1FF,         // VPN[0]（叶子）
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
// RISC-V Sv39 页表项（PTE）标志
// ---------------------------------------------------------------------------

pub mod pte {
    // PTE 标志位（低 10 位）
    pub const V: usize = 1 << 0; // Valid
    pub const R: usize = 1 << 1; // Read
    pub const W: usize = 1 << 2; // Write
    pub const X: usize = 1 << 3; // Execute
    pub const U: usize = 1 << 4; // User
    pub const G: usize = 1 << 5; // Global
    pub const A: usize = 1 << 6; // Accessed
    pub const D: usize = 1 << 7; // Dirty

    /// 构建指向下一级页表的描述符（V=1, R=W=X=0）。
    #[inline]
    pub fn table_descriptor(next_ppn: usize) -> usize {
        (next_ppn << 10) | V
    }

    /// 构建一个叶子页描述符（有效 + 可读 + 可写 + 可执行 + Accessed + Dirty）。
    #[inline]
    pub fn leaf_descriptor(output_ppn: usize) -> usize {
        (output_ppn << 10) | V | R | W | X | A | D
    }
}

// ---------------------------------------------------------------------------
// Global page allocator
// ---------------------------------------------------------------------------

struct FreePage {
    next: *mut FreePage,
}

struct FreeListPtr(*mut FreePage);
unsafe impl Send for FreeListPtr {}

static FREE_LIST: Mutex<FreeListPtr> = Mutex::new(FreeListPtr(core::ptr::null_mut()));
static FREE_COUNT: Mutex<usize> = Mutex::new(0);

// ---------------------------------------------------------------------------
// Page tracking bitmap（检测 double-free / double-alloc）
// ---------------------------------------------------------------------------

static TRACK_BASE: Mutex<usize> = Mutex::new(0);
static TRACK_BITMAP: Mutex<[u64; 2048]> = Mutex::new([0u64; 2048]);

pub fn is_page_allocated(pa: usize) -> bool {
    let base = *TRACK_BASE.lock();
    if base == 0 || pa < base { return true; }
    let idx = (pa - base) / PAGE_SIZE;
    if idx >= 2048 * 64 { return true; }
    let word = idx / 64;
    let bit = idx % 64;
    let bitmap = TRACK_BITMAP.lock();
    (bitmap[word] >> bit) & 1 != 0
}

pub fn init_page_tracking(phys_base: usize) {
    *TRACK_BASE.lock() = phys_base;
    let mut bitmap = TRACK_BITMAP.lock();
    for word in bitmap.iter_mut() {
        *word = u64::MAX;
    }
}

fn track_mark(pa: usize, allocated: bool) {
    let base = *TRACK_BASE.lock();
    if base == 0 { return; }
    if pa < base { return; }
    let idx = (pa - base) / PAGE_SIZE;
    if idx >= 2048 * 64 { return; }
    let word = idx / 64;
    let bit = idx % 64;
    let mut bitmap = TRACK_BITMAP.lock();
    let was_set = (bitmap[word] >> bit) & 1 != 0;
    if allocated {
        if was_set {
            uart_puts("\n*** DOUBLE-ALLOC at PA=0x");
            uart_puthex(pa as u64);
            uart_puts(" ***\n");
            loop { unsafe { core::arch::asm!("wfi"); } }
        }
        bitmap[word] |= 1u64 << bit;
    } else {
        if !was_set {
            uart_puts("\n*** DOUBLE-FREE at PA=0x");
            uart_puthex(pa as u64);
            uart_puts(" ***\n");
            loop { unsafe { core::arch::asm!("wfi"); } }
        }
        bitmap[word] &= !(1u64 << bit);
    }
}

/// 分配一页（清零）。耗尽时返回 `None`。
pub fn alloc_page() -> Option<usize> {
    let mut list = FREE_LIST.lock();
    let head = list.0;
    if head.is_null() {
        return None;
    }
    unsafe {
        list.0 = (*head).next;
        *FREE_COUNT.lock() -= 1;

        let ptr = head as *mut u8;
        ptr::write_bytes(ptr, 0, PAGE_SIZE);

        let pa = head as usize;
        drop(list);
        track_mark(pa, true);
        check_watch_pa(pa, "alloc_page");
        Some(pa)
    }
}

pub fn free_page(phys_addr: usize) {
    let addr = phys_addr & !(PAGE_SIZE - 1);
    track_mark(addr, false);
    check_watch_pa(addr, "free_page");
    let mut list = FREE_LIST.lock();
    unsafe {
        let node = addr as *mut FreePage;
        (*node).next = list.0;
        list.0 = node;
        *FREE_COUNT.lock() += 1;
    }
}

/// 分配 `count` 页物理连续（清零），返回基址 PA。
pub fn alloc_pages_contig(count: usize) -> Option<usize> {
    if count == 0 || count > 8 {
        return None;
    }
    if count == 1 {
        return alloc_page();
    }

    let mut held = [0usize; 128];
    let mut n = 0;

    while n < held.len() {
        match alloc_page() {
            Some(p) => { held[n] = p; n += 1; }
            None => break,
        }
        if n < count {
            continue;
        }
        held[..n].sort_unstable();
        let mut run = 1;
        for i in 1..n {
            if held[i] == held[i - 1] + PAGE_SIZE { run += 1; } else { run = 1; }
            if run == count {
                let start_idx = i + 1 - count;
                for (j, &p) in held[..n].iter().enumerate() {
                    if j < start_idx || j > i {
                        free_page(p);
                    }
                }
                return Some(held[start_idx]);
            }
        }
    }

    for &p in &held[..n] {
        free_page(p);
    }
    None
}

/// 把物理内存区间 `[start, end)` 加入空闲链表。
pub fn free_page_range(start: usize, end: usize) {
    let start = (start + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
    let end = end & !(PAGE_SIZE - 1);

    let mut addr = start;
    while addr + PAGE_SIZE <= end {
        free_page(addr);
        addr += PAGE_SIZE;
    }
}

pub fn free_page_count() -> usize {
    *FREE_COUNT.lock()
}

// 让 `PAGE_OFFSET` 在此模块内保持可用（接口一致性），避免 unused 警告。
#[allow(unused)]
pub(crate) fn _page_offset() -> usize {
    PAGE_OFFSET
}
