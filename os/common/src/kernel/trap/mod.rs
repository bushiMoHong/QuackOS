//! Common trap (exception / interrupt) handling.
//!
//! This module sits on top of the architecture-specific `aarch64::base::trap`
//! module and provides:
//!
//! | Item             | Purpose                                         |
//! |------------------|-------------------------------------------------|
//! | `TrapCause`      | Unified exception / interrupt cause enum        |
//! | `PageFaultCause` | LOAD / STORE / EXEC                             |
//! | `init()`         | Initialise the architecture trap subsystem      |
//! | `irq_enable()`   | Enable IRQs                                     |
//!
//! # Usage from a kernel binary
//!
//! ```ignore
//! use common::kernel::trap;
//!
//! unsafe {
//!     trap::init();
//!     trap::irq_enable();
//! }
//! ```

pub mod context;

pub use context::{ExceptionKind, ExceptionSource, GeneralRegs, TrapFrame, UserContext};

// Re-export arch functions that the kernel layer calls directly
pub use aarch64::base::trap::{
    fiq_disable, fiq_enable, irq_disable, irq_disable_and_store, irq_enable, irq_restore,
    wait_for_interrupt,
};
pub use aarch64::base::trap::handler::{
    install_default_handler, install_trap_handler, set_trap_fns, TrapHandler,
};
pub use aarch64::base::trap::syndrome::{Fault, Syndrome};

// ---------------------------------------------------------------------------
// PageFaultCause
// ---------------------------------------------------------------------------

/// Reason for a page fault.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageFaultCause {
    /// Fault on a load (read) access.
    Load,
    /// Fault on a store (write) access.
    Store,
    /// Fault on an instruction fetch.
    Exec,
}

// ---------------------------------------------------------------------------
// TrapCause — unified across architectures
// ---------------------------------------------------------------------------

/// Unified representation of the reason an exception / interrupt occurred.
///
/// Each architecture port maps its native cause register (ESR_EL1 on AArch64,
/// scause on RISC-V, EStat on LoongArch) into this enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrapCause {
    /// System call from user space.
    Syscall,
    /// Timer interrupt.
    TimerIrq,
    /// Page fault on load.
    PageFaultLoad,
    /// Page fault on store.
    PageFaultStore,
    /// Page fault on instruction fetch.
    PageFaultExec,
    /// Illegal instruction.
    IllegalInstruction,
    /// Breakpoint.
    Breakpoint,
    /// External / device interrupt (not timer).
    ExternalIrq,
    /// Alignment fault.
    AlignmentFault,
    /// A trap whose cause the common framework doesn't handle.
    Other(usize),
}

impl TrapCause {
    /// Convert a `TrapCause` into the corresponding `PageFaultCause`, if
    /// this cause is a page fault.
    pub fn as_page_fault(&self) -> Option<PageFaultCause> {
        match self {
            TrapCause::PageFaultLoad => Some(PageFaultCause::Load),
            TrapCause::PageFaultStore => Some(PageFaultCause::Store),
            TrapCause::PageFaultExec => Some(PageFaultCause::Exec),
            _ => None,
        }
    }

    /// Decode a `TrapCause` from an AArch64 ESR_EL1 register value.
    ///
    /// Call this from the architecture-specific trap handler entry point.
    pub fn from_aarch64_esr(esr: u64) -> Self {
        let ec = ((esr >> 26) & 0x3F) as usize;
        match ec {
            0b010101 => TrapCause::Syscall,             // SVC from AArch64
            0b100000 | 0b100001 => {
                // Data Abort (lower or same EL)
                let wnr = ((esr >> 6) & 1) != 0;        // WnR: 0=read, 1=write
                if wnr {
                    TrapCause::PageFaultStore
                } else {
                    TrapCause::PageFaultLoad
                }
            }
            0b100010 => TrapCause::PageFaultExec,       // Instruction Abort
            0b000000 | 0b000100 | 0b000101 => {
                // Unknown reason, breakpoint, etc.
                let iss = (esr & 0x1FF_FFFF) as usize;
                match iss {
                    0x00 => TrapCause::Breakpoint,
                    _ => TrapCause::Other(ec),
                }
            }
            0b111000 => TrapCause::IllegalInstruction,  // Trapped MSR/MRS
            _ => TrapCause::Other(ec),
        }
    }
}

// ---------------------------------------------------------------------------
// High-level entry points (call these from the arch trap handler)
// ---------------------------------------------------------------------------

/// Initialise the architecture's trap subsystem.
///
/// # Safety
/// Must be called exactly once per core, before enabling IRQs.
pub unsafe fn init() {
    aarch64::base::trap::init();
}
