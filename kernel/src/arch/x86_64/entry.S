/*
 * NullOS Kernel 64-Bit Assembly Entry Point.
 *
 * Execution begins here immediately after the UEFI bootloader calls `ExitBootServices()`
 * and executes a jump to the kernel ELF entry point.
 *
 * Calling Convention Contract:
 * - `%rdi`: Linear physical address of the `BootInfo` contract structure (SysV AMD64 ABI).
 * - `%rcx`: Duplicate `BootInfo` pointer for Win64 ABI compatibility.
 * - CPU Mode: 64-bit Long Mode, paging enabled (identity-mapped by firmware), CPL = 0.
 */

.section .text.entry
.global _start
.type _start, @function

.extern kernel_main

_start:
    /* Step 1: Ensure hardware interrupts remain disabled during early initialization */
    cli

    /* Step 2: Establish the 2 MiB early bootstrap kernel execution stack */
    lea stack_top(%rip), %rsp

    /* Step 3: Jump into the Rust high-level kernel entry point with BootInfo in %rdi */
    call kernel_main

    /* Step 4: Safety fallback: If kernel_main returns, halt CPU execution indefinitely */
1:
    hlt
    jmp 1b

/*
 * Early Kernel Bootstrap Stack:
 * Allocates 2,097,152 bytes (2 MiB) of static .bss memory, aligned to 16 bytes
 * per System V AMD64 stack alignment requirements.
 */
.section .bss
.align 16

stack_bottom:
    .skip 2097152

stack_top:
