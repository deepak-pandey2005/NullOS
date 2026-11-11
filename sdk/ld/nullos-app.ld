/*
 * NullOS x86_64 User Application Linker Script (Static Executable)
 * Maps Ring 3 text/rodata/init_array/data/bss cleanly starting at 0x00400000.
 */

OUTPUT_FORMAT("elf64-x86-64", "elf64-x86-64", "elf64-x86-64")
OUTPUT_ARCH(i386:x86-64)
ENTRY(_start)

SECTIONS
{
    . = 0x00400000;

    .text : ALIGN(4K) {
        *(.text._start)
        *(.text .text.*)
    }

    .rodata : ALIGN(4K) {
        *(.rodata .rodata.*)
    }

    .init_array : ALIGN(8) {
        PROVIDE_HIDDEN(__init_array_start = .);
        KEEP(*(SORT_BY_INIT_PRIORITY(.init_array.*) SORT_BY_INIT_PRIORITY(.ctors.*)))
        KEEP(*(.init_array .ctors))
        PROVIDE_HIDDEN(__init_array_end = .);
    }

    .fini_array : ALIGN(8) {
        PROVIDE_HIDDEN(__fini_array_start = .);
        KEEP(*(SORT_BY_INIT_PRIORITY(.fini_array.*) SORT_BY_INIT_PRIORITY(.dtors.*)))
        KEEP(*(.fini_array .dtors))
        PROVIDE_HIDDEN(__fini_array_end = .);
    }

    .data : ALIGN(4K) {
        *(.data .data.*)
    }

    .bss : ALIGN(4K) {
        *(COMMON)
        *(.bss .bss.*)
    }

    /DISCARD/ : {
        *(.comment)
        *(.note.*)
        *(.eh_frame)
    }
}

