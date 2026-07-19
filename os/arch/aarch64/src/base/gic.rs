//! GICv2 (Generic Interrupt Controller v2) driver for AArch64.
//!
//! Provides raw MMIO register access for the distributor (GICD) and CPU
//! interface (GICC).  Designed for QEMU `virt` machine where GICD is at
//! `0x0800_0000` and GICC at `0x0801_0000`.
//!
//! All functions are `unsafe` — the caller must ensure the GIC is
//! identity-mapped and IRQs are properly set up before enabling them.

use core::ptr::{read_volatile, write_volatile};

// ---------------------------------------------------------------------------
// MMIO base addresses (QEMU virt)
// ---------------------------------------------------------------------------

const GICD_BASE: usize = 0x0800_0000;
const GICC_BASE: usize = 0x0801_0000;

// ---------------------------------------------------------------------------
// Distributor registers (GICD)
// ---------------------------------------------------------------------------

const GICD_CTLR:     *mut u32 = (GICD_BASE + 0x000) as *mut u32;
// GICD_TYPER not used — read-only, for informational purposes
const GICD_ISENABLER: usize   = GICD_BASE + 0x100;  // base; offset by (irq / 32) * 4
const GICD_ITARGETSR: usize   = GICD_BASE + 0x800;  // base; offset by irq (byte per IRQ)

// ---------------------------------------------------------------------------
// CPU interface registers (GICC)
// ---------------------------------------------------------------------------

const GICC_CTLR: *mut u32 = (GICC_BASE + 0x0000) as *mut u32;
const GICC_PMR:  *mut u32 = (GICC_BASE + 0x0004) as *mut u32;
const GICC_IAR:  *const u32 = (GICC_BASE + 0x000C) as *const u32;
const GICC_EOIR: *mut u32 = (GICC_BASE + 0x0010) as *mut u32;

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Initialise the GICv2 distributor and CPU interface.
///
/// After this call the GIC is ready to accept per-IRQ configuration,
/// but individual IRQ lines are still disabled until `gic_enable_irq`
/// is called for each one.
pub unsafe fn gic_init() {
    // Enable distributor (bit 0 = Enable)
    write_volatile(GICD_CTLR, 1);

    // Set priority mask to lowest (0xFF) — accept all priorities
    write_volatile(GICC_PMR, 0xFF);

    // Enable CPU interface (bit 0 = Enable)
    write_volatile(GICC_CTLR, 1);
}

/// Enable a specific SPI (Shared Peripheral Interrupt) and route it to CPU 0.
///
/// # Panics
/// Panics if `irq < 32` — PPIs and SGIs are not supported through this API.
pub unsafe fn gic_enable_irq(irq: u32) {
    assert!(irq >= 32, "gic_enable_irq: only SPIs (>=32) are supported, got IRQ {}", irq);

    // Set-enable: GICD_ISENABLER[n] at offset 0x100 + (irq/32)*4
    let word_off = (irq / 32) as usize * 4;
    let bit = 1u32 << (irq % 32);
    let isenabler = (GICD_ISENABLER + word_off) as *mut u32;
    write_volatile(isenabler, bit);

    // Target CPU: GICD_ITARGETSR[n] at offset 0x800 + irq (byte access)
    let itargetsr = (GICD_ITARGETSR + irq as usize) as *mut u8;
    write_volatile(itargetsr, 0x01); // route to CPU 0
}

/// Read the Interrupt Acknowledge Register.
///
/// Returns the raw 32-bit value.  Bits [9:0] contain the interrupt ID.
/// A value of 1023 means a spurious interrupt (no IRQ pending).
pub unsafe fn gic_read_iar() -> u32 {
    read_volatile(GICC_IAR)
}

/// Write the End-Of-Interrupt register.
///
/// `irq` should be the interrupt ID extracted from IAR (bits [9:0]).
pub unsafe fn gic_write_eoir(irq: u32) {
    write_volatile(GICC_EOIR, irq);
}

/// Extract the interrupt ID from a raw IAR value.
#[inline]
pub fn iar_irq_num(iar: u32) -> u32 {
    iar & 0x3FF // bits [9:0]
}

/// Return `true` if the IAR value indicates a spurious interrupt.
#[inline]
pub fn is_spurious(iar: u32) -> bool {
    iar == 1023
}
