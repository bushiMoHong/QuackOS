//! Common trap (exception / interrupt) handling.
//!
//! This module sits on top of the architecture-specific `aarch64::base::trap`
//! module and provides:
//!
//! | Item             | Purpose                                         |
//! |------------------|-------------------------------------------------|
//! | `TrapCause`      | Unified exception / interrupt cause enum        |
//! | `PageFaultCause` | LOAD / STORE / EXEC                             |
//! | `init()`         | Initialise the architecture trap subsystem      |
//! | `irq_enable()`   | Enable IRQs                                     |
//!
//! # Usage from a kernel binary
//!
//! ```ignore
//! use common::kernel::trap;
//!
//! unsafe {
//!     trap::init();
//!     trap::irq_enable();
//! }
//! ```

pub mod context;
pub mod native;

pub use context::{ExceptionKind, ExceptionSource, GeneralRegs, TrapFrame, UserContext};
pub use native::thread_trampoline_addr;

// Re-export arch functions that the kernel layer calls directly
pub use aarch64::base::trap::{
    fiq_disable, fiq_enable, irq_disable, irq_disable_and_store, irq_enable, irq_restore,
    wait_for_interrupt,
};
pub use aarch64::base::trap::handler::{
    install_default_handler, install_trap_handler, set_trap_fns, TrapHandler,
};
pub use aarch64::base::trap::syndrome::{Fault, Syndrome};

// ---------------------------------------------------------------------------
// PageFaultCause
// ---------------------------------------------------------------------------

/// Reason for a page fault.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageFaultCause {
    /// Fault on a load (read) access.
    Load,
    /// Fault on a store (write) access.
    Store,
    /// Fault on an instruction fetch.
    Exec,
}

// ---------------------------------------------------------------------------
// TrapCause — unified across architectures
// ---------------------------------------------------------------------------

/// Unified representation of the reason an exception / interrupt occurred.
///
/// Each architecture port maps its native cause register (ESR_EL1 on AArch64,
/// scause on RISC-V, EStat on LoongArch) into this enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrapCause {
    /// System call from user space.
    Syscall,
    /// Timer interrupt.
    TimerIrq,
    /// Page fault on load.
    PageFaultLoad,
    /// Page fault on store.
    PageFaultStore,
    /// Page fault on instruction fetch.
    PageFaultExec,
    /// Illegal instruction.
    IllegalInstruction,
    /// Undefined instruction (UDF — permanently unallocated instruction encodings).
    UndefinedInstruction,
    /// Breakpoint.
    Breakpoint,
    /// External / device interrupt (not timer).
    ExternalIrq,
    /// Alignment fault.
    AlignmentFault,
    /// A trap whose cause the common framework doesn't handle.
    Other(usize),
}

impl TrapCause {
    /// Convert a `TrapCause` into the corresponding `PageFaultCause`, if
    /// this cause is a page fault.
    pub fn as_page_fault(&self) -> Option<PageFaultCause> {
        match self {
            TrapCause::PageFaultLoad => Some(PageFaultCause::Load),
            TrapCause::PageFaultStore => Some(PageFaultCause::Store),
            TrapCause::PageFaultExec => Some(PageFaultCause::Exec),
            _ => None,
        }
    }

    /// Decode a `TrapCause` from an AArch64 ESR_EL1 register value.
    ///
    /// Call this from the architecture-specific trap handler entry point.
    pub fn from_aarch64_esr(esr: u64) -> Self {
        let ec = ((esr >> 26) & 0x3F) as usize;
        match ec {
            0b010101 => TrapCause::Syscall,             // SVC from AArch64
            0b100100 | 0b100101 => {
                // Data Abort (lower or same EL) — 0b100x00 is Instruction Abort
                let wnr = ((esr >> 6) & 1) != 0;        // WnR: 0=read, 1=write
                if wnr {
                    TrapCause::PageFaultStore
                } else {
                    TrapCause::PageFaultLoad
                }
            }
            0b100000 | 0b100001 => TrapCause::PageFaultExec, // Instruction Abort
            0b000000 => {
                // Unknown reason — typically UDF (permanently undefined encoding)
                // ISS = 0 for UDF #0
                TrapCause::UndefinedInstruction
            }
            0b110000 | 0b110001 => TrapCause::Breakpoint, // BRK from AArch64
            0b111000 => TrapCause::IllegalInstruction,  // Trapped MSR/MRS
            _ => TrapCause::Other(ec),
        }
    }
}

