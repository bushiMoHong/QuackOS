//! RISC-V 定时器支持（通过 SBI `set_timer` + `time` CSR）。

#![allow(unused)]

use super::sbi::set_timer;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// 定时器中断频率（Hz）。
pub const TICKS_PER_SEC: usize = 100;

pub const MSEC_PER_SEC: usize = 1_000;
pub const USEC_PER_SEC: usize = 1_000_000;
pub const NSEC_PER_SEC: usize = 1_000_000_000;

/// QEMU virt 平台的 RISC-V 定时器频率为 10 MHz。
const CLOCK_FREQ: usize = 10_000_000;

// ---------------------------------------------------------------------------
// Raw access
// ---------------------------------------------------------------------------

/// 读取定时器频率（RISC-V 没有标准 CSR，返回硬编码的 QEMU virt 频率）。
#[inline]
pub fn get_cntfrq() -> usize {
    CLOCK_FREQ
}

/// 读取当前 `time` CSR 值（原始 tick 计数，未除以频率）。
#[inline]
pub fn get_time() -> usize {
    riscv::register::time::read()
}

/// 让定时器从现在起 `ticks` 个 tick 后触发（通过 SBI）。
#[inline]
pub fn set_timer_ticks(ticks: usize) {
    set_timer(get_time() + ticks);
}

/// 让定时器在绝对 counter 值 `cval` 时触发。
#[inline]
pub fn set_timer_cval(cval: usize) {
    set_timer(cval);
}

/// 使能定时器中断（设置 sie.STIE）。
#[inline]
pub fn enable_timer() {
    unsafe { riscv::register::sie::set_stimer(); }
}

/// 关闭定时器中断（清除 sie.STIE）。
#[inline]
pub fn disable_timer() {
    unsafe { riscv::register::sie::clear_stimer(); }
}

/// RISC-V 没有等价的控制寄存器，返回 0。
#[inline]
pub fn get_timer_ctl() -> usize {
    0
}

/// RISC-V 无法直接查询「是否已触发」，返回 false。
#[inline]
pub fn is_timer_fired() -> bool {
    false
}

// ---------------------------------------------------------------------------
// Time conversion helpers
// ---------------------------------------------------------------------------

pub fn get_time_ms() -> usize {
    get_time() / (get_cntfrq() / MSEC_PER_SEC)
}

pub fn get_time_us() -> usize {
    get_time() / (get_cntfrq() / USEC_PER_SEC)
}

pub fn get_time_ns() -> usize {
    get_time() * (NSEC_PER_SEC / get_cntfrq())
}

#[inline]
pub fn get_clock_freq() -> usize {
    get_cntfrq()
}

// ---------------------------------------------------------------------------
// Timer-interrupt scheduling
// ---------------------------------------------------------------------------

/// 调度下一次定时器中断（`TICKS_PER_SEC` Hz）。
pub fn set_next_trigger() {
    let freq = get_cntfrq();
    let ticks = freq / TICKS_PER_SEC;
    set_timer_ticks(ticks);
    enable_timer();
}

/// 在 `ns` 纳秒后调度下一次定时器中断。
pub fn set_next_trigger_ns(ns: usize) {
    let freq = get_cntfrq();
    let ticks = ns * freq / NSEC_PER_SEC;
    set_timer_ticks(ticks);
    enable_timer();
}
