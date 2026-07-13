ENTRY(_start)

/* QEMU virt 机器的 RAM 从 0x40000000 开始 */
BASE_ADDRESS = 0x40000000;

SECTIONS
{
    . = BASE_ADDRESS;

    /* 1. 代码段 */
    .text : ALIGN(4096) {
        __text_start = .;
        *(.text._start) /* 确保 boot.S 的 _start 在最前面 */
        *(.text .text.*)
        __text_end = .;
    }

    /* 2. 只读数据段 */
    .rodata : ALIGN(4096) {
        __rodata_start = .;
        *(.rodata .rodata.*)
        __rodata_end = .;
    }

    /* 3. 可读写数据段 */
    .data : ALIGN(4096) {
        __data_start = .;
        *(.data .data.*)
        __data_end = .;
    }

    /* 4. 未初始化数据段 (BSS) */
    .bss : ALIGN(4096) {
        __bss_start = .;
        
        *(.bss .bss.*)
        
        /* 将汇编里定义的栈统一放置在 bss 段末尾 */
        . = ALIGN(16);
        *(.bss.stack)
        
        __bss_end = .;
    }

    /* 丢弃不需要的调试信息或无关段，减小二进制文件体积 */
    /DISCARD/ : {
        *(.comment)
        *(.gnu*)
        *(.note*)
        *(.eh_frame*)
    }
}
