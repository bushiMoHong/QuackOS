//! RISC-V trap（异常 / 中断）处理模块。接口与 aarch64 crate 一致。

pub mod consts;
pub mod context;
pub mod handler;
pub mod regs;
pub mod syndrome;

pub use context::{ExceptionKind, ExceptionSource, GeneralRegs, TrapFrame, UserContext};
pub use handler::TrapHandler;
pub use syndrome::{Fault, Syndrome};

use core::arch::global_asm;
global_asm!(include_str!("trap.S"));

// ---------------------------------------------------------------------------
// Initialisation
// ---------------------------------------------------------------------------

/// 初始化异常向量表（写 stvec）并初始化 PLIC。
pub unsafe fn init() {
    extern "C" {
        fn __trap_from_user();
    }
    // 写入 stvec（低 2 位为 0 表示 direct 模式）
    core::arch::asm!("csrw stvec, {}", in(reg) __trap_from_user as *const () as usize);

    crate::base::gic::gic_init();
}

// ---------------------------------------------------------------------------
// Interrupt control
// ---------------------------------------------------------------------------

#[inline]
pub unsafe fn irq_enable() {
    regs::irq_enable();
}

#[inline]
pub unsafe fn irq_disable() {
    regs::irq_disable();
}

#[inline]
pub unsafe fn fiq_enable() {
    regs::fiq_enable();
}

#[inline]
pub unsafe fn fiq_disable() {
    regs::fiq_disable();
}

#[inline]
pub fn irq_disable_and_store() -> u64 {
    regs::irq_disable_and_store()
}

#[inline]
pub unsafe fn irq_restore(flags: u64) {
    regs::daif_restore(flags);
}

#[inline]
pub fn wait_for_interrupt() {
    regs::wfe();
}
