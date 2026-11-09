/*
 * NullOS x86_64 User Mode Executable Linker Script
 *
 * ============================================================================
 * ARCHITECTURAL VIRTUAL ADDRESS SPACE LAYOUT:
 * ============================================================================
 * This script specifies the memory layout and ELF segment boundaries for
 * statically linked Ring 3 executables in NullOS:
 * - Base address: 0x00400000 (4 MiB boundary), standard Linux/BSD user code base.
 *   This leaves the first 4 MiB unmapped to trap NULL pointer dereferences.
 * - Section alignment: 4 KiB (4096 bytes) per section, matching standard x86_64
 *   page frame granularity to permit page table protections (R-X for code,
 *   R-- for read-only data, RW- for mutable data/BSS).
 * - Entry point: `_start` in crt0.s.
 */

OUTPUT_FORMAT("elf64-x86-64", "elf64-x86-64", "elf64-x86-64")
OUTPUT_ARCH(i386:x86-64)
ENTRY(_start)

SECTIONS
{
    /* Load user mode binaries starting at 4 MiB virtual address */
    . = 0x00400000;

    /* Executable code (.text): mapped with PROT_READ | PROT_EXEC */
    .text : ALIGN(4K) {
        *(.text._start)
        *(.text .text.*)
    }

    /* Read-only constant data (.rodata): mapped with PROT_READ */
    .rodata : ALIGN(4K) {
        *(.rodata .rodata.*)
    }

    /* Initialized mutable data (.data): mapped with PROT_READ | PROT_WRITE */
    .data : ALIGN(4K) {
        *(.data .data.*)
    }

    /* Zero-initialized uninitialized data (.bss): cleared to 0 at load time */
    .bss : ALIGN(4K) {
        *(COMMON)
        *(.bss .bss.*)
    }

    /* Discard compiler comments, unwind frames, and non-essential ELF notes */
    /DISCARD/ : {
        *(.comment)
        *(.note.*)
        *(.eh_frame)
    }
}
