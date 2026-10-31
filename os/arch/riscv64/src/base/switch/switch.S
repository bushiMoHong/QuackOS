# RISC-V 上下文切换。
#
# TaskContext 布局（128 字节，与 aarch64 一致）：
#
#   偏移     内容
#   ------   ------
#   0x00     ra          – 返回地址
#   0x08     s0          – callee-saved
#   0x10     s1
#   ...      ...
#   0x60     s11         – callee-saved
#   0x68     (reserved)
#   0x70     satp        – 内核页表根
#   0x78     (reserved)
#
# 参数：
#   a0 = current_tcb —— 指向当前线程 TCB 的指针（offset 0 保存内核栈指针）
#   a1 = next_task_kernel_stack —— 指向下一个任务已保存的上下文区
#
# 返回时 a0 = next_sp（thread_trampoline 用它定位 TrapFrame）。

    .section .text
    .globl __switch
__switch:
    # ── 1. 保存当前上下文 ─────────────────────────────────────────────
    addi sp, sp, -128

    sd ra, 0(sp)
    sd s0, 8(sp)
    sd s1, 16(sp)
    sd s2, 24(sp)
    sd s3, 32(sp)
    sd s4, 40(sp)
    sd s5, 48(sp)
    sd s6, 56(sp)
    sd s7, 64(sp)
    sd s8, 72(sp)
    sd s9, 80(sp)
    sd s10, 88(sp)
    sd s11, 96(sp)

    # 保存 satp
    csrr t0, satp
    sd t0, 112(sp)

    # 把当前 sp 写入 current_tcb 的 offset 0
    sd sp, 0(a0)

    # ── 2. 恢复下一个上下文 ───────────────────────────────────────────
    mv t2, a1            # t2 = next_sp

    ld ra, 0(t2)
    ld s0, 8(t2)
    ld s1, 16(t2)
    ld s2, 24(t2)
    ld s3, 32(t2)
    ld s4, 40(t2)
    ld s5, 48(t2)
    ld s6, 56(t2)
    ld s7, 64(t2)
    ld s8, 72(t2)
    ld s9, 80(t2)
    ld s10, 88(t2)
    ld s11, 96(t2)

    # 恢复 satp 并刷新 TLB
    ld t0, 112(t2)
    csrw satp, t0
    sfence.vma

    # ── 3. 设置新线程环境并返回 ───────────────────────────────────────
    mv a0, t2            # a0 = next_sp（返回给 thread_trampoline）
    addi sp, t2, 128     # sp = 下一个线程的栈顶
    ret
