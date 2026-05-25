/* Tell the linker the entry point function name */
ENTRY(kernel_main)

OUTPUT_FORMAT("elf64-x86-64")
OUTPUT_ARCH(i386:x86-64)

PHDRS
{
    text PT_LOAD FLAGS(5); /* Read + Execute */
    rodata PT_LOAD FLAGS(4); /* Read Only */
    data PT_LOAD FLAGS(6); /* Read + Write */
}

SECTIONS
{
    /* Limine expects x86_64 kernels to be linked in the higher half */
    . = 0xffffffff80000000;

    .text : {
        *(.text .text.*)
    } :text

    .rodata : {
        *(.rodata .rodata.*)
    } :rodata

    .data : {
        *(.data .data.*)
    } :data

    .bss : {
        *(.bss .bss.*)
        *(COMMON)
    } :data
}
