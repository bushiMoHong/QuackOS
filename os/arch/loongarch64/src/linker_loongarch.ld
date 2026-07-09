ENTRY(_start)

SECTIONS
{
    . = 0x00200000; /* QEMU virt LoongArch -kernel 加载地址 */

    .text : {
        *(.text.entry)      /* entry.S 启动代码，必须排在最前面 */
        *(.text.__rfill)    /* tlb_refill.S TLB 重填处理 */
        *(.text.trampoline) /* trampoline.S 信号返回跳板 */
        *(.text .text.*)
    }

    .rodata : { *(.rodata .rodata.*) }
    .data : { *(.data .data.*) }
    .bss : { *(.bss .bss.*) }

    /DISCARD/ : { *(.comment) *(.gnu*) *(.note*) *(.eh_frame*) }
}
