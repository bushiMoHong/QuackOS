//! Native microkernel syscall bindings via SVC #1.
//!
//! Each function maps to a syscall number in the kernel's native dispatch table.

use core::arch::asm;

// Syscall numbers (must match kernel's native.rs)
const SYS_MAP_PAGE:               u64 = 1;
const SYS_UNMAP_PAGE:             u64 = 2;
const SYS_IPC_SEND:               u64 = 3;
const SYS_IPC_RECV:               u64 = 4;
const SYS_IPC_CALL:               u64 = 5;
const SYS_CREATE_THREAD:          u64 = 6;
const SYS_EXIT_THREAD:            u64 = 7;
const SYS_REGISTER_LINUX_HANDLER: u64 = 8;
const SYS_LINUX_SYSCALL_DONE:     u64 = 9;
const SYS_YIELD:                  u64 = 10;

/// Issue a native (SVC #1) syscall with up to 4 arguments.
/// Returns x0.
#[inline(always)]
pub unsafe fn svc1(nr: u64, a0: usize, a1: usize, a2: usize, a3: usize) -> usize {
    let ret: usize;
    asm!(
        "svc #1",
        in("x8") nr,
        in("x0") a0, in("x1") a1, in("x2") a2, in("x3") a3,
        lateout("x0") ret,
    );
    ret
}

/// Issue a native (SVC #1) syscall with 5 arguments.
#[inline(always)]
pub unsafe fn svc1_5(nr: u64, a0: usize, a1: usize, a2: usize, a3: usize, a4: usize) -> usize {
    let ret: usize;
    asm!(
        "svc #1",
        in("x8") nr,
        in("x0") a0, in("x1") a1, in("x2") a2, in("x3") a3, in("x4") a4,
        lateout("x0") ret,
    );
    ret
}

// ---------------------------------------------------------------------------
// Public wrappers
// ---------------------------------------------------------------------------

pub unsafe fn map_page(vaddr: usize, prot: usize) -> isize {
    svc1(SYS_MAP_PAGE, vaddr, prot, 0, 0) as isize
}

pub unsafe fn unmap_page(vaddr: usize) -> isize {
    svc1(SYS_UNMAP_PAGE, vaddr, 0, 0, 0) as isize
}

pub unsafe fn ipc_send(ch: u32, msg_ptr: usize, msg_len: usize) -> isize {
    svc1(SYS_IPC_SEND, ch as usize, msg_ptr, msg_len, 0) as isize
}

pub unsafe fn ipc_recv(ch: u32, buf_ptr: usize, buf_len: usize) -> isize {
    svc1(SYS_IPC_RECV, ch as usize, buf_ptr, buf_len, 0) as isize
}

pub unsafe fn ipc_call(ch: u32, send_ptr: usize, send_len: usize, recv_buf: usize, recv_len: usize) -> isize {
    svc1_5(SYS_IPC_CALL, ch as usize, send_ptr, send_len, recv_buf, recv_len) as isize
}

pub unsafe fn create_thread(entry_pc: usize, stack_top: usize, arg: usize) -> isize {
    svc1(SYS_CREATE_THREAD, entry_pc, stack_top, arg, 0) as isize
}

pub unsafe fn exit_thread(code: usize) -> ! {
    svc1(SYS_EXIT_THREAD, code, 0, 0, 0);
    loop { asm!("wfi"); }
}

pub unsafe fn register_linux_handler(handler_pc: usize, save_area: usize) -> isize {
    svc1(SYS_REGISTER_LINUX_HANDLER, handler_pc, save_area, 0, 0) as isize
}

pub unsafe fn linux_syscall_done(ret_val: usize) -> ! {
    svc1(SYS_LINUX_SYSCALL_DONE, ret_val, 0, 0, 0);
    loop { asm!("wfi"); }
}

pub unsafe fn yield_cpu() {
    svc1(SYS_YIELD, 0, 0, 0, 0);
}
