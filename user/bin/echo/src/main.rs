#![no_std]
#![no_main]

use core::ffi::CStr;
use qlibc::{console, proc};

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

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    proc::exit(1);
}

#[unsafe(no_mangle)]
fn echo_main(argc: usize, argv: *const *const u8) -> ! {
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
            console::write(b" ");
        }
        let ptr = unsafe { *argv.add(i) };
        if !ptr.is_null() {
            let arg = unsafe { CStr::from_ptr(ptr) };
            console::write(arg.to_bytes());
        }
    }

    if newline {
        console::write(b"\n");
    }

    proc::exit(0);
}