// ---------------------------------------------------------------------------
// LinuxContext — saved user context for exception reflection (§8.4)
// ---------------------------------------------------------------------------

/// User-mode register state saved by the kernel during exception reflection.
/// Written to the per-thread `save_area` vaddr, then read by liblinux in user
/// mode to dispatch the Linux syscall.
///
/// Layout must match between kernel and liblinux.  Keep aligned to 8 bytes.
#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct LinuxContext {
    pub x0: u64,  pub x1: u64,  pub x2: u64,  pub x3: u64,
    pub x4: u64,  pub x5: u64,  pub x6: u64,  pub x7: u64,
    pub x8: u64,  // syscall number
    pub x9: u64,  pub x10: u64, pub x11: u64, pub x12: u64,
    pub x13: u64, pub x14: u64, pub x15: u64, pub x16: u64,
    pub x17: u64, pub x18: u64, pub x19: u64, pub x20: u64,
    pub x21: u64, pub x22: u64, pub x23: u64, pub x24: u64,
    pub x25: u64, pub x26: u64, pub x27: u64, pub x28: u64,
    pub x29: u64, pub x30: u64,
    pub elr: u64,
    pub spsr: u64,
    pub sp: u64,
}

/// Save the current Linux program context to the thread's per-thread save_area
/// and redirect execution to the registered liblinux handler.
///
/// Called from `handle_user_sync` when SVC #0 is detected.
/// After this returns, trap_return will eret into liblinux's handler.
pub fn reflect_linux_syscall(tf: &mut TrapFrame) {
    use crate::kernel::sche;

    let tid = sche::current_thread();

    let (handler_pc, save_area) = sche::with_thread(tid, |thread| {
        (thread.linux_handler_pc, thread.linux_save_area)
    })
    .unwrap_or((None, None));

    let handler_pc = match handler_pc {
        Some(pc) => pc,
        None => loop { unsafe { core::arch::asm!("wfi"); } },
    };

    let save_area = match save_area {
        Some(addr) => addr,
        None => loop { unsafe { core::arch::asm!("wfi"); } },
    };

    // Save Linux program context to per-thread save_area using scalar u64 writes.
    unsafe {
        let dst = save_area as *mut u64;
        core::ptr::write_volatile(dst.add(0),  tf.general.x0 as u64);
        core::ptr::write_volatile(dst.add(1),  tf.general.x1 as u64);
        core::ptr::write_volatile(dst.add(2),  tf.general.x2 as u64);
        core::ptr::write_volatile(dst.add(3),  tf.general.x3 as u64);
        core::ptr::write_volatile(dst.add(4),  tf.general.x4 as u64);
        core::ptr::write_volatile(dst.add(5),  tf.general.x5 as u64);
        core::ptr::write_volatile(dst.add(6),  tf.general.x6 as u64);
        core::ptr::write_volatile(dst.add(7),  tf.general.x7 as u64);
        core::ptr::write_volatile(dst.add(8),  tf.general.x8 as u64);
        core::ptr::write_volatile(dst.add(9),  tf.general.x9 as u64);
        core::ptr::write_volatile(dst.add(10), tf.general.x10 as u64);
        core::ptr::write_volatile(dst.add(11), tf.general.x11 as u64);
        core::ptr::write_volatile(dst.add(12), tf.general.x12 as u64);
        core::ptr::write_volatile(dst.add(13), tf.general.x13 as u64);
        core::ptr::write_volatile(dst.add(14), tf.general.x14 as u64);
        core::ptr::write_volatile(dst.add(15), tf.general.x15 as u64);
        core::ptr::write_volatile(dst.add(16), tf.general.x16 as u64);
        core::ptr::write_volatile(dst.add(17), tf.general.x17 as u64);
        core::ptr::write_volatile(dst.add(18), tf.general.x18 as u64);
        core::ptr::write_volatile(dst.add(19), tf.general.x19 as u64);
        core::ptr::write_volatile(dst.add(20), tf.general.x20 as u64);
        core::ptr::write_volatile(dst.add(21), tf.general.x21 as u64);
        core::ptr::write_volatile(dst.add(22), tf.general.x22 as u64);
        core::ptr::write_volatile(dst.add(23), tf.general.x23 as u64);
        core::ptr::write_volatile(dst.add(24), tf.general.x24 as u64);
        core::ptr::write_volatile(dst.add(25), tf.general.x25 as u64);
        core::ptr::write_volatile(dst.add(26), tf.general.x26 as u64);
        core::ptr::write_volatile(dst.add(27), tf.general.x27 as u64);
        core::ptr::write_volatile(dst.add(28), tf.general.x28 as u64);
        core::ptr::write_volatile(dst.add(29), tf.general.x29 as u64);
        core::ptr::write_volatile(dst.add(30), tf.general.x30 as u64);
        core::ptr::write_volatile(dst.add(31), tf.elr as u64);
        core::ptr::write_volatile(dst.add(32), tf.spsr as u64);
        core::ptr::write_volatile(dst.add(33), tf.sp as u64);
    }

    // Debug: track Linux syscalls made by user threads
    {
        let tid = sche::current_thread();
        if tid.0 & 0xFFFF <= 3 {
            let nr = tf.general.x8 as usize;
            let name = linux_syscall_name(nr);
            uart_puts("[L:");
            uart_puts(name);
            uart_puts("] tid=");
            uart_put_hex(tid.0 as u64);
            uart_puts(" elr=");
            uart_put_hex(tf.elr as u64);
            uart_puts("\n");
        }
    }

    // Redirect execution to liblinux handler
    tf.elr = handler_pc;
    tf.general.x0 = save_area;
    tf.spsr = 0; // EL0t
}

