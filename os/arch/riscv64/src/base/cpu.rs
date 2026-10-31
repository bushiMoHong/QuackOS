//! 架构相关的 CPU 原语（寄存器访问、屏障、TLB、页表根切换）。

use core::arch::asm;

/// 读取当前栈指针。
#[inline]
pub fn get_sp() -> usize {
    let sp: usize;
    unsafe { asm!("mv {}, sp", out(reg) sp) };
    sp
}

/// 读取当前线程 TCB 指针（RISC-V 用 s3 = x19 保存，与 aarch64 约定对齐）。
#[inline]
pub fn get_tcb_ptr() -> usize {
    let ptr: usize;
    unsafe { asm!("mv {}, x19", out(reg) ptr) };
    ptr
}

/// 读取 trap frame 指针（RISC-V 用 s4 = x20 保存）。
#[inline]
pub fn get_tf_ptr() -> usize {
    let ptr: usize;
    unsafe { asm!("mv {}, x20", out(reg) ptr) };
    ptr
}

/// 读取用户页表根物理地址（与 aarch64 的 ttbr 语义一致）。
#[inline]
pub fn get_page_table_root() -> usize {
    let satp: usize;
    unsafe { asm!("csrr {}, satp", out(reg) satp) };
    (satp & ((1 << 44) - 1)) << 12
}

/// 读取内核页表根物理地址（Sv39 只有一个根，内核/用户共用）。
#[inline]
pub fn get_kernel_page_table_root() -> usize {
    get_page_table_root()
}

/// 设置用户页表根（传入根页表 PA，内部转成 satp = MODE(Sv39=8) | PPN）并刷新 TLB。
#[inline]
pub fn set_page_table_root(root_pa: usize) {
    let satp = (8usize << 60) | (root_pa >> 12);
    unsafe { asm!("csrw satp, {}", in(reg) satp) };
    tlb_flush_all();
}

/// 内存屏障。
#[inline]
pub fn memory_barrier() {
    unsafe { asm!("fence") };
}

/// 刷新整个 TLB。
#[inline]
pub fn tlb_flush_all() {
    unsafe { asm!("sfence.vma x0, x0") };
}
