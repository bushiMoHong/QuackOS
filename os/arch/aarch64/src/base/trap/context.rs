//! Trap frame and user context structures for AArch64.
//!
//! These structures mirror the stack layout produced by `vector.S`.
//! They must be `#[repr(C)]` so their field order matches what the
//! assembly code reads and writes.

/// General-purpose registers for AArch64 (x0–x30).
///
/// x31 is the stack pointer or zero register depending on context
/// and is stored separately in `UserContext::sp`.
///
/// The field order follows the stack layout in `vector.S`:
/// registers are pushed from high to low index, so `x1` comes first.
#[derive(Debug, Default, Clone, Copy, Eq, PartialEq)]
#[repr(C)]
pub struct GeneralRegs {
    pub x1: usize,
    pub x2: usize,
    pub x3: usize,
    pub x4: usize,
    pub x5: usize,
    pub x6: usize,
    pub x7: usize,
    pub x8: usize,   // syscall number on Linux/aarch64
    pub x9: usize,
    pub x10: usize,
    pub x11: usize,
    pub x12: usize,
    pub x13: usize,
    pub x14: usize,
    pub x15: usize,
    pub x16: usize,
    pub x17: usize,
    pub x18: usize,  // platform register (used for TLS on some ABIs)
    pub x19: usize,  // callee-saved
    pub x20: usize,  // callee-saved
    pub x21: usize,  // callee-saved
    pub x22: usize,  // callee-saved
    pub x23: usize,  // callee-saved
    pub x24: usize,  // callee-saved
    pub x25: usize,  // callee-saved
    pub x26: usize,  // callee-saved
    pub x27: usize,  // callee-saved
    pub x28: usize,  // callee-saved
    pub x29: usize,  // frame pointer (callee-saved)
    pub x30: usize,  // link register (lr)
    // Placed last so `x0` can be loaded independently by asm
    pub x0: usize,   // argument / return value
}

/// The full trap frame saved on exception entry by `vector.S`.
///
/// This is the kernel-internal view.  It uses `sp_el1` for the stack
/// pointer and `tpidr_el1` for the thread ID, because kernel traps
/// stay on the same stack.
#[derive(Debug, Default, Clone, Copy)]
#[repr(C)]
pub struct TrapFrame {
    /// Exception source (low 16 bits) | exception kind (high 16 bits).
    ///   source: 0=SpEl0, 1=SpElx, 2=LowerAArch64, 3=LowerAArch32
    ///   kind:   0=Sync, 1=IRQ, 2=FIQ, 3=SError
    pub trap_num: usize,
    /// Exception Link Register (ELR_EL1) — return address
    pub elr: usize,
    /// Saved Processor State Register (SPSR_EL1)
    pub spsr: usize,
    /// Stack pointer at the time of exception:
    ///   - SP_EL1 for kernel traps
    ///   - SP_EL0 for user traps
    pub sp: usize,
    /// Thread ID register:
    ///   - TPIDR_EL1 for kernel traps
    ///   - TPIDR_EL0 for user traps
    pub tpidr: usize,
    /// General-purpose registers (must be last — see vector.S layout)
    pub general: GeneralRegs,
}

/// Userspace execution context.
///
/// Before entering userspace the kernel populates this struct with the
/// desired register state.  On return from a user exception the fields
/// reflect the state at the moment of the trap.
#[derive(Debug, Default, Clone, Copy, Eq, PartialEq)]
#[repr(C)]
pub struct UserContext {
    /// Exception source | kind — set by the hardware/vector stub on trap
    pub trap_num: usize,
    /// Exception Link Register (program counter to resume at)
    pub elr: usize,
    /// Saved Processor State Register
    pub spsr: usize,
    /// Userspace stack pointer (SP_EL0)
    pub sp: usize,
    /// Thread pointer / TLS base (TPIDR_EL0)
    pub tpidr: usize,
    /// General-purpose registers
    pub general: GeneralRegs,
}

impl UserContext {
    // ------------------------------------------------------------------
    // Syscall convention (Linux AArch64 ABI)
    //
    //   syscall number → x8
    //   arguments     → x0–x5
    //   return value  → x0
    // ------------------------------------------------------------------

    /// Return the syscall number (x8).
    #[inline]
    pub fn get_syscall_num(&self) -> usize {
        self.general.x8
    }

    /// Return the syscall return value (x0).
    #[inline]
    pub fn get_syscall_ret(&self) -> usize {
        self.general.x0
    }

