//! Exception Syndrome Register (ESR_EL1) decoding.
//!
//! `ESR_EL1` is a 32-bit register (for AArch64 exceptions) laid out as:
//!
//!   bits [31:26]  — EC   (Exception Class)
//!   bits [25]     — IL   (Instruction Length: 0=16-bit, 1=32-bit)
//!   bits [24:0]   — ISS  (Instruction Specific Syndrome)
//!
//! Reference: ARM ARMv8-A, section D1.10.4 (ESR_EL1) and D10.2.39 (ISS encoding).

// ---------------------------------------------------------------------------
// Fault status codes (extracted from the ISS field of Data/Instruction Aborts)
// ---------------------------------------------------------------------------

/// Data Fault Status Code / Instruction Fault Status Code.
///
/// Encoded in ISS[5:0] (bits [5:0] of the syndrome), with the DFSC/IFSC
/// distinction coming from the top-level EC value.
#[derive(Debug, PartialEq, Eq, Copy, Clone)]
pub enum Fault {
    /// Address size fault (level -1)
    AddressSize,
    /// Translation fault (level 0/1/2/3 from ISS[7:6])
    Translation,
    /// Access flag fault
    AccessFlag,
    /// Permission fault
    Permission,
    /// External abort (not a precise abort)
    External,
    /// External abort on a translation table walk (levels 0–3)
    ExternalTableWalk,
    /// Alignment fault
    Alignment,
    /// TLB conflict abort
    TlbConflict,
    /// Implementation-defined or reserved fault code
    Other(u8),
}

impl From<u32> for Fault {
    /// Decode the ISS low bits into a `Fault`.
    ///
    /// ISS[5:0] is the status code; ISS[7:6] encodes the translation
    /// level for Translation/AccessFlag/Permission faults.
    fn from(iss: u32) -> Fault {
        match iss & 0b111111 {
            // DFSC / IFSC encodings (D10.2.39, Table D10-23)
            0b000000 => Fault::AddressSize,
            0b000001 => Fault::Translation,   // actually reserved for IFSC; reuse
            0b000100 => Fault::Translation,   // level encoded in ISS[7:6]
            0b000101 => Fault::Translation,
            0b000110 => Fault::Translation,
            0b000111 => Fault::Translation,
            0b001000 => Fault::AccessFlag,
            0b001001 => Fault::AccessFlag,
            0b001010 => Fault::AccessFlag,
            0b001011 => Fault::AccessFlag,
            0b001100 => Fault::Permission,
            0b001101 => Fault::Permission,
            0b001110 => Fault::Permission,
            0b001111 => Fault::Permission,
            0b010000 => Fault::External,       // synchronous external abort
            0b010100 => Fault::External,       // external on translation table walk
            0b010101 => Fault::ExternalTableWalk,
            0b010110 => Fault::ExternalTableWalk,
            0b010111 => Fault::ExternalTableWalk,
            0b011000 => Fault::External,       // parity error
            0b011100 => Fault::External,       // parity error on table walk
            0b011101 => Fault::ExternalTableWalk,
            0b011110 => Fault::ExternalTableWalk,
            0b011111 => Fault::ExternalTableWalk,
            0b100000 => Fault::Alignment,
            0b110000 => Fault::TlbConflict,
            _ => Fault::Other((iss & 0b111111) as u8),
        }
    }
}

impl Fault {
    /// The translation level encoded in ISS[7:6], or None for faults
    /// that don't encode stage information.
    pub fn level(&self, _iss: u32) -> Option<u8> {
        match self {
            Fault::Translation | Fault::AccessFlag | Fault::Permission
            | Fault::ExternalTableWalk => Some((_iss >> 6) as u8 & 0b11),
            _ => None,
        }
    }

    /// True if this fault can be recovered by mapping a page
    /// (demand paging / copy-on-write).
    pub fn is_recoverable(&self) -> bool {
        matches!(
            self,
            Fault::Translation | Fault::AccessFlag | Fault::Permission
        )
    }
}

