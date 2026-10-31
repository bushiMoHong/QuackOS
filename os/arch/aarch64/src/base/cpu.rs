//! 架构相关的 CPU 原语（寄存器访问、屏障、TLB、页表根切换）。

use core::arch::asm;

/// 读取当前栈指针。
#[inline]
pub fn get_sp() -> usize {
    let sp: usize;
    unsafe { asm!("mov {}, sp", out(reg) sp) };
    sp
}

/// 读取当前线程 TCB 指针（AArch64 用 x19 保存）。
#[inline]
pub fn get_tcb_ptr() -> usize {
    let ptr: usize;
    unsafe { asm!("mov {}, x19", out(reg) ptr) };
    ptr
}

/// 读取 trap frame 指针（AArch64 用 x20 保存，由 thread 创建时写入）。
#[inline]
pub fn get_tf_ptr() -> usize {
    let ptr: usize;
    unsafe { asm!("mov {}, x20", out(reg) ptr) };
    ptr
}

/// 读取用户页表根（TTBR0_EL1）。
#[inline]
pub fn get_page_table_root() -> usize {
    let root: usize;
    unsafe { asm!("mrs {}, ttbr0_el1", out(reg) root) };
    root
}

/// 读取内核页表根（TTBR1_EL1）。
#[inline]
pub fn get_kernel_page_table_root() -> usize {
    let root: usize;
    unsafe { asm!("mrs {}, ttbr1_el1", out(reg) root) };
    root
}

/// 设置用户页表根（TTBR0_EL1）并刷新 TLB。
#[inline]
pub fn set_page_table_root(root: usize) {
    unsafe { asm!("msr ttbr0_el1, {}", in(reg) root) };
    tlb_flush_all();
}

/// 内存屏障（数据同步屏障）。
#[inline]
pub fn memory_barrier() {
    unsafe { asm!("dsb ishst") };
}

/// 刷新整个 TLB。
#[inline]
pub fn tlb_flush_all() {
    unsafe { asm!("dsb ish; tlbi vmalle1is; dsb ish; isb") };
}
