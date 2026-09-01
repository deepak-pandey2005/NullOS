OUTPUT_FORMAT(elf64-x86-64)
OUTPUT_ARCH(i386:x86-64)

ENTRY(_start)

SECTIONS
{
 . = 1M;

.text ALIGN(4K) :
 {
    KEEP(*(.text.entry))
    *(.text .text.*)
}

.rodata ALIGN(4K) :
{
 *(.rodata .roadata.*)
 }

.data ALIGN(4K) :
{
    *(.data .data.*)
}

.bss ALIGN(4K) :
{
    *(COMMON)
    *(.bss .bss.*)
}

. = ALIGN(4K);

__kernel_start = ADDR(.text);
__kernel_end = .;

}
