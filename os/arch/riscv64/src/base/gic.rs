//! RISC-V PLIC（平台级中断控制器）驱动，针对 QEMU virt 平台。
//!
//! 与 aarch64 crate 的 `gic` 模块对应，提供等价的中断控制接口。

use core::ptr::{read_volatile, write_volatile};

// QEMU virt 平台的 PLIC 基地址
const PLIC_BASE: usize = 0x0C00_0000;
const PLIC_PRIORITY_BASE: usize = PLIC_BASE + 0x0000;
const PLIC_ENABLE_BASE: usize = PLIC_BASE + 0x2000; // context 0
const PLIC_THRESHOLD: usize = PLIC_BASE + 0x20_0000; // context 0
const PLIC_CLAIM: usize = PLIC_BASE + 0x20_0004; // context 0

/// 初始化 PLIC：把 context 0 的阈值设为 0（接受所有优先级的中断）。
pub unsafe fn gic_init() {
    write_volatile(PLIC_THRESHOLD as *mut u32, 0);
}

/// 使能一个外部中断源，并设置其优先级。
pub unsafe fn gic_enable_irq(irq: u32) {
    // 设置优先级
    write_volatile((PLIC_PRIORITY_BASE + irq as usize * 4) as *mut u32, 1);
    // 在 context 0 的使能位图中使能该中断
    let word = irq / 32;
    let bit = 1u32 << (irq % 32);
    let enable = read_volatile((PLIC_ENABLE_BASE + word as usize * 4) as *const u32);
    write_volatile((PLIC_ENABLE_BASE + word as usize * 4) as *mut u32, enable | bit);
}

/// 认领（claim）一个待处理的中断，返回中断号。0 表示无中断（spurious）。
pub unsafe fn gic_read_iar() -> u32 {
    read_volatile(PLIC_CLAIM as *const u32)
}

/// 完成（complete）中断处理。
pub unsafe fn gic_write_eoir(irq: u32) {
    write_volatile(PLIC_CLAIM as *mut u32, irq);
}

/// 从中断号中提取 IRQ 编号（PLIC claim 返回的就是中断号）。
#[inline]
pub fn iar_irq_num(irq: u32) -> u32 {
    irq
}

/// 是否为 spurious 中断（claim 返回 0）。
#[inline]
pub fn is_spurious(irq: u32) -> bool {
    irq == 0
}