struct CommonTrapHandler;

/// UART output: write a single byte to the PL011 at 0x09000000.
pub(crate) fn uart_putc(c: u8) {
    const UART0_DR: *mut u8 = 0x09000000 as *mut u8;
    unsafe { core::ptr::write_volatile(UART0_DR, c); }
}

/// Simple hex dump helper for debug prints.
fn uart_put_hex(mut v: u64) {
    uart_putc(b'0'); uart_putc(b'x');
    for i in 0..16 {
        let nibble = ((v >> (60 - i * 4)) & 0xF) as u8;
        let c = if nibble < 10 { b'0' + nibble } else { b'a' + nibble - 10 };
        uart_putc(c);
    }
}

fn uart_puts(s: &str) {
    for b in s.bytes() { uart_putc(b); }
}

// ---------------------------------------------------------------------------
// Syscall name helpers for debug output
// ---------------------------------------------------------------------------

fn linux_syscall_name(nr: usize) -> &'static str {
    match nr {
        17  => "getcwd",
        23  => "dup",
        25  => "fcntl",
        29  => "ioctl",
        56  => "openat",
        57  => "close",
        62  => "lseek",
        63  => "read",
        64  => "write",
        66  => "writev",
        80  => "fstat",
        93  => "exit",
        94  => "exit_group",
        96  => "set_tid_address",
        98  => "futex",
        99  => "set_robust_list",
        101 => "nanosleep",
        113 => "clock_gettime",
        124 => "sched_yield",
        132 => "sigaltstack",
        134 => "rt_sigaction",
        135 => "rt_sigprocmask",
        153 => "times",
        160 => "uname",
        167 => "prctl",
        169 => "gettimeofday",
        172 => "getpid",
        174 => "getuid",
        175 => "geteuid",
        176 => "getgid",
        177 => "getegid",
        178 => "gettid",
        214 => "brk",
        215 => "munmap",
        222 => "mmap",
        226 => "mprotect",
        233 => "madvise",
        59  => "pipe2",
        220 => "clone",
        221 => "execve",
        278 => "getrandom",
        293 => "rseq",
        _   => "?",
    }
}

