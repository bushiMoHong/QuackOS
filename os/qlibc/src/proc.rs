//! Process management syscalls.
//!
//! Wraps `sys_exit_thread` (nr=7), `sys_yield` (nr=10), `sys_clone` (nr=14),
//! `sys_exec` (nr=16), `sys_wait4` (nr=17).

use crate::syscall;

/// Exit the current thread immediately.
///
/// The kernel will mark this thread as Dying, wake any parent blocked on
/// wait4, and never schedule it again.  Does **not** return.
pub fn exit(code: i32) -> ! {
    unsafe {
        syscall::syscall1(syscall::SYS_EXIT_THREAD, code as usize);
    }
    // The syscall above does not return, but the compiler doesn't know that.
    #[allow(clippy::empty_loop)]
    loop {
        unsafe { core::arch::asm!("wfi"); }
    }
}

/// Yield the CPU to the scheduler.
///
/// The calling thread gives up its current time slice.  It will be
/// re-scheduled normally once it is the next runnable thread at its
/// priority level.
pub fn yield_now() {
    unsafe {
        syscall::syscall0(syscall::SYS_YIELD);
    }
}

/// Clone flags (matching Linux `clone(2)` ABI).
pub mod clone_flags {
    pub const CLONE_CHILD_SETTID: usize = 0x01000000;
    pub const CLONE_SETTLS:       usize = 0x00080000;
}

/// Clone the calling process.
///
/// Creates a child process that shares a **copy** of the parent's address
/// space.  Returns the child's thread-id in the parent; the child wakes up
/// with return value 0.
///
/// `flags` — see [`clone_flags`].  Pass 0 for a plain fork-like clone.
/// `child_sp` — new stack pointer for the child (0 uses the parent's sp).
/// `parent_tid` — pointer to write the child's TID in the parent's memory.
/// `child_tid` — pointer for CLONE_CHILD_SETTID.
/// `tls` — TPIDR_EL0 value for the child (used with CLONE_SETTLS).
pub fn clone(flags: usize, child_sp: usize, parent_tid: usize,
             child_tid: usize, tls: usize) -> Result<usize, isize> {
    let raw = unsafe {
        syscall::syscall5(syscall::SYS_CLONE, flags, child_sp,
                          parent_tid, child_tid, tls) as isize
    };
    syscall::to_result(raw)
}

/// Replace the current process image with a new ELF (execve semantics).
///
/// On success this function **does not return** — the kernel overwrites the
/// trap frame and eret's into the new program's entry point.
pub fn exec(elf_data: &[u8], stack_top: usize, bootinfo_ptr: usize) -> Result<(), isize> {
    let raw = unsafe {
        syscall::syscall5(syscall::SYS_EXEC,
                          elf_data.as_ptr() as usize,
                          elf_data.len(),
                          stack_top,
                          bootinfo_ptr,
                          0) as isize
    };
    if raw < 0 {
        Err(-raw)
    } else {
        // exec on success does not return — the kernel overwrites the trap
        // frame and eret's to the new program.  If we ever reach here the
        // kernel returned 0 (unlikely).
        Ok(())
    }
}

/// Wait for a child thread to exit.
///
/// Returns `(child_tid, exit_status)` on success, or `-errno` on error
/// (e.g. `-ECHILD` when there are no children).
///
/// The caller should check `child_tid >= 0` to distinguish success from
/// error.  The exit status is encoded as `(exit_code & 0xFF) << 8` (Linux
/// WEXITSTATUS convention).
pub fn wait4() -> Result<(usize, usize), isize> {
    // wait4 has two return values: x0 = child_tid (or -errno), x1 = status
    let x0: usize;
    let x1: usize;
    unsafe {
        core::arch::asm!(
            "svc #1",
            in("x8") syscall::SYS_WAIT4,
            out("x0") x0,
            out("x1") x1,
        );
    }
    let ret = x0 as isize;
    if ret < 0 {
        Err(-ret)
    } else {
        Ok((x0, x1))
    }
}
