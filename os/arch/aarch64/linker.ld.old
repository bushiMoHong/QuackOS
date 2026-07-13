ENTRY(_start)

SECTIONS
{
    . = 0x40000000; /* QEMU virt 架构 RAM 起始地址 */

    .text : {
        *(.text._start) /* 确保 boot.S 的代码排在整个程序的最开头 */
        *(.text .text.*)
    }

    .rodata : { *(.rodata .rodata.*) }
    .data : { *(.data .data.*) }
    .bss : { *(.bss .bss.*) }

    /DISCARD/ : { *(.comment) *(.gnu*) *(.note*) *(.eh_frame*) }
}