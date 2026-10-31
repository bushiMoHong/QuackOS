# RISC-V 引导入口（由 OpenSBI 调用，a0 = hart id）。
# 设置栈后跳转到 rust_main。

    .section .text._start
    .globl _start
_start:
    # 每个 hart 分配 64KB 独立栈（4096*16 = 65536 = 2^16）
    la sp, boot_stack_top
    addi t1, a0, 1
    slli t1, t1, 16
    sub sp, sp, t1

    # 跳转到 Rust 主函数
    call rust_main

_loop:
    j _loop

    .section .bss.stack
    .align 12
    .globl boot_stack
boot_stack:
    .space 262144          # 预留 256 KB 启动栈（4 核 × 64 KB）
    .globl boot_stack_top
boot_stack_top:
