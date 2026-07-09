.section .text._start
.globl _start

_start:
    # a0 = hart id (from OpenSBI)
    # 每个 hart 分配 64KB 独立栈空间
    la sp, boot_stack_top
    li t0, 4096 * 16
    addi t1, a0, 1
    mul t0, t0, t1
    sub sp, sp, t0

    # 跳转到 Rust 主函数
    call rust_main

    # 如果 rust_main 意外返回，死循环
_loop:
    j _loop


.section .bss.stack
.align 12
.globl boot_stack_lower_bound
boot_stack_lower_bound:
    .space 4096 * 16 * 4  # 最多 4 核
.globl boot_stack_top
boot_stack_top:
