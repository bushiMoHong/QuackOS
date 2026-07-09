//! AArch64 ARM Generic Timer support.
//!
//! # Overview
//!
//! AArch64 provides the ARM Generic Timer (also called the ARM Architected
//! Timer), accessible via system registers:
//!
//! | Register        | Purpose                                      |
//! |-----------------|----------------------------------------------|
//! | `CNTFRQ_EL0`    | Counter-timer Frequency (read-only)          |
//! | `CNTPCT_EL0`    | Physical counter value (read-only)           |
//! | `CNTP_CTL_EL0`  | Physical timer control (ENABLE/IMASK/ISTATUS)|
//! | `CNTP_TVAL_EL0` | Physical timer value (auto-decrementing)     |
//! | `CNTP_CVAL_EL0` | Physical timer compare value (absolute)      |
//!
//! # Timer interrupt
//!
//! The non-secure physical timer (PPI 30) fires when
//! `CNTPCT_EL0 >= CNTP_CVAL_EL0` or `CNTP_TVAL_EL0` hits zero.
//!
//! # Usage
//!
//! ```ignore
//! // During boot:
//! let freq = timer::get_cntfrq();
//!
//! // In timer IRQ handler:
//! timer::set_next_trigger();  // re-arm for TICKS_PER_SEC Hz
//! ```

#![allow(unused)]

use core::arch::asm;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Timer interrupt frequency (Hz).
pub const TICKS_PER_SEC: usize = 100;

pub const MSEC_PER_SEC: usize = 1_000;
pub const USEC_PER_SEC: usize = 1_000_000;
pub const NSEC_PER_SEC: usize = 1_000_000_000;

// ---------------------------------------------------------------------------
// Raw system-register access
// ---------------------------------------------------------------------------

/// Read the timer frequency from `CNTFRQ_EL0`.
///
/// On QEMU's `virt` machine this is typically 62_500_000 (62.5 MHz).
#[inline]
pub fn get_cntfrq() -> usize {
    let freq: usize;
    unsafe {
        asm!("mrs {}, cntfrq_el0", out(reg) freq);
    }
    freq
}

/// Read the current physical counter value from `CNTPCT_EL0`.
///
/// Returns the raw tick count — **not** divided by frequency.
#[inline]
pub fn get_time() -> usize {
    let count: usize;
    unsafe {
        asm!("mrs {}, cntpct_el0", out(reg) count);
    }
    count
}

/// Set the physical timer to fire after `ticks` from now.
///
/// Writes to `CNTP_TVAL_EL0`, which auto-decrements on each clock tick
/// and fires when it reaches zero.
#[inline]
pub fn set_timer_ticks(ticks: usize) {
    unsafe {
        asm!("msr cntp_tval_el0, {}", in(reg) ticks);
    }
}

/// Set the physical timer to fire at an absolute counter value.
///
/// Writes to `CNTP_CVAL_EL0`; the timer fires when `CNTPCT_EL0 >= cval`.
#[inline]
pub fn set_timer_cval(cval: usize) {
    unsafe {
        asm!("msr cntp_cval_el0, {}", in(reg) cval);
    }
}

/// Enable the physical timer and unmask its interrupt.
///
/// `CNTP_CTL_EL0` bit layout:
///   - bit 0: ENABLE  (1 = timer active)
///   - bit 1: IMASK   (0 = interrupt not masked)
///   - bit 2: ISTATUS (1 = timer condition met, read-only)
#[inline]
pub fn enable_timer() {
    // bit 0 = 1 (ENABLE), bit 1 = 0 (IMASK clear → interrupt unmasked)
    unsafe {
        asm!("msr cntp_ctl_el0, {}", in(reg) 1usize);
    }
}

/// Disable the physical timer.
#[inline]
pub fn disable_timer() {
    unsafe {
        asm!("msr cntp_ctl_el0, {}", in(reg) 0usize);
    }
}

/// Read `CNTP_CTL_EL0`.
///
/// Returns the raw control register value: `(ISTATUS << 2) | (IMASK << 1) | ENABLE`.
#[inline]
pub fn get_timer_ctl() -> usize {
    let ctl: usize;
    unsafe {
        asm!("mrs {}, cntp_ctl_el0", out(reg) ctl);
    }
    ctl
}

/// Returns `true` if the timer has fired (ISTATUS bit is set).
#[inline]
pub fn is_timer_fired() -> bool {
    (get_timer_ctl() & 0b100) != 0
}

// ---------------------------------------------------------------------------
// Time conversion helpers
// ---------------------------------------------------------------------------

/// Return current time in milliseconds (since boot).
pub fn get_time_ms() -> usize {
    let freq = get_cntfrq();
    get_time() / (freq / MSEC_PER_SEC)
}

/// Return current time in microseconds (since boot).
pub fn get_time_us() -> usize {
    let freq = get_cntfrq();
    get_time() / (freq / USEC_PER_SEC)
}

/// Return current time in nanoseconds (since boot).
pub fn get_time_ns() -> usize {
    let freq = get_cntfrq();
    get_time() * (NSEC_PER_SEC / freq)
}

/// Alias for `get_cntfrq()` — used by trap timer logic.
#[inline]
pub fn get_clock_freq() -> usize {
    get_cntfrq()
}

// ---------------------------------------------------------------------------
// Timer-interrupt scheduling
// ---------------------------------------------------------------------------

/// Schedule the next timer interrupt at `TICKS_PER_SEC` Hz.
///
/// Call this from the timer IRQ handler to re-arm the timer.
/// Uses `CNTP_TVAL_EL0` for a one-shot fire.
pub fn set_next_trigger() {
    let freq = get_cntfrq();
    let ticks = freq / TICKS_PER_SEC;
    set_timer_ticks(ticks);
    enable_timer();
}

/// Schedule the next timer interrupt after `ns` nanoseconds.
///
/// Useful when the kernel wants a custom sleep / timeout interval.
pub fn set_next_trigger_ns(ns: usize) {
    let freq = get_cntfrq();
    let ticks = ns * freq / NSEC_PER_SEC;
    set_timer_ticks(ticks);
    enable_timer();
}
