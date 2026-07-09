ENTRY(_start)

SECTIONS
{
    . = 0x80200000; /* QEMU virt 架构，OpenSBI 加载内核的起始地址 */

    .text : {
        *(.text.entry)      /* entry.S 页表初始化代码，必须排在最前面 */
        *(.text._start)     /* boot.S 简化启动代码（备用入口） */
        *(.text.trampoline) /* trampoline.S 上下文切换跳板 */
        *(.text .text.*)
    }

    .rodata : { *(.rodata .rodata.*) }
    .data : { *(.data .data.*) }
    .bss : { *(.bss .bss.*) }

    /DISCARD/ : { *(.comment) *(.gnu*) *(.note*) *(.eh_frame*) }
}
