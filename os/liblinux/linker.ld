/* User-space linker script for liblinux. */
ENTRY(_start)

PHDRS
{
  text   PT_LOAD FLAGS((1 << 0) | (1 << 2)); /* R-X: PF_X | PF_R */
  rodata PT_LOAD FLAGS((1 << 2));            /* R--: PF_R */
  data   PT_LOAD FLAGS((1 << 2) | (1 << 1)); /* RW-: PF_R | PF_W */
}

SECTIONS
{
  . = 0x200000;

  .text : {
    *(.text._start)
    *(.text .text.*)
  } :text

  .rodata : ALIGN(4096) {
    *(.rodata .rodata.*)
  } :rodata

  .data : ALIGN(4096) {
    *(.data .data.*)
  } :data

  .bss : ALIGN(4096) {
    *(.bss .bss.*)
    *(COMMON)
  } :data

  /DISCARD/ : { *(.comment) *(.note .note.*) }
}
