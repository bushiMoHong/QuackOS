    .section .text.entry
    .globl _start
_start:
    # 设置 DMW1: 恒等映射当前 VSEG (0 → 0), 可缓存
    pcaddi  $t0, 0
    srli.d  $t0, $t0, 60
    slli.d  $t0, $t0, 60
    ori     $t0, $t0, 0x17       # V=1, MAT=SUC, PLV=all (0-3)
    csrwr   $t0, 0x181           # DMW1

    # 设置 DMW0: VSEG=8 → PSEG=0, 用于 MMIO (uncached)
    li.d    $t1, 0x8000000000000001
    csrwr   $t1, 0x180           # DMW0

    # 管道屏障
    pcaddi  $t0, 0
    slli.d  $t0, $t0, 16
    srli.d  $t0, $t0, 16
    jirl    $zero, $t0, 16

    # 设置栈指针
    la.abs  $sp, boot_stack_top

    # 跳转 Rust 主函数
    bl      rust_main

    # 如果 rust_main 意外返回，死循环
_loop:
    b       _loop


    .section .bss.stack
    .align 12
    .globl boot_stack_lower_bound
boot_stack_lower_bound:
    .space 4096 * 16 * 4   # 最多 4 核
    .globl boot_stack_top
boot_stack_top:
