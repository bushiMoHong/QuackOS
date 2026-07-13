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
mod ipc;
mod loader;
mod misc;
mod mm;
mod native;
mod proc;
mod task;

use core::arch::asm;
use core::panic::PanicInfo;
use task::TaskStruct;

// ---------------------------------------------------------------------------
// BootInfo — must match the kernel's definition
// ---------------------------------------------------------------------------

/// Fixed user-space address where the kernel writes BootInfo.
const BOOTINFO_ADDR: usize = 0x7FFF_FFEF_FFA0;

#[repr(C)]
struct BootInfo {
    program_entry: u64,
    stack_top:    u64,
    brk:          u64,
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
    match nr {
        17  => misc::sys_getcwd(a0, a1),
        56  => fs::sys_openat(task, a0, a1, a2, a3),
        57  => fs::sys_close(task, a0),
        63  => fs::sys_read(task, a0, a1, a2),
        64  => fs::sys_write(task, a0, a1, a2),
        80  => fs::sys_fstat(task, a0, a1),
        93  => proc::sys_exit(task, a0),
        94  => proc::sys_exit_group(task, a0),
        160 => misc::sys_uname(a0),
        172 => proc::sys_getpid(task),
        174 => proc::sys_getuid(task),
        175 => proc::sys_geteuid(task),
        176 => proc::sys_getgid(task),
        177 => proc::sys_getegid(task),
        214 => mm::sys_brk(task, a0),
        215 => mm::sys_munmap(task, a0, a1),
        222 => mm::sys_mmap(task, a0, a1, a2, a3, a4, a5),
        278 => misc::sys_getrandom(a0, a1, a2),
        _   => u64::from_le_bytes((-(errno::ENOSYS as i64)).to_le_bytes()),
    }
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

    // 4. Jump to the Linux binary — we never return from here.
    let entry = bootinfo.program_entry as usize;
    let stack = bootinfo.stack_top as usize;

    unsafe {
        asm!(
            "mov sp, {sp}",
            "br  {entry}",
            sp = in(reg) stack,
            entry = in(reg) entry,
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
