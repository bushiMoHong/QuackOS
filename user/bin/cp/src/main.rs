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
    "b   {cp_main}",
    cp_main = sym cp_main,
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
fn cp_main(_argc: usize, argv: *const *const u8) -> ! {
    let src = arg(argv, 1);
    let dst = arg(argv, 2);
    if src.is_empty() || dst.is_empty() {
        console::write(b"cp: missing operand\n");
        proc::exit(1);
    }

    let sfd = match fs::open(src) {
        Ok(f) => f,
        Err(_) => { console::write(b"cp: cannot open source\n"); proc::exit(1); }
    };
    match fs::create(dst) {
        Ok(()) | Err(17 /* EEXIST */) => {}
        Err(_) => { fs::close(sfd).ok(); console::write(b"cp: cannot create dest\n"); proc::exit(1); }
    }
    let dfd = match fs::open(dst) {
        Ok(f) => f,
        Err(_) => { fs::close(sfd).ok(); console::write(b"cp: cannot open dest\n"); proc::exit(1); }
    };

    let mut buf = [0u8; 256];
    loop {
        match fs::read(sfd, &mut buf) {
            Ok(0) => break,
            Ok(n) => { fs::write(dfd, &buf[..n]).ok(); }
            Err(_) => break,
        }
    }

    fs::close(sfd).ok();
    fs::close(dfd).ok();
    proc::exit(0);
}
