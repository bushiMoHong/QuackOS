//! liblinux — Linux syscall compatibility layer (LibOS) for QuackOS.
//!
//! Runs as a user-space ELF alongside the Linux binary.  The microkernel
//! reflects all SVC #0 (Linux) syscalls to our handler, which dispatches
//! them and returns the result.  Execution alternates between the Linux
//! binary (doing useful work) and this handler (servicing syscalls).
//!
//! Phase 1: enough syscalls to run a statically-linked musl "hello world".

#![no_std]
#![no_main]

mod errno;
mod fd_table;
mod fs;
mod futex;
mod ipc;
mod loader;
mod misc;
mod mm;
mod native;
mod proc;
mod signal;
mod syscall;
mod task;
mod time;

use core::arch::asm;
use core::panic::PanicInfo;
use task::TaskStruct;

// ---------------------------------------------------------------------------
// BootInfo — must match the kernel's definition
// ---------------------------------------------------------------------------

/// Fixed user-space address where the kernel writes BootInfo.
/// Placed right after SAVE_AREA (34×u64 = 272 bytes at the .bss Symbol) so
/// it lives in the same already-mapped page and the kernel's BootInfo write
/// never overlaps with live liblinux data.
/// Must match the kernel-side `elf_loader::BOOTINFO_VA`.
const BOOTINFO_ADDR: usize = 0x22b000;

#[repr(C)]
struct BootInfo {
    program_entry: u64,
    stack_top:    u64,
    brk:          u64,
    phdr_addr:    u64,
    phent_size:   u64,
    phnum:        u64,
    argc:         u64,
    argv_buf:     [u8; 2048],
    cwd:          [u8; 256],
}

// ---------------------------------------------------------------------------
// Per-process state
// ---------------------------------------------------------------------------

/// Save area for the Linux program's context (34 × u64, filled by the kernel).
#[repr(C)]
struct LinuxContext {
    regs: [u64; 34], // x0-x30, elr, spsr, sp
}

static mut SAVE_AREA: LinuxContext = LinuxContext { regs: [0; 34] };
static mut TASK: Option<TaskStruct> = None;

// ---------------------------------------------------------------------------
// Syscall dispatch
// ---------------------------------------------------------------------------

fn dispatch_linux_syscall(
    task: &mut TaskStruct,
    nr: usize,
    a0: usize, a1: usize, a2: usize,
    a3: usize, a4: usize, a5: usize,
) -> u64 {
    syscall::dispatch(task, nr, a0, a1, a2, a3, a4, a5)
}

// ---------------------------------------------------------------------------
// Linux syscall handler (invoked directly by the kernel after reflection)
// ---------------------------------------------------------------------------

