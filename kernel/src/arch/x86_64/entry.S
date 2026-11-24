/*
 * NullOS Kernel Multi-Boot Entry Point.
 *
 * Supports:
 * 1. Native NullOS UEFI Bootloader (passes pointer to BootInfo in %rdi / %rcx)
 * 2. GRUB / Multiboot2 (passes magic 0x36d76289 in %eax, info pointer in %ebx)
 * 3. Limine Boot Protocol (populates responses in .limine_requests section)
 */

/* =========================================================================
 * Multiboot2 Header Definition
 * Must be within the first 32 KiB of the ELF file, aligned to 8 bytes.
 * ========================================================================= */
.section .multiboot2, "a"
.align 8
multiboot2_header_start:
    .long 0xe85250d6                        /* Magic number (Multiboot 2) */
    .long 0                                 /* Architecture: 0 (32-bit/64-bit protected mode) */
    .long multiboot2_header_end - multiboot2_header_start /* Header length */
    /* Checksum: -(magic + architecture + header_length) */
    .long -(0xe85250d6 + 0 + (multiboot2_header_end - multiboot2_header_start))

    /* Framebuffer tag (optional request for GOP linear framebuffer) */
    .align 8
    .short 5                                /* Type: Framebuffer request */
    .short 0                                /* Flags */
    .long 20                                /* Size: 20 bytes */
    .long 1280                              /* Preferred width */
    .long 800                               /* Preferred height */
    .long 32                                /* Preferred depth (32-bit truecolor) */

    /* End tag */
    .align 8
    .short 0                                /* Type: End tag */
    .short 0                                /* Flags */
    .long 8                                 /* Size: 8 bytes */
multiboot2_header_end:


/* =========================================================================
 * Limine Protocol Request Structures
 * Embedded in .limine_requests
 * ========================================================================= */
.section .limine_requests_start, "a"
.align 8
limine_reqs_start_marker:
    .quad 0xf6b8f4b39de7d1ae
    .quad 0xfab91a6940fcb9cf
    .quad 0x785c6ed015d3e316
    .quad 0x181e920a7852b9d9

.section .limine_requests, "a"
.align 8

/* Base Revision Request (Revision 2) */
.global limine_base_rev
limine_base_rev:
    .quad 0xf9562b2d5c95a671
    .quad 0x6a7b1e0281206f43
    .quad 2
    .quad 0

/* Limine Framebuffer Request */
.global limine_fb_request
limine_fb_request:
    .quad 0x9d5827dcd881dd7b
    .quad 0xa31f439950824a01
    .quad 0                                 /* Response pointer filled by Limine */
    .quad 0

/* Limine Memory Map Request */
.global limine_mmap_request
limine_mmap_request:
    .quad 0x67cf3d9d378a1497
    .quad 0x9334735d4a93b853
    .quad 0                                 /* Response pointer filled by Limine */
    .quad 0

/* Limine HHDM Request */
.global limine_hhdm_request
limine_hhdm_request:
    .quad 0x48dcf1038cf54537
    .quad 0xad12872d32f6c049
    .quad 0                                 /* Response pointer filled by Limine */
    .quad 0

/* Limine RSDP Request */
.global limine_rsdp_request
limine_rsdp_request:
    .quad 0xc5e77d6b397e7b43
    .quad 0x2763783636f9cb88
    .quad 0                                 /* Response pointer filled by Limine */
    .quad 0

.section .limine_requests_end, "a"
.align 8
limine_reqs_end_marker:
    .quad 0xadc0e0531bb10d03
    .quad 0x957071076cad3450
    .quad 0x582c3756d5825362
    .quad 0x4e650b25e791b795


/* =========================================================================
 * Kernel 64-Bit Entry Point
 * ========================================================================= */
.section .text.entry
.global _start
.type _start, @function

.extern boot_dispatcher

_start:
    /* Step 1: Ensure hardware interrupts remain disabled during early initialization */
    cli

    /* Step 2: Establish early bootstrap kernel execution stack */
    lea stack_top(%rip), %rsp

    /* Step 3:
     * Preserve incoming registers:
     * - %rdi: NullOS native BootInfo pointer (or first arg in SysV)
     * - %rsi: Secondary argument
     * - %rax: Multiboot2 magic 0x36d76289 if booted via Multiboot2
     * - %rbx: Multiboot2 info struct physical pointer
     *
     * Calling Rust boot_dispatcher(rdi, rsi, rax, rbx) -> !
     */
    mov %rax, %rdx                          /* arg3 = rax (Multiboot2 magic) */
    mov %rbx, %rcx                          /* arg4 = rbx (Multiboot2 info ptr) */
    call boot_dispatcher

    /* Step 4: Safety fallback */
1:
    hlt
    jmp 1b

/*
 * Early Kernel Bootstrap Stack (2 MiB)
 */
.section .bss
.align 16

stack_bottom:
    .skip 2097152

stack_top:
