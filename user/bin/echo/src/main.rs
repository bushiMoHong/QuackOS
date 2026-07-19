#![no_std]
#![no_main]

use core::arch::asm;
use core::ffi::CStr;

const SYS_WRITE: u64 = 64;
const SYS_EXIT: u64 = 93;
const STDOUT: u64 = 1;

unsafe fn sys_write(fd: u64, buf: *const u8, len: usize) -> isize {
    let ret: isize;
    unsafe {
        asm!(
            "svc #0",
            in("x8") SYS_WRITE,
            in("x0") fd,
            in("x1") buf,
            in("x2") len,
            lateout("x0") ret,
        );
    }
    ret
}

unsafe fn sys_exit(code: i32) -> ! {
    unsafe {
        asm!(
            "svc #0",
            in("x8") SYS_EXIT,
            in("x0") code as u64,
            options(noreturn),
        );
    }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    unsafe { sys_exit(1); }
}

// strlen stub — the compiler may call this when optimizing CStr::to_bytes()
#[unsafe(no_mangle)]
unsafe extern "C" fn strlen(s: *const u8) -> usize {
    let mut len = 0;
    unsafe {
        while *s.add(len) != 0 {
            len += 1;
        }
    }
    len
}

core::arch::global_asm!(
    ".globl _start",
    "_start:",
    "mov x29, #0",
    "mov x30, #0",
    "mov x2, sp",
    "and sp, x2, #-16",
    "ldr x0, [x2]",          // argc from orig sp
    "add x1, x2, #8",        // argv from orig sp
    "b   {echo_main}",
    echo_main = sym echo_main,
);

#[unsafe(no_mangle)]
unsafe fn echo_main(argc: usize, argv: *const *const u8) -> ! {
    let mut newline = true;
    let mut start: usize = 1;

    if argc > 1 {
        let ptr = unsafe { *argv.add(1) };
        if !ptr.is_null() {
            if unsafe { CStr::from_ptr(ptr) }.to_bytes() == b"-n" {
                newline = false;
                start = 2;
            }
        }
    }

    for i in start..argc {
        if i > start {
            unsafe { sys_write(STDOUT, b" ".as_ptr(), 1) };
        }
        let ptr = unsafe { *argv.add(i) };
        if !ptr.is_null() {
            let arg = unsafe { CStr::from_ptr(ptr) };
            let bytes = arg.to_bytes();
            unsafe { sys_write(STDOUT, bytes.as_ptr(), bytes.len()) };
        }
    }

    if newline {
        unsafe { sys_write(STDOUT, b"\n".as_ptr(), 1) };
    }

    unsafe { sys_exit(0); }
}
