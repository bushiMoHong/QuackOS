# RISC-V trap 入口。
#
# TrapFrame 布局（36*8 = 288 字节，与 aarch64 一致）：
#   offset 0: trap_num = source | (kind << 16)
#   offset 1: elr     = sepc
#   offset 2: spsr    = sstatus
#   offset 3: sp
#   offset 4: tpidr   = tp
#   offset 5..34: general.x1..x30
#   offset 35: general.x0
#
# 寄存器映射（general 字段 → RISC-V 寄存器）：
#   x0→a0, x1→a1, x2→a2, x3→a3, x4→a4, x5→a5, x6→a6, x7→a7,
#   x8→a7(系统调用号), x9→ra, x10→sp, x11→gp, x12→tp,
#   x13→t0, x14→t1, x15→t2, x16→s0, x17→s1, x18→s2, x19→s3,
#   x20→s4, x21→s5, x22→s6, x23→s7, x24→s8, x25→s9, x26→s10,
#   x27→s11, x28→t3, x29→t4, x30→t5
#
# 约定：sscratch 在用户态保存内核栈指针；进入 trap 时交换 sp 与 sscratch。

    .section .text
    .globl __trap_from_user
    .globl __trap_from_kernel
    .globl __return_to_user
    .globl run_user
    .align 2

# ── 用户态 trap 入口 ──────────────────────────────────────────────────────
__trap_from_user:
    csrrw sp, sscratch, sp       # 交换 sp 和 sscratch（sscratch 存内核栈）
    addi sp, sp, -36*8           # 分配 TrapFrame

    # 保存 sepc / sstatus / 用户 sp / tp
    csrr t0, sepc
    csrr t1, sstatus
    sd t0, 1*8(sp)               # elr = sepc
    sd t1, 2*8(sp)               # spsr = sstatus
    csrr t0, sscratch            # 用户 sp 现在在 sscratch
    sd t0, 3*8(sp)               # TrapFrame.sp = 用户 sp
    sd tp, 4*8(sp)               # tpidr = tp

    # 保存通用寄存器（见顶部映射表）
    sd x10, 35*8(sp)             # x0  = a0
    sd x11, 5*8(sp)              # x1  = a1
    sd x12, 6*8(sp)              # x2  = a2
    sd x13, 7*8(sp)              # x3  = a3
    sd x14, 8*8(sp)              # x4  = a4
    sd x15, 9*8(sp)              # x5  = a5
    sd x16, 10*8(sp)             # x6  = a6
    sd x17, 11*8(sp)             # x7  = a7
    sd x17, 12*8(sp)             # x8  = a7（系统调用号）
    sd x1,  13*8(sp)             # x9  = ra
    sd x2,  14*8(sp)             # x10 = sp
    sd x3,  15*8(sp)             # x11 = gp
    sd x4,  16*8(sp)             # x12 = tp
    sd x5,  17*8(sp)             # x13 = t0
    sd x6,  18*8(sp)             # x14 = t1
    sd x7,  19*8(sp)             # x15 = t2
    sd x8,  20*8(sp)             # x16 = s0
    sd x9,  21*8(sp)             # x17 = s1
    sd x18, 22*8(sp)             # x18 = s2
    sd x19, 23*8(sp)             # x19 = s3
    sd x20, 24*8(sp)             # x20 = s4
    sd x21, 25*8(sp)             # x21 = s5
    sd x22, 26*8(sp)             # x22 = s6
    sd x23, 27*8(sp)             # x23 = s7
    sd x24, 28*8(sp)             # x24 = s8
    sd x25, 29*8(sp)             # x25 = s9
    sd x26, 30*8(sp)             # x26 = s10
    sd x27, 31*8(sp)             # x27 = s11
    sd x28, 32*8(sp)             # x28 = t3
    sd x29, 33*8(sp)             # x29 = t4
    sd x30, 34*8(sp)             # x30 = t5

    # 计算 trap_num = source | (kind << 16)，source=2(用户), kind=scause>>63
    csrr t0, scause
    srli t0, t0, 63
    slli t0, t0, 16
    li   t1, 2
    or   t0, t0, t1
    sd   t0, 0*8(sp)

    # 进入内核态，后续 trap 走 __trap_from_kernel
    la t0, __trap_from_kernel
    csrw stvec, t0

    mv a0, sp
    call trap_handler
    j __return_to_user

