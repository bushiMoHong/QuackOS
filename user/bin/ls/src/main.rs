#![no_std]
#![no_main]

use core::ffi::CStr;
use qlibc::{console, dirent, fs, proc};

core::arch::global_asm!(
    ".globl _start",
    "_start:",
    "mov x29, #0",
    "mov x30, #0",
    "mov x2, sp",
    "and sp, x2, #-16",
    "ldr x0, [x2]",          // argc from orig sp
    "add x1, x2, #8",        // argv from orig sp
    "b   {ls_main}",
    ls_main = sym ls_main,
);

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    proc::exit(1);
}

/// Read argv[idx] as a &str, defaulting to "." for idx=0.
fn arg_or(argc: usize, argv: *const *const u8, idx: usize, default: &str) -> &str {
    if idx < argc {
        let ptr = unsafe { *argv.add(idx) };
        if !ptr.is_null() {
            if let Ok(s) = unsafe { CStr::from_ptr(ptr) }.to_str() {
                return s;
            }
        }
    }
    default
}

#[unsafe(no_mangle)]
fn ls_main(argc: usize, argv: *const *const u8) -> ! {
    // Ensure CWD is initialised before first use
    qlibc::fs::init_cwd();
    let path = arg_or(argc, argv, 1, ".");

    // Open directory
    let fid = match fs::open(path) {
        Ok(f) => f,
        Err(_) => {
            console::write(b"ls: cannot open '");
            console::write(path.as_bytes());
            console::write(b"'\n");
            proc::exit(1);
        }
    };

    // Read directory entries in a loop — the FsServer may return
    // entries in chunks because the IPC buffer is limited to 256 bytes.
    let mut buf = [0u8; 1024];
    loop {
        let n = match fs::getdents(fid, &mut buf) {
            Ok(n) => n,
            Err(_) => {
                fs::close(fid).ok();
                console::write(b"ls: read error\n");
                proc::exit(1);
            }
        };
        if n == 0 {
            break; // no more entries
        }
        // Only parse the valid portion of the buffer.
        for entry in dirent::parse_dirents(&buf[..n]) {
            let name = entry.name_bytes();
            if name != b"." && name != b".." {
                console::write(name);
                console::write(b"  ");
            }
        }
    }
    console::write(b"\n");

    fs::close(fid).ok();
    proc::exit(0);
}
