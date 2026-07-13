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
            0b000000 | 0b000100 | 0b000101 => {
                // Unknown reason, breakpoint, etc.
                let iss = (esr & 0x1FF_FFFF) as usize;
                match iss {
                    0x00 => TrapCause::Breakpoint,
                    _ => TrapCause::Other(ec),
                }
            }
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

    // Get per-thread handler pc and save_area
    let (handler_pc, save_area) = sche::with_thread(tid, |thread| {
        (thread.linux_handler_pc, thread.linux_save_area)
    })
    .unwrap_or((None, None));

    let handler_pc = match handler_pc {
        Some(pc) => pc,
        None => {
            // liblinux hasn't registered a handler — fall back to spin
            loop { unsafe { core::arch::asm!("wfi"); } }
        }
    };

    let save_area = match save_area {
        Some(addr) => addr,
        None => loop { unsafe { core::arch::asm!("wfi"); } },
    };

    // Build the LinuxContext from the current trap frame (usize → u64)
    let ctx = LinuxContext {
        x0: tf.general.x0 as u64, x1: tf.general.x1 as u64,
        x2: tf.general.x2 as u64, x3: tf.general.x3 as u64,
        x4: tf.general.x4 as u64, x5: tf.general.x5 as u64,
        x6: tf.general.x6 as u64, x7: tf.general.x7 as u64,
        x8: tf.general.x8 as u64,
        x9: tf.general.x9 as u64, x10: tf.general.x10 as u64,
        x11: tf.general.x11 as u64, x12: tf.general.x12 as u64,
        x13: tf.general.x13 as u64, x14: tf.general.x14 as u64,
        x15: tf.general.x15 as u64, x16: tf.general.x16 as u64,
        x17: tf.general.x17 as u64, x18: tf.general.x18 as u64,
        x19: tf.general.x19 as u64, x20: tf.general.x20 as u64,
        x21: tf.general.x21 as u64, x22: tf.general.x22 as u64,
        x23: tf.general.x23 as u64, x24: tf.general.x24 as u64,
        x25: tf.general.x25 as u64, x26: tf.general.x26 as u64,
        x27: tf.general.x27 as u64, x28: tf.general.x28 as u64,
        x29: tf.general.x29 as u64, x30: tf.general.x30 as u64,
        elr: tf.elr as u64,
        spsr: tf.spsr as u64,
        sp: tf.sp as u64,
    };

    // Write the context to user-space save_area (liblinux will read it)
    unsafe {
        core::ptr::write_volatile(save_area as *mut LinuxContext, ctx);
    }

    // Redirect execution to the liblinux handler.
    // When trap_return runs, it will eret into the handler with:
    //   x0  = save_area vaddr (pointer to the saved context above)
    //   ELR = handler_pc
    //   SPSR = user mode (EL0t)
    //   SP  = we need a stack pointer for the handler...
    //
    // For the initial implementation, the liblinux handler uses the same
    // user stack.  A dedicated handler stack (stored per-thread) will be
    // added when multi-threaded Linux programs need it (§8.7).
    tf.elr = handler_pc;
    tf.general.x0 = save_area;
    tf.spsr = 0; // EL0t with all exception levels masked
    // SP remains as-is (user stack); liblinux handler must be careful
    // not to overflow the user stack.
}

struct CommonTrapHandler;

/// UART output: write a single byte to the PL011 at 0x09000000.
fn uart_putc(c: u8) {
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
                        uart_puts("[PF] page fault at ");
                        uart_put_hex(fault_addr as u64);
                        uart_puts(" ELR=");
                        uart_put_hex(tf.elr as u64);
                        uart_puts(" ESR=");
                        uart_put_hex(esr);
                        uart_puts("\n");
                        loop {
                            unsafe { core::arch::asm!("wfi"); }
                        }
                    }
                    TrapCause::Breakpoint => {
                        uart_puts("[DBG] breakpoint\n");
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
