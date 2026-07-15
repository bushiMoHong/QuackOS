// SPDX-License-Identifier: MIT
// AArch64 exception vector table and context save/restore.

.section .text

// ---------------------------------------------------------------------------
// __alltraps — 统一异常入口
// ---------------------------------------------------------------------------
.global __alltraps
__alltraps:
    // 此时硬件/HANDLER 已经在栈顶 [sp, #272] 和 [sp, #280] 处压入了 lr(x30) 和 x0
    // 我们直接一次性向下平移栈指针，预留出整张 TrapFrame 的空间 (共 288 字节，去掉已压入的 16 字节 = 272)
    sub     sp, sp, #272

    // 严格按照 Rust 中 GeneralRegs 的内存偏移量，使用绝对地址存入各个通用寄存器
    stp     x1,  x2,  [sp, #40]
    stp     x3,  x4,  [sp, #56]
    stp     x5,  x6,  [sp, #72]
    stp     x7,  x8,  [sp, #88]
    stp     x9,  x10, [sp, #104]
    stp     x11, x12, [sp, #120]
    stp     x13, x14, [sp, #136]
    stp     x15, x16, [sp, #152]
    stp     x17, x18, [sp, #168]
    stp     x19, x20, [sp, #184]
    stp     x21, x22, [sp, #200]
    stp     x23, x24, [sp, #216]
    stp     x25, x26, [sp, #232]
    stp     x27, x28, [sp, #248]
    str     x29,      [sp, #264]

    // 存入底部的状态和控制寄存器
    mrs     x1, elr_el1
    stp     x0, x1, [sp, #0]          // [sp, #0]=trap_num, [sp, #8]=elr

    mrs     x1, spsr_el1
    str     x1, [sp, #16]             // [sp, #16]=spsr

    // ------------------------------------------------------------------
    // 判断来源：Kernel 还是 User
    // ------------------------------------------------------------------
    mov     x1, #0x3
    and     x1, x1, x0
    cmp     x1, #2
    beq     trap_from_user

    // ---------- 来自内核的异常 ----------
trap_from_kernel:
    mrs     x2, tpidr_el1
    add     x1, sp, #288              // 计算异常发生前的内核 SP
    stp     x1, x2, [sp, #24]         // 填入 tf.sp (+24) 和 tf.tpidr (+32)

    mov     x0, sp
    bl      trap_handler

    // 安全恢复 tpidr_el1
    ldr     x1, [sp, #32]
    msr     tpidr_el1, x1

    b       trap_return

    // ---------- 来自用户态的异常 ----------
trap_from_user:
    mrs     x2, tpidr_el0
    mrs     x1, sp_el0
    stp     x1, x2, [sp, #24]         // 填入用户态的 sp 和 tpidr

    mov     x20, sp
    mrs     x2, tpidr_el1
    mov     sp, x2                    // 切换到事先准备好的内核栈

    mov     x0, x20
    bl      trap_handler

    mov     sp, x20
    b       trap_return


// ---------------------------------------------------------------------------
// trap_return — 绝对偏移量恢复环境
// ---------------------------------------------------------------------------
trap_return:
    // 从固定偏移量恢复状态寄存器
    ldr     x1, [sp, #8]
    msr     elr_el1, x1
    ldr     x1, [sp, #16]
    msr     spsr_el1, x1

    // If returning to EL0 (SPSR.M == 0), restore sp_el0 from tf.sp.
    // The handler may have changed tf.sp (e.g. linux_syscall_done restores
    // the original Linux binary's SP).  Without this, the user process
    // resumes with the wrong stack pointer and function epilogues that
    // reload x30 from the stack will branch to garbage.
    and     x2, x1, #0xf           // extract SPSR.M (low 4 bits)
    cbnz    x2, 1f                 // skip if not EL0t
    ldr     x1, [sp, #24]          // tf.sp
    msr     sp_el0, x1
1:
    // 从固定偏移量恢复所有通用寄存器（此时 x1 将被稳稳地恢复为 0x2222）
    ldp     x1,  x2,  [sp, #40]
    ldp     x3,  x4,  [sp, #56]
    ldp     x5,  x6,  [sp, #72]
    ldp     x7,  x8,  [sp, #88]
    ldp     x9,  x10, [sp, #104]
    ldp     x11, x12, [sp, #120]
    ldp     x13, x14, [sp, #136]
    ldp     x15, x16, [sp, #152]
    ldp     x17, x18, [sp, #168]
    ldp     x19, x20, [sp, #184]
    ldp     x21, x22, [sp, #200]
    ldp     x23, x24, [sp, #216]
    ldp     x25, x26, [sp, #232]
    ldp     x27, x28, [sp, #248]
    ldr     x29,      [sp, #264]

    // 恢复最初压入的 lr 和经过 Rust 修改后的 x0
    ldp     lr,  x0,  [sp, #272]

    // 彻底释放整张栈帧的空间 (288 字节)，恢复 SP
    add     sp, sp, #288
    eret


// ---------------------------------------------------------------------------
// 宏定义与其它辅助函数
// ---------------------------------------------------------------------------
.macro HANDLER source kind
    .align 7
    stp     lr, x0, [sp, #-16]!
    mov     x0, #\source
    movk    x0, #\kind, lsl #16
    b       __alltraps
.endm

.global __vectors
.align 11
__vectors:
    HANDLER 0 0; HANDLER 0 1; HANDLER 0 2; HANDLER 0 3
    HANDLER 1 0; HANDLER 1 1; HANDLER 1 2; HANDLER 1 3
    HANDLER 2 0; HANDLER 2 1; HANDLER 2 2; HANDLER 2 3
    HANDLER 3 0; HANDLER 3 1; HANDLER 3 2; HANDLER 3 3

.global run_user
run_user:
    // Save kernel stack pointer BEFORE pushing callee-saved regs,
    // so trap_from_user switches to a safe distance above TrapFrame.
    mov     x1, sp
    msr     tpidr_el1, x1

    stp     x29, x30, [sp, #-16]!
    stp     x27, x28, [sp, #-16]!
    stp     x25, x26, [sp, #-16]!
    stp     x23, x24, [sp, #-16]!
    stp     x21, x22, [sp, #-16]!
    stp     x19, x20, [sp, #-16]!             // 暂存内核栈

    mov     sp, x0                    // 切换到用户上下文

    ldp     x1, x2, [sp, #24]         // 读取用户 sp 和 tpidr
    msr     sp_el0, x1
    msr     tpidr_el0, x2

    b       trap_return