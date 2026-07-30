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
    "b   {clear_main}",
    clear_main = sym clear_main,
);

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    proc::exit(1);
}

#[unsafe(no_mangle)]
fn clear_main() -> ! {
    // ANSI: clear entire screen, move cursor to top-left
    console::write(b"\x1b[2J\x1b[H");
    proc::exit(0);
}
