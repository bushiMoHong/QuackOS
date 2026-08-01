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
    "ldr x0, [x2]",          // argc from orig sp
    "add x1, x2, #8",        // argv from orig sp
    "b   {sudo_main}",
    sudo_main = sym sudo_main,
);

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    proc::exit(1);
}

/// Get an argument as a byte slice without UTF-8 validation overhead.
fn arg_bytes(argv: *const *const u8, idx: usize) -> &'static [u8] {
    let ptr = unsafe { *argv.add(idx) };
    if ptr.is_null() {
        return b"";
    }
    let s = unsafe { CStr::from_ptr(ptr) };
    s.to_bytes()
}

/// Fixed virtual address where the kernel writes BootInfo.
const BOOTINFO_ADDR: usize = 0x22b000;
const USER_STACK_TOP: usize = 0x7fff_fff1_0000;

/// BootInfo struct — must match kernel / liblinux definition.
#[repr(C)]
struct BootInfo {
    program_entry: u64,
    stack_top: u64,
    brk: u64,
    phdr_addr: u64,
    phent_size: u64,
    phnum: u64,
    argc: u64,
    argv_buf: [u8; 2048],
    cwd: [u8; 256],
}

/// Static buffer for reading the target ELF (64 KB, in .bss).
static mut ELF_BUF: [u8; 65536] = [0u8; 65536];

#[unsafe(no_mangle)]
fn sudo_main(argc: usize, argv: *const *const u8) -> ! {
    let cmd = arg_bytes(argv, 1);
    if cmd.is_empty() {
        console::write(b"sudo: missing command\n");
        proc::exit(1);
    }
    let cmd_str = core::str::from_utf8(cmd).unwrap_or("");
    if cmd_str.is_empty() {
        console::write(b"sudo: bad command name\n");
        proc::exit(1);
    }

    // ------------------------------------------------------------------
    // 1. Read the target ELF from the filesystem
    // ------------------------------------------------------------------

    let fd = match fs::open(cmd_str) {
        Ok(f) => f,
        Err(_) => {
            console::write(b"sudo: open failed\n");
            proc::exit(1);
        }
    };

    let file_size = match fs::fstat(fd) {
        Ok(sz) => sz as usize,
        Err(_) => {
            fs::close(fd).ok();
            console::write(b"sudo: stat failed\n");
            proc::exit(1);
        }
    };

    if file_size == 0 || file_size > 65535 {
        fs::close(fd).ok();
        console::write(b"sudo: file too large\n");
        proc::exit(1);
    }

    let elf_ptr = &raw mut ELF_BUF as *mut u8;
    let mut total = 0usize;
    while total < file_size {
        let chunk = (file_size - total).min(256);
        let dest = unsafe { core::slice::from_raw_parts_mut(elf_ptr.add(total), chunk) };
        match fs::read(fd, dest) {
            Ok(0) => break,
            Ok(n) => total += n,
            Err(_) => {
                fs::close(fd).ok();
                console::write(b"sudo: read failed\n");
                proc::exit(1);
            }
        }
    }
    fs::close(fd).ok();

    if total < file_size {
        console::write(b"sudo: short read\n");
        proc::exit(1);
    }

    // ------------------------------------------------------------------
    // 2. Build new argv in a static buffer (avoiding stack overflow)
    // ------------------------------------------------------------------

    // Write directly into BootInfo's argv_buf to save stack space
    let mut new_argc = 0u64;
    let mut wp = 0usize;
    let argv_dst = unsafe { &raw mut (*(BOOTINFO_ADDR as *mut BootInfo)).argv_buf as *mut u8 };

    for i in 1..argc {
        if new_argc >= 128 || wp >= 2047 {
            break;
        }
        let a = arg_bytes(argv, i);
        if a.is_empty() {
            continue;
        }
        let end = wp + a.len();
        if end >= 2048 {
            break;
        }
        unsafe {
            core::ptr::copy_nonoverlapping(a.as_ptr(), argv_dst.add(wp), a.len());
            argv_dst.add(end).write(0);
        }
        wp = end + 1;
        new_argc += 1;
    }

    // Zero out remaining argv_buf so leftover data from
    // the previous BootInfo isn't decoded as extra args.
    if wp < 2047 {
        unsafe {
            core::ptr::write_bytes(argv_dst.add(wp), 0u8, 2048 - wp);
        }
    }

    // ------------------------------------------------------------------
    // 3. Set argc in BootInfo
    // ------------------------------------------------------------------

    unsafe {
        (*(BOOTINFO_ADDR as *mut BootInfo)).argc = new_argc;
    }

    // ------------------------------------------------------------------
    // 4. Exec the target program (does not return on success)
    // ------------------------------------------------------------------

    let elf_slice = unsafe { core::slice::from_raw_parts(elf_ptr, total) };
    match proc::exec(elf_slice, USER_STACK_TOP, BOOTINFO_ADDR) {
        Ok(()) => {}
        Err(_) => { console::write(b"sudo: exec failed\n"); }
    }

    proc::exit(1);
}