fn native_syscall_name(nr: usize) -> &'static str {
    match nr {
        1  => "map_page",
        2  => "unmap_page",
        3  => "ipc_send",
        4  => "ipc_recv",
        5  => "ipc_call",
        6  => "create_thread",
        7  => "exit_thread",
        8  => "register_linux_handler",
        9  => "linux_syscall_done",
        10 => "yield_cpu",
        11 => "console_write",
        12 => "mprotect",
        13 => "spawn",
        14 => "clone",
        15 => "console_read",
        16 => "exec",
        17 => "wait4",
        18 => "create_notification",
        19 => "notify_send",
        20 => "notify_wait",
        21 => "irq_register",
        22 => "irq_ack",
        23 => "ipc_recv_timeout",
        24 => "ipc_call_timeout",
        _  => "?",
    }
}

impl TrapHandler for CommonTrapHandler {
    fn handle_user_sync(tf: &mut TrapFrame) {
        let esr = tf.esr();
        let ec = ((esr >> 26) & 0x3F) as usize;

        match ec {
            // ── SVC from AArch64 (0b010101) ──
            // The 16-bit immediate value in ISS[15:0] distinguishes:
            //   SVC #0 → Linux syscall → exception reflection to liblinux
            //   SVC #1 → QuackOS native syscall → kernel dispatch
            0b010101 => {
                let svc_imm = (esr & 0xFFFF) as u32;
                match svc_imm {
                    0 => {
                        // Linux syscall (SVC #0) → exception reflection
                        reflect_linux_syscall(tf);
                        // tf redirected to liblinux handler; trap_return will eret there.
                    }
                    1 => {
                        // QuackOS native syscall (SVC #1) → kernel dispatch
                        // ELR already points past the SVC (AArch64 SVC saves PC+4).
                        // Do NOT add 4 here — that would skip the next instruction.
                        let nr = tf.general.x8 as u64;
                        native::native_syscall_dispatch(nr, tf);
                    }
                    _ => {
                        uart_puts("[SYS] unknown SVC immediate=");
                        uart_put_hex(svc_imm as u64);
                        uart_puts("\n");
                        loop {
                            unsafe { core::arch::asm!("wfi"); }
                        }
                    }
                }
            }

            // ── all other exception classes ──
            _ => {
                let cause = TrapCause::from_aarch64_esr(esr);
                match cause {
                    TrapCause::Syscall => {
                        // SVC cases handled above — this is unreachable but
                        // kept as a safety net.
                        uart_puts("[SYS] unhandled syscall path\n");
                        loop {
                            unsafe { core::arch::asm!("wfi"); }
                        }
                    }
                    TrapCause::PageFaultLoad | TrapCause::PageFaultStore | TrapCause::PageFaultExec => {
                        let fault_addr = tf.fault_addr();
                        let rw = match cause {
                            TrapCause::PageFaultLoad => "READ",
                            TrapCause::PageFaultStore => "WRITE",
                            _ => "EXEC",
                        };
                        uart_puts("[PF] ");
                        uart_puts(rw);
                        uart_puts(" at ");
                        uart_put_hex(fault_addr as u64);
                        uart_puts(" ELR=");
                        uart_put_hex(tf.elr as u64);
                        uart_puts(" ESR=");
                        uart_put_hex(esr);
                        uart_puts("\n");
                        // Dump regs most likely to be address/base registers
                        uart_puts("[PF] x0="); uart_put_hex(tf.general.x0 as u64);
                        uart_puts(" x1=");     uart_put_hex(tf.general.x1 as u64);
                        uart_puts(" x2=");     uart_put_hex(tf.general.x2 as u64);
                        uart_puts(" x8=");     uart_put_hex(tf.general.x8 as u64);
                        uart_puts(" x21=");    uart_put_hex(tf.general.x21 as u64);
                        uart_puts(" x25=");    uart_put_hex(tf.general.x25 as u64);
                        uart_puts(" x27=");    uart_put_hex(tf.general.x27 as u64);
                        uart_puts(" x30=");    uart_put_hex(tf.general.x30 as u64);
                        uart_puts(" SP=");     uart_put_hex(tf.sp as u64);
                        uart_puts(" TPIDR=");  uart_put_hex(tf.tpidr as u64);
                        uart_puts("\n");
                        loop {
                            unsafe { core::arch::asm!("wfi"); }
                        }
                    }
                    TrapCause::UndefinedInstruction => {
                        uart_puts("[UDEF] undefined instruction at ELR=");
                        uart_put_hex(tf.elr as u64);
                        uart_puts(" ESR=");
                        uart_put_hex(esr);
                        uart_puts("\n");
                        loop {
                            unsafe { core::arch::asm!("wfi"); }
                        }
                    }
                    TrapCause::Breakpoint => {
                        uart_puts("[BRK] breakpoint at ELR=");
                        uart_put_hex(tf.elr as u64);
                        uart_puts("\n");
                        tf.elr += 4; // skip past BRK #imm
                    }
                    _ => {
                        uart_puts("[KILL] unhandled user exception, ESR=");
                        uart_put_hex(esr);
                        uart_puts(" ELR=");
                        uart_put_hex(tf.elr as u64);
                        uart_puts(" FAR=");
                        uart_put_hex(tf.fault_addr() as u64);
                        uart_puts("\n");
                        loop {
                            unsafe { core::arch::asm!("wfi"); }
                        }
                    }
                }
            }
        }
    }

