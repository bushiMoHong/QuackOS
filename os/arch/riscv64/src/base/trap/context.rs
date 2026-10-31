//! RISC-V trap 帧和用户上下文结构。
//!
//! 字段名与 aarch64 crate 保持一致（`GeneralRegs.x0..x30`），使上层 `common`
//! 无需改动。底层 RISC-V 寄存器到这些字段的映射由 `trap.S` 完成。

/// 通用寄存器（与 aarch64 的 `GeneralRegs` 字段名一致：x0–x30）。
///
/// RISC-V 有 x0–x31 共 32 个寄存器，这里按 aarch64 的字段命名 x0–x30 保存，
/// x31（帧指针）不单独暴露。字段顺序与 `trap.S` 的栈布局对应。
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
    pub x8: usize, // 系统调用号（RISC-V 下映射为 a7）
    pub x9: usize,
    pub x10: usize,
    pub x11: usize,
    pub x12: usize,
    pub x13: usize,
    pub x14: usize,
    pub x15: usize,
    pub x16: usize,
    pub x17: usize,
    pub x18: usize,
    pub x19: usize, // callee-saved
    pub x20: usize,
    pub x21: usize,
    pub x22: usize,
    pub x23: usize,
    pub x24: usize,
    pub x25: usize,
    pub x26: usize,
    pub x27: usize,
    pub x28: usize,
    pub x29: usize,
    pub x30: usize,
    pub x0: usize, // 参数 / 返回值
}

/// 异常入口保存的完整 trap 帧。
#[derive(Debug, Default, Clone, Copy)]
#[repr(C)]
pub struct TrapFrame {
    /// 异常来源（低 16 位）| 异常类型（高 16 位）。
    pub trap_num: usize,
    /// 异常 PC（sepc）—— 返回地址
    pub elr: usize,
    /// 保存的状态寄存器（sstatus）
    pub spsr: usize,
    /// 异常时的栈指针
    pub sp: usize,
    /// 线程 ID（tp）
    pub tpidr: usize,
    /// 通用寄存器
    pub general: GeneralRegs,
}

/// 用户态执行上下文。
#[derive(Debug, Default, Clone, Copy, Eq, PartialEq)]
#[repr(C)]
pub struct UserContext {
    pub trap_num: usize,
    pub elr: usize,
    pub spsr: usize,
    pub sp: usize,
    pub tpidr: usize,
    pub general: GeneralRegs,
}

impl UserContext {
    #[inline]
    pub fn get_syscall_num(&self) -> usize {
        self.general.x8
    }

    #[inline]
    pub fn get_syscall_ret(&self) -> usize {
        self.general.x0
    }

    #[inline]
    pub fn set_syscall_ret(&mut self, ret: usize) {
        self.general.x0 = ret;
    }

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

    #[inline]
    pub fn set_ip(&mut self, ip: usize) {
        self.elr = ip;
    }

    #[inline]
    pub fn get_ip(&self) -> usize {
        self.elr
    }

    #[inline]
    pub fn set_sp(&mut self, sp: usize) {
        self.sp = sp;
    }

    #[inline]
    pub fn get_sp(&self) -> usize {
        self.sp
    }

    #[inline]
    pub fn set_tls(&mut self, tls: usize) {
        self.tpidr = tls;
    }

    #[inline]
    pub fn get_tls(&self) -> usize {
        self.tpidr
    }

    /// 进入用户态。该调用只在异常陷入内核时返回。
    pub fn run(&mut self) {
        extern "C" {
            fn run_user(ctx: &mut UserContext);
        }
        unsafe { run_user(self) }
    }
}

impl TrapFrame {
    #[inline]
    pub fn source(&self) -> ExceptionSource {
        ExceptionSource::from(self.trap_num & 0xFFFF)
    }

    #[inline]
    pub fn kind(&self) -> ExceptionKind {
        ExceptionKind::from(self.trap_num >> 16)
    }

    /// 获取 fault address（stval）。
    #[inline]
    pub fn fault_addr(&self) -> usize {
        super::regs::far_el1_read() as usize
    }

    /// 读取「合成 ESR」——把 RISC-V scause 翻译成 AArch64 ESR_EL1 格式，
    /// 使上层 `common` 的 `from_aarch64_esr` 无需改动即可解码。
    #[inline]
    pub fn esr(&self) -> u64 {
        super::regs::esr_el1_read()
    }
}

// ---------------------------------------------------------------------------
// Exception source / kind enums
// ---------------------------------------------------------------------------

/// 异常来源（用户态 / 内核态）。
///
/// RISC-V 只有用户态（U-mode）和内核态（S-mode）两种，映射到 aarch64 的
/// `LowerAArch64`（用户）和 `CurrentSpElx`（内核）。
#[derive(Debug, PartialEq, Eq, Copy, Clone)]
#[repr(usize)]
pub enum ExceptionSource {
    CurrentSpEl0 = 0,
    CurrentSpElx = 1,
    LowerAArch64 = 2,
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

/// 异常类型（同步异常 / 中断）。
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