    /// Set the syscall return value (x0).
    #[inline]
    pub fn set_syscall_ret(&mut self, ret: usize) {
        self.general.x0 = ret;
    }

    /// Return the six syscall arguments [x0, x1, x2, x3, x4, x5].
    #[inline]
    pub fn get_syscall_args(&self) -> [usize; 6] {
        [
            self.general.x0,
            self.general.x1,
            self.general.x2,
            self.general.x3,
            self.general.x4,
            self.general.x5,
        ]
    }

    // ------------------------------------------------------------------
    // Convenience accessors
    // ------------------------------------------------------------------

    /// Set the instruction pointer (ELR_EL1 → return address on eret).
    #[inline]
    pub fn set_ip(&mut self, ip: usize) {
        self.elr = ip;
    }

    /// Get the instruction pointer.
    #[inline]
    pub fn get_ip(&self) -> usize {
        self.elr
    }

    /// Set the userspace stack pointer.
    #[inline]
    pub fn set_sp(&mut self, sp: usize) {
        self.sp = sp;
    }

    /// Get the userspace stack pointer.
    #[inline]
    pub fn get_sp(&self) -> usize {
        self.sp
    }

    /// Set the TLS base (TPIDR_EL0).
    #[inline]
    pub fn set_tls(&mut self, tls: usize) {
        self.tpidr = tls;
    }

    /// Get the TLS base.
    #[inline]
    pub fn get_tls(&self) -> usize {
        self.tpidr
    }

    /// Enter userspace with this context.
    ///
    /// This call only returns when an exception traps back into the kernel.
    /// On return, the context fields hold the state at the moment of the trap.
    ///
    /// # Safety
    ///
    /// Caller must ensure the vector table is installed and interrupts are
    /// configured so that some path exists to return to kernel mode.
    pub fn run(&mut self) {
        extern "C" {
            fn run_user(ctx: &mut UserContext);
        }
        unsafe { run_user(self) }
    }
}

impl TrapFrame {
    /// Decode the exception source from trap_num.
    #[inline]
    pub fn source(&self) -> ExceptionSource {
        ExceptionSource::from(self.trap_num & 0xFFFF)
    }

    /// Decode the exception kind from trap_num.
    #[inline]
    pub fn kind(&self) -> ExceptionKind {
        ExceptionKind::from(self.trap_num >> 16)
    }

    /// Get the fault address (FAR_EL1) for data/instruction aborts.
    #[inline]
    pub fn fault_addr(&self) -> usize {
        super::regs::far_el1_read() as usize
    }

    /// Read ESR_EL1 (Exception Syndrome Register).
    #[inline]
    pub fn esr(&self) -> u64 {
        super::regs::esr_el1_read()
    }
}

// ---------------------------------------------------------------------------
// Exception source / kind enums
// ---------------------------------------------------------------------------

/// Where the exception was taken from.
///
/// Encoded in the low 16 bits of `trap_num` by the HANDLER macro.
#[derive(Debug, PartialEq, Eq, Copy, Clone)]
#[repr(usize)]
pub enum ExceptionSource {
    /// Exception from current EL with SP_EL0 (e.g., EL1 with thread stack)
    CurrentSpEl0 = 0,
    /// Exception from current EL with SP_ELx (e.g., EL1 with handler stack)
    CurrentSpElx = 1,
    /// Exception from lower EL running AArch64 (userspace)
    LowerAArch64 = 2,
    /// Exception from lower EL running AArch32
    LowerAArch32 = 3,
}

impl From<usize> for ExceptionSource {
    fn from(x: usize) -> Self {
        match x {
            0 => ExceptionSource::CurrentSpEl0,
            1 => ExceptionSource::CurrentSpElx,
            2 => ExceptionSource::LowerAArch64,
            3 => ExceptionSource::LowerAArch32,
            _ => panic!("invalid exception source: {}", x),
        }
    }
}

/// What kind of exception occurred.
///
/// Encoded in the high 16 bits of `trap_num` by the HANDLER macro.
#[derive(Debug, PartialEq, Eq, Copy, Clone)]
#[repr(usize)]
pub enum ExceptionKind {
    Synchronous = 0,
    Irq = 1,
    Fiq = 2,
    SError = 3,
}

impl From<usize> for ExceptionKind {
    fn from(x: usize) -> Self {
        match x {
            0 => ExceptionKind::Synchronous,
            1 => ExceptionKind::Irq,
            2 => ExceptionKind::Fiq,
            3 => ExceptionKind::SError,
            _ => panic!("invalid exception kind: {}", x),
        }
    }
}