# ── 返回用户态 ────────────────────────────────────────────────────────────
__return_to_user:
    ld t0, 1*8(sp)
    ld t1, 2*8(sp)
    csrw sepc, t0
    csrw sstatus, t1
    ld tp, 4*8(sp)

    # 恢复通用寄存器（逆映射）
    ld x11, 5*8(sp)              # a1  ← x1
    ld x12, 6*8(sp)              # a2  ← x2
    ld x13, 7*8(sp)              # a3  ← x3
    ld x14, 8*8(sp)              # a4  ← x4
    ld x15, 9*8(sp)              # a5  ← x5
    ld x16, 10*8(sp)             # a6  ← x6
    ld x17, 11*8(sp)             # a7  ← x7
    # x8 不单独恢复（与 x7 同为 a7）
    ld x1,  13*8(sp)             # ra  ← x9
    # sp 稍后恢复
    ld x3,  15*8(sp)             # gp  ← x11
    ld x4,  16*8(sp)             # tp  ← x12
    ld x5,  17*8(sp)             # t0  ← x13
    ld x6,  18*8(sp)             # t1  ← x14
    ld x7,  19*8(sp)             # t2  ← x15
    ld x8,  20*8(sp)             # s0  ← x16
    ld x9,  21*8(sp)             # s1  ← x17
    ld x18, 22*8(sp)             # s2  ← x18
    ld x19, 23*8(sp)             # s3  ← x19
    ld x20, 24*8(sp)             # s4  ← x20
    ld x21, 25*8(sp)             # s5  ← x21
    ld x22, 26*8(sp)             # s6  ← x22
    ld x23, 27*8(sp)             # s7  ← x23
    ld x24, 28*8(sp)             # s8  ← x24
    ld x25, 29*8(sp)             # s9  ← x25
    ld x26, 30*8(sp)             # s10 ← x26
    ld x27, 31*8(sp)             # s11 ← x27
    ld x28, 32*8(sp)             # t3  ← x28
    ld x29, 33*8(sp)             # t4  ← x29
    ld x30, 34*8(sp)             # t5  ← x30
    ld x10, 35*8(sp)             # a0  ← x0

    # 恢复用户 sp 到 sscratch，释放 TrapFrame
    ld t2, 3*8(sp)
    csrw sscratch, t2
    addi sp, sp, 36*8
    csrrw sp, sscratch, sp

    la t0, __trap_from_user
    csrw stvec, t0
    sret