/// Entry point for reflected Linux syscalls.
///
/// The kernel saves the Linux binary's context into `SAVE_AREA`, sets ELR
/// to this function, and does `eret`.  We read the syscall number/args from
/// the save area, dispatch, and call `linux_syscall_done` to resume the
/// Linux binary.
#[no_mangle]
#[allow(static_mut_refs)]
fn linux_syscall_handler() {
    let nr = unsafe { SAVE_AREA.regs[8] }; // x8 = Linux syscall number
    let n = nr as usize;

    let args = unsafe {
        (
            SAVE_AREA.regs[0] as usize,
            SAVE_AREA.regs[1] as usize,
            SAVE_AREA.regs[2] as usize,
            SAVE_AREA.regs[3] as usize,
            SAVE_AREA.regs[4] as usize,
            SAVE_AREA.regs[5] as usize,
        )
    };

    let task = unsafe { TASK.as_mut().unwrap() };
    let ret = dispatch_linux_syscall(task, nr as usize, args.0, args.1, args.2, args.3, args.4, args.5);

    unsafe { native::linux_syscall_done(ret as usize); }
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

#[no_mangle]
pub extern "C" fn _start() -> ! {
    // 1. Register as the Linux syscall handler with the microkernel.
    let handler_pc = linux_syscall_handler as *const () as usize;
    let save_area_ptr = (&raw const SAVE_AREA) as *const LinuxContext as usize;
    unsafe { native::register_linux_handler(handler_pc, save_area_ptr); }

    // 2. Read BootInfo (written by the kernel at a fixed address).
    let bootinfo = unsafe { core::ptr::read_volatile(BOOTINFO_ADDR as *const BootInfo) };

    // 3. Initialise per-process state.
    unsafe { TASK = Some(TaskStruct::new(bootinfo.brk as usize)); }
    // Restore CWD from BootInfo (preserved across exec)
    if let Some(ref mut task) = unsafe { TASK.as_mut() } {
        let cwd_len = bootinfo.cwd.iter().position(|&b| b == 0).unwrap_or(0);
        if cwd_len > 0 {
            task.cwd = [0u8; 256];
            task.cwd[..cwd_len.min(255)].copy_from_slice(&bootinfo.cwd[..cwd_len.min(255)]);
        }
        // Write CWD back to BootInfo page so native (qlibc) programs
        // can read it from the user-space VA.  The kernel may have
        // written via a different VA/cache mapping, so we re-write
        // here to ensure the data is visible through this VA.
        let cwd_end = task.cwd.iter().position(|&b| b == 0).unwrap_or(0);
        if cwd_end > 0 {
            let dst = (BOOTINFO_ADDR + 2104) as *mut u8;
            unsafe {
                for i in 0..cwd_end {
                    core::ptr::write_volatile(dst.add(i), task.cwd[i]);
                }
                core::ptr::write_volatile(dst.add(cwd_end), 0u8);
            }
        }
    }

    // 4. Build PWD environment variable from the restored CWD.
    //    bash uses PWD directly for \W prompt expansion, so it must
    //    match the actual current working directory.
    let mut pwd_buf = [0u8; 264];
    let pwd_len;
    {
        let task = unsafe { TASK.as_ref().unwrap() };
        let cwd_end = task.cwd.iter().position(|&b| b == 0).unwrap_or(0);
        pwd_buf[0] = b'P'; pwd_buf[1] = b'W'; pwd_buf[2] = b'D'; pwd_buf[3] = b'=';
        if cwd_end > 0 {
            let copy = cwd_end.min(256);
            pwd_buf[4..4 + copy].copy_from_slice(&task.cwd[..copy]);
            pwd_len = 4 + copy;
        } else {
            pwd_buf[4] = b'/';
            pwd_len = 5;
        }
    }

    // 5. Set up the Linux program's initial stack, then jump to it.
    //
    // AArch64 Linux process entry convention:
    //   sp → argc (8 bytes)
    //        argv[0..argc], NULL  (8 bytes each)
    //        envp[0..N], NULL     (8 bytes each)
    //        auxv[0..N], AT_NULL  (16 bytes each)
    //        (16-byte aligned)
    //
    // Reserve one page for this data below the stack top.
    let entry = bootinfo.program_entry as usize;
    let sp = bootinfo.stack_top as usize - 4096;

    // Helper constants
    const AT_PHDR: u64   = 3;
    const AT_PHENT: u64  = 4;
    const AT_PHNUM: u64  = 5;
    const AT_PAGESZ: u64 = 6;
    const AT_ENTRY: u64  = 9;
    const AT_HWCAP: u64  = 16;
    const AT_CLKTCK: u64 = 17;
    const AT_RANDOM: u64 = 25;
    const AT_NULL: u64   = 0;

    // argc/argv from the parent process (preserved across exec via BootInfo)
    let argc = bootinfo.argc as usize;

    unsafe {
        let p = sp as *mut u64;
        // Zero the first 512 bytes of the stack page (enough for argc +
        // argv pointers + auxv; no code beyond this range writes here).
        core::ptr::write_bytes(p, 0u8, 512);

        // Write argv strings backwards from near the top of the page.
        let mut wp = (p as usize) + 4096 - 8;
        let mut argv_ptrs = [0usize; 128];
        let mut arg_idx = 0usize;

        // Decode null-separated strings from BootInfo.argv_buf
        let mut buf_pos = 0usize;
        while buf_pos < bootinfo.argv_buf.len() && arg_idx < 128 {
            while buf_pos < bootinfo.argv_buf.len() && bootinfo.argv_buf[buf_pos] == 0 {
                buf_pos += 1;
            }
            if buf_pos >= bootinfo.argv_buf.len() { break; }
            let start = buf_pos;
            while buf_pos < bootinfo.argv_buf.len() && bootinfo.argv_buf[buf_pos] != 0 {
                buf_pos += 1;
            }
            let slen = buf_pos - start;
            if slen == 0 { break; }

            let alloc = ((slen + 1) + 7) & !7;
            wp -= alloc;
            let s_va = wp;
            core::ptr::copy_nonoverlapping(
                bootinfo.argv_buf[start..start + slen].as_ptr(),
                s_va as *mut u8, slen);
            core::ptr::write_volatile((s_va + slen) as *mut u8, 0u8);
            argv_ptrs[arg_idx] = s_va;
            arg_idx += 1;
            buf_pos += 1;
        }

        // Write env strings backwards from near the top of the page.
        // Use $PWD variable expansion instead of \W escape.
        // bash's promptvars option (on by default) expands $PWD
        // via expand_prompt_string() after decode_prompt_string().
        let ps1_str = b"PS1=$PWD \\$ ";
        let home_str = b"HOME=/";
        let ps1_alloc = ((ps1_str.len() + 1) + 7) & !7;
        let home_alloc = ((home_str.len() + 1) + 7) & !7;
        wp -= home_alloc;
        let home_va = wp;
        core::ptr::copy_nonoverlapping(home_str.as_ptr(), home_va as *mut u8, home_str.len());
        core::ptr::write_volatile((home_va + home_str.len()) as *mut u8, 0u8);
        wp -= ps1_alloc;
        let ps1_va = wp;
        core::ptr::copy_nonoverlapping(ps1_str.as_ptr(), ps1_va as *mut u8, ps1_str.len());
        core::ptr::write_volatile((ps1_va + ps1_str.len()) as *mut u8, 0u8);
        // PWD
        let pwd_alloc = ((pwd_len + 1) + 7) & !7;
        wp -= pwd_alloc;
        let pwd_va = wp;
        core::ptr::copy_nonoverlapping(pwd_buf.as_ptr(), pwd_va as *mut u8, pwd_len);
        core::ptr::write_volatile((pwd_va + pwd_len) as *mut u8, 0u8);

        // Stack layout (bottom-up from p):
        //   [argc] [argv...] [NULL] [PS1] [PWD] [HOME] [NULL] [auxv...]
        p.write_volatile(argc as u64);
        let a = p.add(1);
        for i in 0..arg_idx {
            a.add(i).write_volatile(argv_ptrs[i] as u64);
        }
        a.add(arg_idx).write_volatile(0);                // argv terminator
        a.add(arg_idx + 1).write_volatile(ps1_va as u64);  // PS1
        a.add(arg_idx + 2).write_volatile(pwd_va as u64);  // PWD
        a.add(arg_idx + 3).write_volatile(home_va as u64); // HOME
        a.add(arg_idx + 4).write_volatile(0);              // envp terminator
        let auxv = a.add(arg_idx + 5);
        unsafe { auxv.add(0).write_volatile(AT_PHDR); }
        unsafe { auxv.add(1).write_volatile(bootinfo.phdr_addr); }
        unsafe { auxv.add(2).write_volatile(AT_PHENT); }
        unsafe { auxv.add(3).write_volatile(bootinfo.phent_size); }
        unsafe { auxv.add(4).write_volatile(AT_PHNUM); }
        unsafe { auxv.add(5).write_volatile(bootinfo.phnum); }
        unsafe { auxv.add(6).write_volatile(AT_PAGESZ); }
        unsafe { auxv.add(7).write_volatile(4096); }
        unsafe { auxv.add(8).write_volatile(AT_ENTRY); }
        unsafe { auxv.add(9).write_volatile(bootinfo.program_entry); }
        // AT_HWCAP — CPU feature flags (0 = safe fallback)
        unsafe { auxv.add(10).write_volatile(AT_HWCAP); }
        unsafe { auxv.add(11).write_volatile(0); }
        // AT_CLKTCK — clock ticks per second
        unsafe { auxv.add(12).write_volatile(AT_CLKTCK); }
        unsafe { auxv.add(13).write_volatile(100); }
        // AT_RANDOM — 16 random bytes for stack canary / arc4random
        // The data lives at sp+208, right after the auxv table.
        unsafe { auxv.add(14).write_volatile(AT_RANDOM); }
        unsafe { auxv.add(15).write_volatile((sp + 208) as u64); }
        // AT_NULL terminator
        unsafe { auxv.add(16).write_volatile(AT_NULL); }
        unsafe { auxv.add(17).write_volatile(0); }

        // 16 pseudo-random bytes at sp+208
        let random_bytes: [u8; 16] = [
            0x7f, 0x3a, 0x91, 0x4e, 0x56, 0xb8, 0x2c, 0x0d,
            0xe1, 0xf5, 0x48, 0x37, 0xaa, 0x9b, 0x6c, 0x12,
        ];
        core::ptr::copy_nonoverlapping(random_bytes.as_ptr(), (sp + 208) as *mut u8, 16);

        #[cfg(target_arch = "aarch64")]
        asm!("dsb ishst");
        #[cfg(target_arch = "riscv64")]
        asm!("fence");
    }

    // Enter the Linux program with the register state execve() guarantees:
    // all GPRs zeroed.  glibc's _start treats x0 as rtld_fini and registers
    // any non-NULL value with atexit — a stale stack pointer here makes
    // exit() branch to the stack and fault.
    unsafe {
        #[cfg(target_arch = "aarch64")]
        asm!(
            "mov sp, x0",
            "mov x0,  xzr",
            "mov x1,  xzr",
            "mov x2,  xzr",
            "mov x3,  xzr",
            "mov x4,  xzr",
            "mov x5,  xzr",
            "mov x6,  xzr",
            "mov x7,  xzr",
            "mov x8,  xzr",
            "mov x9,  xzr",
            "mov x10, xzr",
            "mov x11, xzr",
            "mov x12, xzr",
            "mov x13, xzr",
            "mov x14, xzr",
            "mov x15, xzr",
            "mov x17, xzr",
            "mov x18, xzr",
            "mov x19, xzr",
            "mov x20, xzr",
            "mov x21, xzr",
            "mov x22, xzr",
            "mov x23, xzr",
            "mov x24, xzr",
            "mov x25, xzr",
            "mov x26, xzr",
            "mov x27, xzr",
            "mov x28, xzr",
            "mov x29, xzr",
            "mov x30, xzr",
            "br  x16",
            in("x0") sp,
            in("x16") entry,
            options(noreturn)
        );
        #[cfg(target_arch = "riscv64")]
        asm!(
            "mv sp, x10",   // sp = 栈指针（来自 a0）
            "mv x10, x0",   // a0 = 0（rtld_fini）
            "mv x11, x0",   // a1 = 0
            "mv x12, x0",   // a2 = 0
            "mv x13, x0",   // a3 = 0
            "mv x14, x0",   // a4 = 0
            "mv x15, x0",   // a5 = 0
            "mv x17, x0",   // a7 = 0
            "mv x1,  x0",   // ra = 0
            "mv x3,  x0",   // gp = 0
            "mv x4,  x0",   // tp = 0
            "mv x5,  x0",   // t0 = 0
            "mv x6,  x0",   // t1 = 0
            "mv x7,  x0",   // t2 = 0
            "mv x8,  x0",   // s0 = 0
            "mv x9,  x0",   // s1 = 0
            "mv x18, x0",   // s2 = 0
            "mv x19, x0",   // s3 = 0
            "mv x20, x0",   // s4 = 0
            "mv x21, x0",   // s5 = 0
            "mv x22, x0",   // s6 = 0
            "mv x23, x0",   // s7 = 0
            "mv x24, x0",   // s8 = 0
            "mv x25, x0",   // s9 = 0
            "mv x26, x0",   // s10 = 0
            "mv x27, x0",   // s11 = 0
            "mv x28, x0",   // t3 = 0
            "mv x29, x0",   // t4 = 0
            "mv x30, x0",   // t5 = 0
            "mv x31, x0",   // t6 = 0
            "jr x16",       // 跳转到入口
            in("x10") sp,
            in("x16") entry,
            options(noreturn)
        );
    }
}

// ---------------------------------------------------------------------------
// Panic / alloc error handlers
// ---------------------------------------------------------------------------

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop { unsafe { asm!("wfi"); } }
}
