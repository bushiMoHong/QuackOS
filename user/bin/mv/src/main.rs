#![no_std]
#![no_main]

use core::ffi::CStr;
use qlibc::{console, fs, proc};

core::arch::global_asm!(
    ".globl _start",
    "_start:",
    "mov x29, #0",
    "mov x30, #0",
    "mov x2, sp",
    "and sp, x2, #-16",
    "ldr x0, [x2]",
    "add x1, x2, #8",
    "b   {mv_main}",
    mv_main = sym mv_main,
);

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    proc::exit(1);
}

fn arg(argv: *const *const u8, idx: usize) -> &'static str {
    let ptr = unsafe { *argv.add(idx) };
    if ptr.is_null() { "" } else {
        unsafe { CStr::from_ptr(ptr) }.to_str().unwrap_or("")
    }
}

#[unsafe(no_mangle)]
fn mv_main(_argc: usize, argv: *const *const u8) -> ! {
    let src = arg(argv, 1);
    let dst = arg(argv, 2);
    if src.is_empty() || dst.is_empty() {
        console::write(b"mv: missing operand\n");
        proc::exit(1);
    }

    match fs::rename(src, dst) {
        Ok(()) => proc::exit(0),
        Err(_) => {
            console::write(b"mv: cannot rename\n");
            proc::exit(1);
        }
    }
}
