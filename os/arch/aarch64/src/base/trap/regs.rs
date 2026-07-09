//! Raw system-register access for AArch64.
//!
//! Thin wrappers around `mrs` / `msr` instructions for the registers used
//! by the trap dispatch path.  Kept in a separate module so the assembly
//! details don't leak into the higher-level trap logic.

use core::arch::asm;

// ---------------------------------------------------------------------------
// DAIF — interrupt mask bits
// ---------------------------------------------------------------------------

/// Enable IRQ (clear the I bit in DAIF).
///
/// # Safety
/// Call only after the trap handler is installed.
#[inline]
pub unsafe fn irq_enable() {
    asm!("msr daifclr, #2");
}

/// Disable IRQ (set the I bit in DAIF).
#[inline]
pub unsafe fn irq_disable() {
    asm!("msr daifset, #2");
}

/// Enable FIQ (clear the F bit in DAIF).
#[inline]
pub unsafe fn fiq_enable() {
    asm!("msr daifclr, #1");
}

/// Disable FIQ (set the F bit in DAIF).
#[inline]
pub unsafe fn fiq_disable() {
    asm!("msr daifset, #1");
}

/// Read the current DAIF value.
#[inline]
pub fn daif_read() -> u64 {
    let flags: u64;
    unsafe { asm!("mrs {}, daif", out(reg) flags) };
    flags
}

/// Disable IRQ and return the previous DAIF value for later restoration.
#[inline]
pub fn irq_disable_and_store() -> u64 {
    let flags = daif_read();
    unsafe { irq_disable() };
    flags
}

/// Restore DAIF from a previously saved value.
///
/// # Safety
/// The saved value must be a valid DAIF read from this same core.
#[inline]
pub unsafe fn daif_restore(flags: u64) {
    asm!("msr daif, {}", in(reg) flags);
}

/// Wait For Event — low-power standby until an IRQ or FIQ fires.
#[inline]
pub fn wfe() {
    unsafe { asm!("wfe") };
}

// ---------------------------------------------------------------------------
// FAR_EL1 — Fault Address Register
// ---------------------------------------------------------------------------

/// Read FAR_EL1 (the faulting virtual address for a synchronous abort).
#[inline]
pub fn far_el1_read() -> u64 {
    let far: u64;
    unsafe { asm!("mrs {}, far_el1", out(reg) far) };
    far
}

// ---------------------------------------------------------------------------
// ESR_EL1 — Exception Syndrome Register
// ---------------------------------------------------------------------------

/// Read ESR_EL1 (describes the reason for a synchronous exception).
#[inline]
pub fn esr_el1_read() -> u64 {
    let esr: u64;
    unsafe { asm!("mrs {}, esr_el1", out(reg) esr) };
    esr
}
