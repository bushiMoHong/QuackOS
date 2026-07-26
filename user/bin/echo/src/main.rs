#![no_std]
#![no_main]

use core::arch::asm;
use core::ffi::CStr;

// Native microkernel syscall numbers (SVC #1)
const SYS_CONSOLE_WRITE: u64 = 11;
const SYS_EXIT_THREAD:    u64 = 7;

/// Write bytes directly to UART via native sys_console_write.
unsafe fn console_write(buf: *const u8, len: usize) -> isize {
    let ret: isize;
    unsafe {
        asm!(
            "svc #1",
            in("x8") SYS_CONSOLE_WRITE,
            in("x0") buf,
            in("x1") len,
            lateout("x0") ret,
        );
    }
    ret
}

/// Exit the current thread via native sys_exit_thread.
unsafe fn exit_thread(code: i32) -> ! {
    unsafe {
        asm!(
            "svc #1",
            in("x8") SYS_EXIT_THREAD,
            in("x0") code as u64,
            options(noreturn),
        );
    }
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

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    unsafe { exit_thread(1); }
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

    // Parse -n flag
    if argc > 1 {
        let ptr = unsafe { *argv.add(1) };
        if !ptr.is_null() {
            if unsafe { CStr::from_ptr(ptr) }.to_bytes() == b"-n" {
                newline = false;
                start = 2;
            }
        }
    }

    // Output arguments, space-separated
    for i in start..argc {
        if i > start {
            unsafe { console_write(b" ".as_ptr(), 1) };
        }
        let ptr = unsafe { *argv.add(i) };
        if !ptr.is_null() {
            let arg = unsafe { CStr::from_ptr(ptr) };
            let bytes = arg.to_bytes();
            unsafe { console_write(bytes.as_ptr(), bytes.len()) };
        }
    }

    if newline {
        unsafe { console_write(b"\n".as_ptr(), 1) };
    }

    unsafe { exit_thread(0); }
}
