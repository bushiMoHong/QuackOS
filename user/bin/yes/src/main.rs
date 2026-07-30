#![no_std]
#![no_main]

use qlibc::{console, proc};

core::arch::global_asm!(
    ".globl _start",
    "_start:",
    "mov x29, #0",
    "mov x30, #0",
    "mov x2, sp",
    "and sp, x2, #-16",
    "ldr x0, [x2]",
    "add x1, x2, #8",
    "b   {yes_main}",
    yes_main = sym yes_main,
);

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    proc::exit(1);
}

#[unsafe(no_mangle)]
fn yes_main(argc: usize, argv: *const *const u8) -> ! {
    use core::ffi::CStr;

    // Determine the string to repeat
    let msg: &[u8] = if argc > 1 {
        let ptr = unsafe { *argv.add(1) };
        if !ptr.is_null() {
            unsafe { CStr::from_ptr(ptr) }.to_bytes()
        } else {
            b"y"
        }
    } else {
        b"y"
    };

    loop {
        console::write(msg);
        console::write(b"\n");
    }
}
