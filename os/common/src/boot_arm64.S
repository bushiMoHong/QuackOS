	.section .text._start
	.global _start

	_start:
	    // 1. 启用 FPU/高级 SIMD
	    // 必须位于 Rust 代码之前，否则编译器生成的 d8–d15 保存指令会在 CPACR_EL1 为 0 时触发陷阱
	    mrs x0, cpacr_el1
	    orr x0, x0, #(3 << 20)       // FPEN = 0b11：EL0/EL1 均可访问
	    msr cpacr_el1, x0
	    isb                          // 确保 CPACR 更新对后续指令可见

	    // 2. 清零 .bss 段 (C/Rust 规范要求未初始化静态变量必须为 0)
	    // 避免 Trap Handler 回调指针出现垃圾数据
	    adrp x1, __bss_start
	    add  x1, x1, :lo12:__bss_start
	    adrp x2, __bss_end
	    add  x2, x2, :lo12:__bss_end
	1:
	    cmp  x1, x2
	    b.hs 2f
	    str  xzr, [x1], #8           // 将 0 (xzr) 写入 x1 对应地址，随后 x1 += 8
	    b    1b
	2:

	    // 3. 设置安全的栈指针
	    // 指向由链接器自动分配的 boot_stack_top，告别硬编码导致的内存碰撞
	    adrp x0, boot_stack_top
	    add  x0, x0, :lo12:boot_stack_top
	    mov  sp, x0

	    // 4. 跳转到 Rust 的主函数
	    bl   rust_main

	    // 5. 如果 rust_main 意外返回，在此处低功耗死循环
	_loop:
	    wfe                          // Wait For Event (降低 QEMU/CPU 占用)
	    b    _loop


	// ---------------------------------------------------------------------------
	// 显式预留内核启动栈空间
	// ---------------------------------------------------------------------------
	.section .bss.stack
	.align 12                        // 按 4KB 对齐
	.global boot_stack
	boot_stack:
	    .space 262144                // 预留 256 KB 作为启动栈！(65536 * 4)
	.global boot_stack_top
	boot_stack_top:

	.align 12
	.global exc_stack
	exc_stack:
	    .space 8192                  // 8 KB 专用异常栈
	.global exc_stack_top
	exc_stack_top:
