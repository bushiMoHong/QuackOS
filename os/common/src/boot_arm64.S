.section .text._start
.global _start

_start:
    // 1. 启用 FPU/高级 SIMD（必须位于 Rust 代码之前，否则编译器
    //    生成的 d8–d15 保存指令会在 CPACR_EL1 为 0 时触发陷阱）
    mrs x0, cpacr_el1
    orr x0, x0, #(3 << 20)       // FPEN = 0b11：EL0/EL1 均可访问
    msr cpacr_el1, x0
    isb                          // 确保 CPACR 更新对后续指令可见

    // 2. 设置栈指针（AArch64 的栈必须 16 字节对齐）
    // 将栈顶设置为 0x40080000
    ldr x0, =0x40080000
    mov sp, x0

    // 3. 跳转到 Rust 的主函数
    bl rust_main

    // 4. 如果 rust_main 意外返回，在此处死循环
_loop:
    b _loop