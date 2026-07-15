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

    // Debug: print what we're saving (BEFORE)
    uart_puts("[REFL] save_area=");
    uart_put_hex(save_area as u64);
    uart_puts(" handler_pc=");
    uart_put_hex(handler_pc as u64);
    uart_puts("\n[REFL] BEFORE: tf.elr=");
    uart_put_hex(tf.elr as u64);
    uart_puts(" tf.x0=");
    uart_put_hex(tf.general.x0 as u64);
    uart_puts(" tf.x8=");
    uart_put_hex(tf.general.x8 as u64);
    uart_puts("\n");

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

    // Verify: read back first 2 words from save_area
    uart_puts("[REFL] VERIFY save_area[0]=");
    uart_put_hex(unsafe { core::ptr::read_volatile(save_area as *const u64) });
    uart_puts(" [1]=");
    uart_put_hex(unsafe { core::ptr::read_volatile((save_area as *const u64).add(1)) });
    uart_puts("\n");

    // Redirect execution to liblinux handler
    tf.elr = handler_pc;
    tf.general.x0 = save_area;
    tf.spsr = 0; // EL0t

    // Debug: AFTER modification
    uart_puts("[REFL] AFTER:  tf.elr=");
    uart_put_hex(tf.elr as u64);
    uart_puts(" tf.x0=");
    uart_put_hex(tf.general.x0 as u64);
    uart_puts(" RAW[280]=");
    uart_put_hex(unsafe { core::ptr::read_volatile((tf as *const TrapFrame as *const u8).add(280) as *const u64) });
    uart_puts("\n");
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
        56  => "openat",
        57  => "close",
        63  => "read",
        64  => "write",
        80  => "fstat",
        93  => "exit",
        94  => "exit_group",
        96  => "set_tid_address",
        160 => "uname",
        172 => "getpid",
        174 => "getuid",
        175 => "geteuid",
        176 => "getgid",
        177 => "getegid",
        214 => "brk",
        215 => "munmap",
        222 => "mmap",
        278 => "getrandom",
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
        _  => "?",
    }
}

impl TrapHandler for CommonTrapHandler {
    fn handle_user_sync(tf: &mut TrapFrame) {
        let esr = tf.esr();
        let ec = ((esr >> 26) & 0x3F) as usize;

        // Debug: print first few user exceptions with classification
        {
            static mut DBG_COUNT: usize = 0;
            unsafe {
                if DBG_COUNT < 20 {
                    DBG_COUNT += 1;
                    if ec == 0x15 {
                        let svc_imm = (esr & 0xFFFF) as u32;
                        let nr = tf.general.x8;
                        match svc_imm {
                            0 => {
                                // Linux syscall
                                let name = linux_syscall_name(nr);
                                uart_puts("[TRAP] SVC#0 LINUX nr=");
                                uart_put_hex(nr as u64);
                                uart_puts(" (");
                                uart_puts(name);
                                uart_puts(") ELR=");
                                uart_put_hex(tf.elr as u64);
                                uart_puts(" x0=");
                                uart_put_hex(tf.general.x0 as u64);
                            }
                            1 => {
                                // Native syscall
                                let name = native_syscall_name(nr);
                                uart_puts("[TRAP] SVC#1 NATIVE nr=");
                                uart_put_hex(nr as u64);
                                uart_puts(" (");
                                uart_puts(name);
                                uart_puts(")");
                                if nr == 8 {
                                    uart_puts(" x0=");
                                    uart_put_hex(tf.general.x0 as u64);
                                    uart_puts(" x1=");
                                    uart_put_hex(tf.general.x1 as u64);
                                }
                                uart_puts(" ELR=");
                                uart_put_hex(tf.elr as u64);
                            }
                            _ => {
                                uart_puts("[TRAP] SVC#");
                                uart_put_hex(svc_imm as u64);
                                uart_puts(" UNKNOWN ELR=");
                                uart_put_hex(tf.elr as u64);
                            }
                        }
                    } else {
                        uart_puts("[TRAP] user EC=");
                        uart_put_hex(ec as u64);
                        uart_puts(" ELR=");
                        uart_put_hex(tf.elr as u64);
                    }
                    uart_puts("\n");
                }
            }
        }

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
                        uart_puts(" x3=");     uart_put_hex(tf.general.x3 as u64);
                        uart_puts("\n");
                        uart_puts("[PF] x4="); uart_put_hex(tf.general.x4 as u64);
                        uart_puts(" x5=");     uart_put_hex(tf.general.x5 as u64);
                        uart_puts(" x6=");     uart_put_hex(tf.general.x6 as u64);
                        uart_puts(" x7=");     uart_put_hex(tf.general.x7 as u64);
                        uart_puts(" x8=");     uart_put_hex(tf.general.x8 as u64);
                        uart_puts("\n");
                        uart_puts("[PF] x9="); uart_put_hex(tf.general.x9 as u64);
                        uart_puts(" x10=");    uart_put_hex(tf.general.x10 as u64);
                        uart_puts(" x11=");    uart_put_hex(tf.general.x11 as u64);
                        uart_puts(" x12=");    uart_put_hex(tf.general.x12 as u64);
                        uart_puts(" SP=");     uart_put_hex(tf.sp as u64);
                        uart_puts("\n");
                        // Dump buffer contents: buf is at __init_libc sp + 0x60
                        // __init_libc sp = tf.sp + 16 (undo __init_tls's stp [sp,#-16]!)
                        {
                            // Calculate __init_libc's stack pointer
                            let init_libc_sp = tf.sp + 16;
                            let buf_base = init_libc_sp + 0x60;
                            uart_puts("[BUF] init_libc_sp=");
                            uart_put_hex(init_libc_sp as u64);
                            uart_puts(" buf=");
                            uart_put_hex(buf_base as u64);
                            uart_puts("\n");

                            // Scan all buf entries [0..38) — 304 bytes / 8
                            for i in 0_usize..38 {
                                let ptr = (buf_base + i * 8) as *const u64;
                                let val = unsafe { core::ptr::read_volatile(ptr) };
                                if val != 0 {
                                    uart_puts("[BUF]   [");
                                    uart_put_hex(i as u64);
                                    uart_puts("]=");
                                    uart_put_hex(val);
                                    uart_puts("\n");
                                }
                            }
                        }

                        // Dump raw auxv on initial user stack (set up by liblinux)
                        // USER_STACK_TOP = 0x7FFFFFF10000, liblinux writes auxv at TOP - 4096
                        {
                            let auxv_base = 0x7FFFFFF10000usize - 4096;
                            uart_puts("[AUXV] base=");
                            uart_put_hex(auxv_base as u64);
                            uart_puts("\n");
                            for i in 0_usize..32 {
                                let ptr = (auxv_base + i * 8) as *const u64;
                                let val = unsafe { core::ptr::read_volatile(ptr) };
                                if val != 0 || i <= 15 {
                                    uart_puts("[AUXV]  +");
                                    uart_put_hex((i * 8) as u64);
                                    uart_puts("=");
                                    uart_put_hex(val);
                                    uart_puts("\n");
                                }
                            }
                        }

                        // Read BootInfo at 0x204028 directly
                        {
                            let bi_base = 0x204028usize;
                            uart_puts("[BOOT] addr=");
                            uart_put_hex(bi_base as u64);
                            uart_puts("\n");
                            for i in 0_usize..6 {
                                let ptr = (bi_base + i * 8) as *const u64;
                                let val = unsafe { core::ptr::read_volatile(ptr) };
                                uart_puts("[BOOT]  +");
                                uart_put_hex((i * 8) as u64);
                                uart_puts("=");
                                uart_put_hex(val);
                                uart_puts("\n");
                            }
                        }

                        // Translate key VAs to PAs to detect page aliasing
                        {
                            use crate::kernel::trap::context::TrapFrame;
                            // Use aarch64::base::mm::PageTable
                            let l0_pa = *crate::KERNEL_L0_PA.lock();
                            let pt = aarch64::base::mm::page_table::PageTable::from_token(l0_pa);
                            let vas = [
                                (0x200000usize, "liblinux_code"),
                                (0x7FFF_FFF0_F000usize, "user_stack"),
                                (0x204028usize, "bootinfo"),
                            ];
                            for (va, name) in &vas {
                                match pt.translate_va_to_pa(aarch64::base::mm::VirtAddr(*va)) {
                                    Some(pa) => {
                                        uart_puts("[PT] ");
                                        uart_puts(name);
                                        uart_puts(" VA=");
                                        uart_put_hex(*va as u64);
                                        uart_puts(" PA=");
                                        uart_put_hex(pa as u64);
                                        uart_puts("\n");
                                    }
                                    None => {
                                        uart_puts("[PT] ");
                                        uart_puts(name);
                                        uart_puts(" UNMAPPED\n");
                                    }
                                }
                            }
                        }
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

    fn handle_user_irq(tf: &mut TrapFrame) {
        // TODO
        // 读取中断控制器的状态 (例如 GIC)，判断是 Timer 还是外设
        // 如果是 Timer -> 触发调度器 sched::yield()
        // 如果是 磁盘/网卡 -> 触发相应的驱动回调
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
