//! RISC-V CSR 访问。接口与 aarch64 crate 的 `regs` 模块一致。

use core::arch::asm;

// ---------------------------------------------------------------------------
// 中断开关（sstatus.SIE）
// ---------------------------------------------------------------------------

#[inline]
pub unsafe fn irq_enable() {
    asm!("csrsi sstatus, 2"); // SIE = bit 1
}

#[inline]
pub unsafe fn irq_disable() {
    asm!("csrci sstatus, 2"); // SIE = bit 1
}

#[inline]
pub unsafe fn fiq_enable() {
    // RISC-V 无 FIQ
}

#[inline]
pub unsafe fn fiq_disable() {
    // RISC-V 无 FIQ
}

#[inline]
pub fn daif_read() -> u64 {
    let flags: usize;
    unsafe { asm!("csrr {}, sstatus", out(reg) flags) };
    flags as u64
}

#[inline]
pub fn irq_disable_and_store() -> u64 {
    let flags = daif_read();
    unsafe { irq_disable() };
    flags
}

#[inline]
pub unsafe fn daif_restore(flags: u64) {
    asm!("csrw sstatus, {}", in(reg) flags as usize);
}

#[inline]
pub fn wfe() {
    unsafe { asm!("wfi") };
}

// ---------------------------------------------------------------------------
// Fault address / trap cause
// ---------------------------------------------------------------------------

/// 读取 stval（faulting virtual address）。
#[inline]
pub fn far_el1_read() -> u64 {
    let val: usize;
    unsafe { asm!("csrr {}, stval", out(reg) val) };
    val as u64
}

/// 读取 scause 的原始值。
#[inline]
pub fn scause_read() -> usize {
    let val: usize;
    unsafe { asm!("csrr {}, scause", out(reg) val) };
    val
}

/// 把 RISC-V scause 翻译成 AArch64 ESR_EL1 格式（合成值）。
///
/// 这样 `common` 里的 `TrapCause::from_aarch64_esr` 就能直接解码，无需改动。
/// ESR 的 EC 字段位于 bits[31:26]，WnR（数据访问是写还是读）位于 bit 6。
#[inline]
pub fn esr_el1_read() -> u64 {
    let scause = scause_read();
    let is_interrupt = scause >> 63 != 0;
    let cause = scause & 0x7FFF_FFFF;

    if is_interrupt {
        // 中断由 IRQ 路径处理，这里返回 0 即可（同步异常才解码 ESR）。
        return 0;
    }

    let ec: u64 = match cause {
        8 => 0b010101, // ecall from U-mode → SVC（系统调用）
        12 => 0b100000, // instruction page fault → Instruction Abort
        13 => 0b100100, // load page fault → Data Abort (read)
        15 => 0b100101, // store page fault → Data Abort (write)
        2 => 0b000000, // illegal instruction
        3 => 0b110000, // breakpoint
        other => (other as u64) & 0x3F,
    };
    ec << 26
}
