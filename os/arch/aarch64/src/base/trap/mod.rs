//! AArch64 trap (exception / interrupt) handling module.
//!
//! # Overview
//!
//! This module provides a complete trap-handling subsystem for AArch64
//! bare-metal kernels:
//!
//! | Submodule       | Purpose                                                |
//! |-----------------|--------------------------------------------------------|
//! | `vector.S`      | Assembly: VBAR vector table, context save/restore      |
//! | `context`       | `TrapFrame`, `UserContext`, `GeneralRegs` data types   |
//! | `syndrome`      | ESR_EL1 decoding into `Syndrome` / `Fault`             |
//! | `consts`        | Trap-number constants and predicate helpers             |
//! | `handler`       | `TrapHandler` trait + C-ABI `trap_handler` entry point |
//! | `regs`          | System register read/write helpers                     |
//!
//! # Quick start
//!
//! ```ignore
//! use trap::{self, handler::{TrapHandler, set_trap_handler}};
//!
//! struct MyHandler;
//! impl TrapHandler for MyHandler { /* ... */ }
//!
//! // Early in kmain:
//! let handler: &'static dyn TrapHandler = &MyHandler;
//! unsafe {
//!     trap::init();                  // set VBAR_EL1
//!     set_trap_handler(handler);     // install dispatch callbacks
//!     trap::irq_enable();            // unmask IRQs
//! }
//!
//! // Enter userspace:
//! let mut ctx = UserContext::default();
//! ctx.elr = entry_point;
//! ctx.sp  = user_stack_top;
//! ctx.run();  // <-- returns after first trap
//! ```
//!
//! # File layout
//!
//! ```text
//! trap/
//! ├── mod.rs        ← you are here
//! ├── vector.S      ← assembly: vectors, __alltraps, run_user, trap_return
//! ├── context.rs    ← TrapFrame, UserContext, GeneralRegs, ExceptionSource/Kind
//! ├── syndrome.rs   ← ESR_EL1 → Syndrome, Fault
//! ├── consts.rs     ← SYSCALL, TIMER, IRQ_MIN/MAX, is_*() predicates
//! ├── handler.rs    ← TrapHandler trait, trap_handler entry, DefaultHandler
//! └── regs.rs       ← raw system-register access (mrs/msr wrappers)
//! ```

// Sub-modules
pub mod consts;
pub mod context;
pub mod handler;
pub mod regs;
pub mod syndrome;

// Re-export the most-used types so callers can write `trap::UserContext`.
pub use context::{ExceptionKind, ExceptionSource, GeneralRegs, TrapFrame, UserContext};
pub use handler::TrapHandler;
pub use syndrome::{Fault, Syndrome};

// Embed the assembly file.
use core::arch::global_asm;
global_asm!(include_str!("vector.S"));

// ---------------------------------------------------------------------------
// Initialisation
// ---------------------------------------------------------------------------

/// Initialise the exception vector table.
///
/// Writes the address of `__vectors` (from `vector.S`) into `VBAR_EL1`.
/// After this call, any exception will enter the kernel through the
/// vector table defined in this module.
///
/// # Safety
///
/// - Must be called exactly once per CPU core, after MMU is enabled.
/// - The vector table must reside in memory that is identity-mapped
///   (or at least accessible at EL1).
/// - Call `handler::set_trap_handler()` before enabling IRQs.
pub unsafe fn init() {
    extern "C" {
        fn __vectors();
    }
    core::arch::asm!("msr VBAR_EL1, {}", in(reg) __vectors as *const () as usize);

    // Initialise the GICv2 interrupt controller.
    crate::base::gic::gic_init();
}

// ---------------------------------------------------------------------------
// Interrupt control (thin wrappers around regs)
// ---------------------------------------------------------------------------

/// Enable IRQ (clear the I bit in DAIF).
///
/// # Safety
/// Call only after `init()` and `set_trap_handler()`.
#[inline]
pub unsafe fn irq_enable() {
    regs::irq_enable();
}

/// Disable IRQ (set the I bit in DAIF).
#[inline]
pub unsafe fn irq_disable() {
    regs::irq_disable();
}

/// Enable FIQ.
#[inline]
pub unsafe fn fiq_enable() {
    regs::fiq_enable();
}

/// Disable FIQ.
#[inline]
pub unsafe fn fiq_disable() {
    regs::fiq_disable();
}

/// Disable IRQ, return previous DAIF for later restore.
#[inline]
pub fn irq_disable_and_store() -> u64 {
    regs::irq_disable_and_store()
}

/// Restore DAIF from a previously saved value.
///
/// # Safety
/// The saved value must be a valid DAIF read from this same core.
#[inline]
pub unsafe fn irq_restore(flags: u64) {
    regs::daif_restore(flags);
}

/// Wait for interrupt (low-power standby until an IRQ/FIQ fires).
#[inline]
pub fn wait_for_interrupt() {
    let flags = regs::daif_read();
    unsafe { regs::irq_enable() };
    regs::wfe();
    unsafe { regs::daif_restore(flags) };
}
