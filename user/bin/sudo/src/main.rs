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

/// Static buffer for constructing full path from command name (256 B).
static mut PATH_BUF: [u8; 256] = [0u8; 256];

/// Parse a 64-bit little-endian ELF header and return (entry, brk, phdr_addr,
/// phent_size, phnum).  Only processes PT_LOAD segments.
fn parse_elf(elf: &[u8]) -> Result<(usize, usize, usize, usize, usize), &'static [u8]> {
    // ELF header (64-bit LE)
    if elf.len() < 64 {
        return Err(b"sudo: ELF too short\n");
    }
    // Magic: \x7fELF
    if elf[0] != 0x7f || elf[1] != b'E' || elf[2] != b'L' || elf[3] != b'F' {
        return Err(b"sudo: bad ELF magic\n");
    }
    // 64-bit (class == 2) and little-endian (data == 1)
    if elf[4] != 2 || elf[5] != 1 {
        return Err(b"sudo: not 64-bit LE ELF\n");
    }

    let entry = u64::from_le_bytes(elf[24..32].try_into().unwrap()) as usize;
    let phoff = u64::from_le_bytes(elf[32..40].try_into().unwrap()) as usize;
    let phentsize = u16::from_le_bytes(elf[54..56].try_into().unwrap()) as usize;
    let phnum = u16::from_le_bytes(elf[56..58].try_into().unwrap()) as usize;

    if phentsize == 0 || phnum == 0 {
        return Err(b"sudo: ELF has no program headers\n");
    }

    // Scan PT_LOAD segments
    let mut first_load_va = 0usize;
    let mut data_end = 0usize;
    for i in 0..phnum {
        let off = phoff + i * phentsize;
        if off + 56 > elf.len() {
            return Err(b"sudo: ELF PHDR out of bounds\n");
        }
        let p_type = u32::from_le_bytes(elf[off..off+4].try_into().unwrap());
        if p_type != 1 { continue; } // PT_LOAD

        let p_vaddr = u64::from_le_bytes(elf[off+16..off+24].try_into().unwrap()) as usize;
        let p_memsz = u64::from_le_bytes(elf[off+40..off+48].try_into().unwrap()) as usize;

        if first_load_va == 0 || p_vaddr < first_load_va {
            first_load_va = p_vaddr;
        }
        let seg_end = p_vaddr + p_memsz;
        if seg_end > data_end {
            data_end = seg_end;
        }
    }

    let page_size = 4096;
    let brk = (data_end + page_size - 1) & !(page_size - 1);
    let phdr_addr = first_load_va + phoff;

    Ok((entry, brk, phdr_addr, phentsize, phnum))
}

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

    // Resolve the command path: if it's already absolute use it directly;
    // otherwise try <cmd> first then /bin/<cmd> as a fallback.
    let resolved: &str = if cmd_str.starts_with('/') {
        cmd_str
    } else {
        let fd = fs::open(cmd_str);
        if fd.is_ok() {
            fs::close(fd.unwrap()).ok();
            cmd_str
        } else {
            // Try /bin/<cmd>
            let bin_ptr = (&raw mut PATH_BUF) as *mut u8;
            let prefix = b"/bin/";
            let cmd_len = cmd.len().min(255 - prefix.len());
            let path_len = prefix.len() + cmd_len;
            unsafe {
                core::ptr::copy_nonoverlapping(prefix.as_ptr(), bin_ptr, prefix.len());
                core::ptr::copy_nonoverlapping(cmd.as_ptr(), bin_ptr.add(prefix.len()), cmd_len);
                bin_ptr.add(path_len).write(0);
            }
            let resolved_slice = unsafe {
                core::slice::from_raw_parts(bin_ptr, path_len)
            };
            core::str::from_utf8(resolved_slice).unwrap_or("/bin/")
        }
    };

    let fd = match fs::open(resolved) {
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
    // 2. Parse the target ELF to get entry point, brk, phdr info
    // ------------------------------------------------------------------
    let elf_slice = unsafe { core::slice::from_raw_parts(elf_ptr, total) };
    let (entry, brk, phdr_addr, phent, phnum) = match parse_elf(elf_slice) {
        Ok(info) => info,
        Err(msg) => { console::write(msg); proc::exit(1); }
    };

    // Update BootInfo with the target program's values (preserve cwd)
    unsafe {
        let bi = &raw mut *(BOOTINFO_ADDR as *mut BootInfo);
        (*bi).program_entry = entry as u64;
        (*bi).brk = brk as u64;
        (*bi).phdr_addr = phdr_addr as u64;
        (*bi).phent_size = phent as u64;
        (*bi).phnum = phnum as u64;
    }

    // ------------------------------------------------------------------
    // 3. Build new argv in a static buffer (avoiding stack overflow)
    // ------------------------------------------------------------------

    // Write directly into BootInfo's argv_buf to save stack space.
    // argv_buf is at offset 56 from BOOTINFO_ADDR (field layout verified).
    const ARGV_BUF_OFFSET: usize = 56;
    let mut new_argc = 0u64;
    let mut wp = 0usize;
    let argv_dst = (BOOTINFO_ADDR + ARGV_BUF_OFFSET) as *mut u8;

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
    // 4. Set argc in BootInfo
    // ------------------------------------------------------------------

    unsafe {
        (*(BOOTINFO_ADDR as *mut BootInfo)).argc = new_argc;
    }

    // ------------------------------------------------------------------
    // 5. Exec the target program (does not return on success)
    // ------------------------------------------------------------------

    let elf_slice = unsafe { core::slice::from_raw_parts(elf_ptr, total) };
    match proc::exec(elf_slice, USER_STACK_TOP, BOOTINFO_ADDR) {
        Ok(()) => {}
        Err(_) => { console::write(b"sudo: exec failed\n"); }
    }

    proc::exit(1);
}
