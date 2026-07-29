//! Raw `svc #1` syscall bindings.
//!
//! **This is the ONLY `unsafe` module in qlibc.**  Every other module calls
//! these helpers to cross the kernel boundary, then wraps the result in a
//! safe API.

use core::arch::asm;

// Syscall numbers (must match kernel native dispatch table)
pub(crate) const SYS_MAP_PAGE:              u64 = 1;
pub(crate) const SYS_UNMAP_PAGE:            u64 = 2;
pub(crate) const SYS_IPC_SEND:              u64 = 3;
pub(crate) const SYS_IPC_RECV:              u64 = 4;
pub(crate) const SYS_IPC_CALL:              u64 = 5;
pub(crate) const SYS_CREATE_THREAD:         u64 = 6;
pub(crate) const SYS_EXIT_THREAD:           u64 = 7;
pub(crate) const SYS_REGISTER_LINUX_HANDLER: u64 = 8;
pub(crate) const SYS_LINUX_SYSCALL_DONE:    u64 = 9;
pub(crate) const SYS_YIELD:                 u64 = 10;
pub(crate) const SYS_CONSOLE_WRITE:         u64 = 11;
pub(crate) const SYS_MPROTECT:             u64 = 12;
pub(crate) const SYS_SPAWN:                u64 = 13;
pub(crate) const SYS_CLONE:                u64 = 14;
pub(crate) const SYS_CONSOLE_READ:         u64 = 15;
pub(crate) const SYS_EXEC:                 u64 = 16;
pub(crate) const SYS_WAIT4:                u64 = 17;
pub(crate) const SYS_CREATE_NOTIFICATION:  u64 = 18;
pub(crate) const SYS_NOTIFY_SEND:          u64 = 19;
pub(crate) const SYS_NOTIFY_WAIT:          u64 = 20;
pub(crate) const SYS_IRQ_REGISTER:         u64 = 21;
pub(crate) const SYS_IRQ_ACK:              u64 = 22;
pub(crate) const SYS_IPC_RECV_TIMEOUT:     u64 = 23;
pub(crate) const SYS_IPC_CALL_TIMEOUT:     u64 = 24;

// ---------------------------------------------------------------------------
// Generic syscall helpers (0–6 arguments)
// ---------------------------------------------------------------------------

#[inline(always)]
pub(crate) unsafe fn syscall0(nr: u64) -> usize {
    let ret: usize;
    unsafe {
        asm!("svc #1", in("x8") nr, lateout("x0") ret);
    }
    ret
}

#[inline(always)]
pub(crate) unsafe fn syscall1(nr: u64, a0: usize) -> usize {
    let ret: usize;
    unsafe {
        asm!("svc #1", in("x8") nr, in("x0") a0, lateout("x0") ret);
    }
    ret
}

#[inline(always)]
pub(crate) unsafe fn syscall2(nr: u64, a0: usize, a1: usize) -> usize {
    let ret: usize;
    unsafe {
        asm!("svc #1", in("x8") nr, in("x0") a0, in("x1") a1,
             lateout("x0") ret);
    }
    ret
}

#[inline(always)]
pub(crate) unsafe fn syscall3(nr: u64, a0: usize, a1: usize, a2: usize) -> usize {
    let ret: usize;
    unsafe {
        asm!("svc #1", in("x8") nr, in("x0") a0, in("x1") a1,
             in("x2") a2, lateout("x0") ret);
    }
    ret
}

#[inline(always)]
pub(crate) unsafe fn syscall4(nr: u64, a0: usize, a1: usize, a2: usize,
                              a3: usize) -> usize {
    let ret: usize;
    unsafe {
        asm!("svc #1", in("x8") nr, in("x0") a0, in("x1") a1,
             in("x2") a2, in("x3") a3, lateout("x0") ret);
    }
    ret
}

#[inline(always)]
pub(crate) unsafe fn syscall5(nr: u64, a0: usize, a1: usize, a2: usize,
                              a3: usize, a4: usize) -> usize {
    let ret: usize;
    unsafe {
        asm!("svc #1", in("x8") nr, in("x0") a0, in("x1") a1,
             in("x2") a2, in("x3") a3, in("x4") a4,
             lateout("x0") ret);
    }
    ret
}

#[inline(always)]
pub(crate) unsafe fn syscall6(nr: u64, a0: usize, a1: usize, a2: usize,
                              a3: usize, a4: usize, a5: usize) -> usize {
    let ret: usize;
    unsafe {
        asm!("svc #1", in("x8") nr, in("x0") a0, in("x1") a1,
             in("x2") a2, in("x3") a3, in("x4") a4, in("x5") a5,
             lateout("x0") ret);
    }
    ret
}

/// Convert a raw isize return to Result<usize, isize>.
/// Returns ≥ 0 → Ok(value), < 0 → Err(-value) (positive errno).
#[inline(always)]
pub(crate) fn to_result(raw: isize) -> Result<usize, isize> {
    if raw >= 0 { Ok(raw as usize) } else { Err(-raw) }
}
