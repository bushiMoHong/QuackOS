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
const BOOTINFO_ADDR: usize = 0x208110;

#[repr(C)]
struct BootInfo {
    program_entry: u64,
    stack_top:    u64,
    brk:          u64,
    phdr_addr:    u64,
    phent_size:   u64,
    phnum:        u64,
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

    // 4. Set up the Linux program's initial stack, then jump to it.
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

    unsafe {
        let p = sp as *mut u64;
        // Zero the auxv region — stack page may contain stale data
        // from the physical page allocator (not guaranteed to be zeroed).
        core::ptr::write_bytes(p, 0u8, 32 * 8); // 256 bytes
        // argc
        p.write_volatile(0);
        // argv terminator
        p.add(1).write_volatile(0);
        // envp terminator
        p.add(2).write_volatile(0);
        // auxv entries
        p.add(3).write_volatile(AT_PHDR);
        p.add(4).write_volatile(bootinfo.phdr_addr);
        p.add(5).write_volatile(AT_PHENT);
        p.add(6).write_volatile(bootinfo.phent_size);
        p.add(7).write_volatile(AT_PHNUM);
        p.add(8).write_volatile(bootinfo.phnum);
        p.add(9).write_volatile(AT_PAGESZ);
        p.add(10).write_volatile(4096);
        p.add(11).write_volatile(AT_ENTRY);
        p.add(12).write_volatile(bootinfo.program_entry);
        // AT_HWCAP — CPU feature flags (0 = safe fallback)
        p.add(13).write_volatile(AT_HWCAP);
        p.add(14).write_volatile(0);
        // AT_CLKTCK — clock ticks per second
        p.add(15).write_volatile(AT_CLKTCK);
        p.add(16).write_volatile(100);
        // AT_RANDOM — 16 random bytes for stack canary / arc4random
        // The data lives at sp+208, right after the auxv table.
        p.add(17).write_volatile(AT_RANDOM);
        p.add(18).write_volatile((sp + 208) as u64);
        // AT_NULL terminator
        p.add(19).write_volatile(AT_NULL);
        p.add(20).write_volatile(0);

        // 16 pseudo-random bytes at sp+208
        let random_bytes: [u8; 16] = [
            0x7f, 0x3a, 0x91, 0x4e, 0x56, 0xb8, 0x2c, 0x0d,
            0xe1, 0xf5, 0x48, 0x37, 0xaa, 0x9b, 0x6c, 0x12,
        ];
        core::ptr::copy_nonoverlapping(random_bytes.as_ptr(), (sp + 208) as *mut u8, 16);

        asm!("dsb ishst");
    }

    // Enter the Linux program with the register state execve() guarantees:
    // all GPRs zeroed.  glibc's _start treats x0 as rtld_fini and registers
    // any non-NULL value with atexit — a stale stack pointer here makes
    // exit() branch to the stack and fault.
    unsafe {
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
    }
}

// ---------------------------------------------------------------------------
// Panic / alloc error handlers
// ---------------------------------------------------------------------------

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop { unsafe { asm!("wfi"); } }
}