# ── 内核态 trap 入口 ──────────────────────────────────────────────────────
__trap_from_kernel:
    addi sp, sp, -36*8           # 分配 TrapFrame（不换栈）

    csrr t0, sepc
    csrr t1, sstatus
    sd t0, 1*8(sp)
    sd t1, 2*8(sp)
    sd sp, 3*8(sp)               # 注意：此时 sp 已指向 TrapFrame
    sd tp, 4*8(sp)

    sd x10, 35*8(sp)
    sd x11, 5*8(sp)
    sd x12, 6*8(sp)
    sd x13, 7*8(sp)
    sd x14, 8*8(sp)
    sd x15, 9*8(sp)
    sd x16, 10*8(sp)
    sd x17, 11*8(sp)
    sd x17, 12*8(sp)
    sd x1,  13*8(sp)
    sd x2,  14*8(sp)
    sd x3,  15*8(sp)
    sd x4,  16*8(sp)
    sd x5,  17*8(sp)
    sd x6,  18*8(sp)
    sd x7,  19*8(sp)
    sd x8,  20*8(sp)
    sd x9,  21*8(sp)
    sd x18, 22*8(sp)
    sd x19, 23*8(sp)
    sd x20, 24*8(sp)
    sd x21, 25*8(sp)
    sd x22, 26*8(sp)
    sd x23, 27*8(sp)
    sd x24, 28*8(sp)
    sd x25, 29*8(sp)
    sd x26, 30*8(sp)
    sd x27, 31*8(sp)
    sd x28, 32*8(sp)
    sd x29, 33*8(sp)
    sd x30, 34*8(sp)

    csrr t0, scause
    srli t0, t0, 63
    slli t0, t0, 16
    li   t1, 1                   # 内核态 source = 1
    or   t0, t0, t1
    sd   t0, 0*8(sp)

    mv a0, sp
    call trap_handler

    # 恢复上下文
    ld t0, 1*8(sp)
    ld t1, 2*8(sp)
    csrw sepc, t0
    csrw sstatus, t1
    ld tp, 4*8(sp)

    ld x11, 5*8(sp)
    ld x12, 6*8(sp)
    ld x13, 7*8(sp)
    ld x14, 8*8(sp)
    ld x15, 9*8(sp)
    ld x16, 10*8(sp)
    ld x17, 11*8(sp)
    ld x1,  13*8(sp)
    ld x3,  15*8(sp)
    ld x4,  16*8(sp)
    ld x5,  17*8(sp)
    ld x6,  18*8(sp)
    ld x7,  19*8(sp)
    ld x8,  20*8(sp)
    ld x9,  21*8(sp)
    ld x18, 22*8(sp)
    ld x19, 23*8(sp)
    ld x20, 24*8(sp)
    ld x21, 25*8(sp)
    ld x22, 26*8(sp)
    ld x23, 27*8(sp)
    ld x24, 28*8(sp)
    ld x25, 29*8(sp)
    ld x26, 30*8(sp)
    ld x27, 31*8(sp)
    ld x28, 32*8(sp)
    ld x29, 33*8(sp)
    ld x30, 34*8(sp)
    ld x10, 35*8(sp)

    addi sp, sp, 36*8
    sret

# ── 进入用户态（run_user(ctx: &mut UserContext)）─────────────────────────
# a0 = &mut UserContext（布局与 TrapFrame 相同）
run_user:
    csrw sscratch, sp           # 保存内核栈指针到 sscratch

    ld t0, 8(a0)                # elr
    ld t1, 16(a0)               # spsr
    ld t2, 24(a0)               # sp
    ld tp, 32(a0)               # tpidr
    csrw sepc, t0
    csrw sstatus, t1

    # 载入通用寄存器
    ld x11, 40(a0)              # a1  ← x1
    ld x12, 48(a0)              # a2  ← x2
    ld x13, 56(a0)              # a3  ← x3
    ld x14, 64(a0)              # a4  ← x4
    ld x15, 72(a0)              # a5  ← x5
    ld x16, 80(a0)              # a6  ← x6
    ld x17, 88(a0)              # a7  ← x7
    ld x1,  104(a0)             # ra  ← x9
    ld x3,  120(a0)             # gp  ← x11
    ld x4,  128(a0)             # tp  ← x12
    ld x5,  136(a0)             # t0  ← x13
    ld x6,  144(a0)             # t1  ← x14
    ld x7,  152(a0)             # t2  ← x15
    ld x8,  160(a0)             # s0  ← x16
    ld x9,  168(a0)             # s1  ← x17
    ld x18, 176(a0)             # s2  ← x18
    ld x19, 184(a0)             # s3  ← x19
    ld x20, 192(a0)             # s4  ← x20
    ld x21, 200(a0)             # s5  ← x21
    ld x22, 208(a0)             # s6  ← x22
    ld x23, 216(a0)             # s7  ← x23
    ld x24, 224(a0)             # s8  ← x24
    ld x25, 232(a0)             # s9  ← x25
    ld x26, 240(a0)             # s10 ← x26
    ld x27, 248(a0)             # s11 ← x27
    ld x28, 256(a0)             # t3  ← x28
    ld x29, 264(a0)             # t4  ← x29
    ld x30, 272(a0)             # t5  ← x30
    ld x10, 280(a0)             # a0  ← x0

    csrw sscratch, t2           # sscratch = 用户 sp

    la t0, __trap_from_user
    csrw stvec, t0
    sret