// ---------------------------------------------------------------------------
// Exception class (EC) constants
// ---------------------------------------------------------------------------

mod ec {
    pub const UNKNOWN: u32                = 0b000000;
    pub const TRAPPED_WFI_WFE: u32        = 0b000001;
    pub const TRAPPED_MCR_MRC: u32        = 0b000011;
    pub const TRAPPED_MCRR_MRRC: u32      = 0b000100;
    pub const TRAPPED_MCR_MRC2: u32       = 0b000101;
    pub const TRAPPED_LDC_STC: u32        = 0b000110;
    pub const TRAPPED_SIMD_FP: u32        = 0b000111;
    pub const TRAPPED_VMRS: u32           = 0b001000;
    pub const TRAPPED_MRRC: u32           = 0b001100;
    pub const ILLEGAL_EXECUTION_STATE: u32 = 0b001110;
    pub const SVC_AARCH64: u32            = 0b010101;
    pub const HVC_AARCH64: u32            = 0b010110;
    pub const SMC_AARCH64: u32            = 0b010111;
    pub const MSR_MRS_SYSTEM: u32         = 0b011000;
    pub const INSTRUCTION_ABORT_LOWER: u32 = 0b100000;
    pub const INSTRUCTION_ABORT_SAME: u32  = 0b100001;
    pub const PC_ALIGNMENT_FAULT: u32     = 0b100010;
    pub const DATA_ABORT_LOWER: u32       = 0b100100;
    pub const DATA_ABORT_SAME: u32        = 0b100101;
    pub const SP_ALIGNMENT_FAULT: u32     = 0b100110;
    pub const TRAPPED_FPU_LOWER: u32      = 0b101000;
    pub const TRAPPED_FPU_SAME: u32       = 0b101100;
    pub const SERROR: u32                 = 0b101111;
    pub const BREAKPOINT_LOWER: u32       = 0b110000;
    pub const BREAKPOINT_SAME: u32        = 0b110001;
    pub const SOFTWARE_STEP_LOWER: u32    = 0b110010;
    pub const SOFTWARE_STEP_SAME: u32     = 0b110011;
    pub const WATCHPOINT_LOWER: u32       = 0b110100;
    pub const WATCHPOINT_SAME: u32        = 0b110101;
    pub const BRK: u32                    = 0b111100;
}

// ---------------------------------------------------------------------------
// Syndrome — decoded view of ESR_EL1
// ---------------------------------------------------------------------------

/// A fully decoded exception syndrome.
///
/// Constructed from the raw 32-bit `ESR_EL1` value by `Syndrome::from(esr)`.
#[derive(Debug, PartialEq, Eq, Copy, Clone)]
pub enum Syndrome {
    /// Reason unknown or not yet classified
    Unknown,
    /// WFI or WFE trapped
    WfiWfe,
    /// Trapped MCR/MRC access
    McrMrc,
    /// Trapped MCRR/MRRC access
    McrrMrrc,
    /// Trapped LDC/STC access
    LdcStc,
    /// Trapped SIMD/FP access
    SimdFp,
    /// Trapped VMRS access
    Vmrs,
    /// Trapped MRRC access
    Mrrc,
    /// Illegal execution state
    IllegalExecutionState,
    /// SVC (supervisor call) — the AArch64 syscall instruction.
    /// The immediate value (0–65535) is carried as the payload.
    Svc(u16),
    /// HVC (hypervisor call)
    Hvc(u16),
    /// SMC (secure monitor call)
    Smc(u16),
    /// Trapped MSR/MRS (system register access)
    MsrMrsSystem,
    /// Instruction abort (page fault on instruction fetch)
    InstructionAbort { kind: Fault, level: u8 },
    /// PC alignment fault
    PCAlignmentFault,
    /// Data abort (page fault on data access)
    DataAbort { kind: Fault, level: u8 },
    /// SP alignment fault
    SpAlignmentFault,
    /// Trapped FPU access from lower EL
    TrappedFpu,
    /// SError interrupt
    SError,
    /// Breakpoint exception
    Breakpoint,
    /// Software step exception (single-step debug)
    Step,
    /// Watchpoint exception
    Watchpoint,
    /// BRK instruction (software breakpoint)
    Brk(u16),
    /// Unrecognised EC value (raw EC passed as payload)
    Other(u32),
}

