.section .text._start
.globl _start

_start:
    # a0 = hart id (from OpenSBI)
    # 每个 hart 分配 64KB 独立栈空间（4096*16 = 65536 = 2^16）
    la sp, boot_stack_top
    addi t1, a0, 1
    slli t1, t1, 16     # t1 = (hart_id + 1) * 65536
    sub sp, sp, t1

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
