//! Trap number constants and classification helpers.
//!
//! Each `trap_num` is a `usize` encoding:
//!
//!   trap_num = source | (kind << 16)
//!
//!   source ∈ {0, 1, 2, 3}  — see `ExceptionSource`
//!   kind   ∈ {0, 1, 2, 3}  — see `ExceptionKind`
//!
//! This module provides symbolic constants for the common patterns and
//! predicate functions that the high-level trap dispatcher uses.

use super::context::ExceptionKind;

// ---------------------------------------------------------------------------
// Individual trap_num values
// ---------------------------------------------------------------------------

// --- From lower EL (userspace), source = 2 ---------------------------------

/// Syscall: source = LowerAArch64 (2), kind = Synchronous (0)
pub const SYSCALL: usize = 0x00002;

/// Timer IRQ: source = LowerAArch64 (2), kind = IRQ (1)
pub const TIMER: usize = 0x10002;

/// The minimum IRQ trap_num from userspace.
pub const IRQ_MIN: usize = 0x10002;

/// The maximum IRQ trap_num from userspace (same as min — only one IRQ source).
pub const IRQ_MAX: usize = 0x10002;

// --- From current EL (kernel), source = 1 ------------------------------------

/// Kernel synchronous exception
pub const KERNEL_SYNC: usize = 0x00001;

/// Kernel IRQ
pub const KERNEL_IRQ: usize = 0x10001;

/// Kernel FIQ
pub const KERNEL_FIQ: usize = 0x20001;

/// Kernel SError
pub const KERNEL_SERROR: usize = 0x30001;

// ---------------------------------------------------------------------------
// Classification predicates
// ---------------------------------------------------------------------------

/// True if the trap_num corresponds to an SVC (syscall) from userspace.
#[inline]
pub fn is_syscall(trap_num: usize) -> bool {
    trap_num == SYSCALL
}

/// True if the trap_num is an IRQ from userspace.
#[inline]
pub fn is_intr(trap_num: usize) -> bool {
    trap_num >= IRQ_MIN && trap_num <= IRQ_MAX
}

/// True if the trap_num is the timer IRQ.
#[inline]
pub fn is_timer_intr(trap_num: usize) -> bool {
    trap_num == TIMER
}

/// Check whether a trap_num represents a page fault by decoding
/// the exception syndrome register (ESR_EL1).
///
/// A trap is a page fault if:
///   - It came from lower EL (source == 2)
///   - It was synchronous (kind == 0)
///   - ESR_EL1 decodes to a recoverable DataAbort or InstructionAbort
#[inline]
pub fn is_page_fault_safe(trap_num: usize, esr: u64) -> bool {
    // 必须来自低版本EL(2)且是同步异常(0) -> 即 0x00002
    if trap_num != SYSCALL {
        return false;
    }
    let syndrome = super::syndrome::Syndrome::from(esr);
    syndrome.is_page_fault()
}

/// True if this is a reserved-instruction trap.
/// On AArch64 there is no dedicated reserved-instruction exception;
/// this always returns false.
#[inline]
pub fn is_reserved_inst(_trap_num: usize) -> bool {
    false
}

/// Extract the exception source from trap_num (low 16 bits).
#[inline]
pub fn trap_source(trap_num: usize) -> usize {
    trap_num & 0xFFFF
}

/// Extract the exception kind from trap_num (high 16 bits).
#[inline]
pub fn trap_kind(trap_num: usize) -> usize {
    trap_num >> 16
}

/// Build a trap_num from source and kind.
#[inline]
pub const fn trap_num(source: usize, kind: ExceptionKind) -> usize {
    source | ((kind as usize) << 16)
}