impl From<u64> for Syndrome {
    fn from(esr: u64) -> Self {
        Self::from_raw(esr as u32)
    }
}

impl From<u32> for Syndrome {
    fn from(esr: u32) -> Self {
        Self::from_raw(esr)
    }
}

impl Syndrome {
    /// Decode a raw 32-bit ESR value.
    pub fn from_raw(esr: u32) -> Self {
        use ec::*;

        let ec = esr >> 26;
        let iss = esr & 0x01FF_FFFF;
        let iss16 = (iss & 0xFFFF) as u16;

        match ec {
            UNKNOWN             => Syndrome::Unknown,
            TRAPPED_WFI_WFE     => Syndrome::WfiWfe,
            TRAPPED_MCR_MRC | TRAPPED_MCR_MRC2 => Syndrome::McrMrc,
            TRAPPED_MCRR_MRRC   => Syndrome::McrrMrrc,
            TRAPPED_LDC_STC     => Syndrome::LdcStc,
            TRAPPED_SIMD_FP     => Syndrome::SimdFp,
            TRAPPED_VMRS        => Syndrome::Vmrs,
            TRAPPED_MRRC        => Syndrome::Mrrc,
            ILLEGAL_EXECUTION_STATE => Syndrome::IllegalExecutionState,
            SVC_AARCH64 | 0b010001 => Syndrome::Svc(iss16), // 0b010001 = SVC from same EL
            HVC_AARCH64 | 0b010010 => Syndrome::Hvc(iss16),
            SMC_AARCH64 | 0b010011 => Syndrome::Smc(iss16),
            MSR_MRS_SYSTEM      => Syndrome::MsrMrsSystem,
            INSTRUCTION_ABORT_LOWER | INSTRUCTION_ABORT_SAME => {
                let kind = Fault::from(iss);
                let level = (iss >> 6) as u8 & 0b11;
                Syndrome::InstructionAbort { kind, level }
            }
            PC_ALIGNMENT_FAULT  => Syndrome::PCAlignmentFault,
            DATA_ABORT_LOWER | DATA_ABORT_SAME => {
                let kind = Fault::from(iss);
                let level = (iss >> 6) as u8 & 0b11;
                Syndrome::DataAbort { kind, level }
            }
            SP_ALIGNMENT_FAULT  => Syndrome::SpAlignmentFault,
            TRAPPED_FPU_LOWER | TRAPPED_FPU_SAME => Syndrome::TrappedFpu,
            SERROR              => Syndrome::SError,
            BREAKPOINT_LOWER | BREAKPOINT_SAME => Syndrome::Breakpoint,
            SOFTWARE_STEP_LOWER | SOFTWARE_STEP_SAME => Syndrome::Step,
            WATCHPOINT_LOWER | WATCHPOINT_SAME => Syndrome::Watchpoint,
            BRK                 => Syndrome::Brk(iss16),
            other               => Syndrome::Other(other),
        }
    }

    /// True if this syndrome represents a page fault that might be
    /// resolved through demand paging.
    pub fn is_page_fault(&self) -> bool {
        matches!(
            self,
            Syndrome::DataAbort {
                kind: Fault::Translation | Fault::AccessFlag | Fault::Permission,
                ..
            }
            | Syndrome::InstructionAbort {
                kind: Fault::Translation | Fault::AccessFlag | Fault::Permission,
                ..
            }
        )
    }

    /// True if an SVC (syscall) instruction triggered this exception.
    pub fn is_syscall(&self) -> bool {
        matches!(self, Syndrome::Svc(_))
    }
}