    fn handle_user_irq(_tf: &mut TrapFrame) {
        crate::kernel::irq::dispatch_irq();
    }

    fn handle_kernel_sync(tf: &mut TrapFrame) {
        let esr = tf.esr();
        let cause = TrapCause::from_aarch64_esr(esr);

        match cause {
            TrapCause::PageFaultLoad | TrapCause::PageFaultStore => {
                let fa = tf.fault_addr();
                uart_puts("[KERNEL PF] page fault at ");
                uart_put_hex(fa as u64);
                uart_puts(", elr=");
                uart_put_hex(tf.elr as u64);
                uart_puts("\n");
                loop { unsafe { core::arch::asm!("wfi"); } }
            }
            _ => {
                uart_puts("[KERNEL PANIC] unhandled sync exception, elr=");
                uart_put_hex(tf.elr as u64);
                uart_puts("\n");
                loop { unsafe { core::arch::asm!("wfi"); } }
            }
        }
    }

    fn handle_kernel_irq(tf: &mut TrapFrame) {
        // TODO
        // 与 user_irq 类似，处理内核态被中断打断的情况
        // 主要是时钟中断和设备中断
    }

    fn handle_fiq(_tf: &mut TrapFrame) {
        panic!("FIQ not supported yet");
    }

    fn handle_serror(_tf: &mut TrapFrame) {
        panic!("System Error (SError) detected!");
    }
}

// ---------------------------------------------------------------------------
// High-level entry points (call these from the arch trap handler)
// ---------------------------------------------------------------------------

/// Initialise the architecture's trap subsystem.
///
/// # Safety
/// Must be called exactly once per core, before enabling IRQs.
pub unsafe fn init() {
    aarch64::base::trap::init();

    aarch64::base::trap::handler::install_trap_handler::<CommonTrapHandler>();
}
