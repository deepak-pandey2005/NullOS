/*
 * Application Processor (AP) Real-Mode Trampoline
 *
 * Bootstraps an Application Processor from 16-bit Real Mode into 64-bit Long Mode:
 *   16-bit Real Mode -> 32-bit Protected Mode -> 64-bit Long Mode
 *
 * Copied by the BSP to physical address 0x8000 (SIPI vector 0x08).
 * All addresses are computed relative to 0x8000 automatically by the assembler.
 */

.section .rodata
.global ap_trampoline_start
.global ap_trampoline_end

.align 16
ap_trampoline_start:

/* --- Entry: 16-Bit Real Mode Jump to start16 --- */
    .byte 0xEB, (start16 - ap_trampoline_start - 2)

.balign 8
/* --- Mailbox Area (Offsets synchronized via labels) --- */
.global ap_mailbox_cr3
.global ap_mailbox_stack
.global ap_mailbox_entry
.global ap_mailbox_cpu_id
.global ap_mailbox_status

ap_mailbox_cr3:     .quad 0            /* Physical PML4 address */
ap_mailbox_stack:   .quad 0            /* Top of AP kernel stack */
ap_mailbox_entry:   .quad 0            /* 64-bit Rust AP entry function */
ap_mailbox_cpu_id:  .quad 0            /* CPU ID (usize) */
ap_mailbox_status:  .long 0            /* Handshake status (0 = waiting, 1 = online) */
                    .long 0

.balign 8
/* --- GDT Table (32 bytes) --- */
gdt32:
    .quad 0x0000000000000000           /* 0x00: Null descriptor */
    .quad 0x00CF9A000000FFFF           /* 0x08: 32-bit Code (Base 0, Limit 4GB, Exec/Read) */
    .quad 0x00CF92000000FFFF           /* 0x10: 32-bit Data (Base 0, Limit 4GB, Read/Write) */
    .quad 0x00209A0000000000           /* 0x18: 64-bit Code (Long Mode L=1, D=0) */
gdt32_end:

/* --- GDTR Descriptor (6 bytes) --- */
gdt32_desc:
    .word (gdt32_end - gdt32 - 1)      /* Limit: 31 (0x1F) */
    .long 0x8000 + (gdt32 - ap_trampoline_start)

.balign 16
/* --- 16-Bit Real Mode Entry --- */
start16:
    .byte 0xFA                         /* cli */
    .byte 0xFC                         /* cld */

    .byte 0x31, 0xC0                   /* xor %ax, %ax */
    .byte 0x8E, 0xD8                   /* mov %ax, %ds */
    .byte 0x8E, 0xC0                   /* mov %ax, %es */
    .byte 0x8E, 0xD0                   /* mov %ax, %ss */

    /* lgdt (%cs:gdt32_desc) */
    .byte 0x2E, 0x0F, 0x01, 0x16
    .word (gdt32_desc - ap_trampoline_start)

    /* Enable Protected Mode in CR0 (bit 0 PE) */
    .byte 0x0F, 0x20, 0xC0             /* mov %cr0, %eax */
    .byte 0x0C, 0x01                   /* or $1, %al */
    .byte 0x0F, 0x22, 0xC0             /* mov %eax, %cr0 */

    /* Far jump to 32-bit Protected Mode (Selector 0x08, Offset start32) */
    .byte 0x66, 0xEA
    .long 0x8000 + (start32 - ap_trampoline_start)
    .word 0x0008

.balign 16
/* --- 32-Bit Protected Mode Entry --- */
start32:
    /* Reload data selectors to 0x10 */
    .byte 0x66, 0xB8, 0x10, 0x00       /* mov $0x10, %ax */
    .byte 0x8E, 0xD8                   /* mov %eax, %ds */
    .byte 0x8E, 0xC0                   /* mov %eax, %es */
    .byte 0x8E, 0xD0                   /* mov %eax, %ss */

    /* Enable Physical Address Extension (PAE, bit 5), PGE (bit 7), OSFXSR (bit 9), OSXMMEXCPT (bit 10) in CR4 */
    .byte 0x0F, 0x20, 0xE0             /* mov %cr4, %eax */
    .byte 0x0F, 0xBA, 0xE8, 0x05       /* bts $5, %eax */
    .byte 0x0F, 0xBA, 0xE8, 0x07       /* bts $7, %eax */
    .byte 0x0F, 0xBA, 0xE8, 0x09       /* bts $9, %eax */
    .byte 0x0F, 0xBA, 0xE8, 0x0A       /* bts $10, %eax */
    .byte 0x0F, 0x22, 0xE0             /* mov %eax, %cr4 */

    /* Load PML4 address from mailbox into CR3 */
    .byte 0xA1
    .long 0x8000 + (ap_mailbox_cr3 - ap_trampoline_start)
    .byte 0x0F, 0x22, 0xD8             /* mov %eax, %cr3 */

    /* Enable Long Mode (LME, bit 8) and No-Execute (NXE, bit 11) in EFER MSR (0xC0000080) */
    .byte 0xB9, 0x80, 0x00, 0x00, 0xC0 /* mov $0xC0000080, %ecx */
    .byte 0x0F, 0x32                   /* rdmsr */
    .byte 0x0F, 0xBA, 0xE8, 0x08       /* bts $8, %eax (LME) */
    .byte 0x0F, 0xBA, 0xE8, 0x0B       /* bts $11, %eax (NXE) */
    .byte 0x0F, 0x30                   /* wrmsr */

    /* Enable Paging (PG, bit 31) and Write Protect (WP, bit 16) in CR0 to enter 64-bit Long Mode */
    .byte 0x0F, 0x20, 0xC0             /* mov %cr0, %eax */
    .byte 0x0F, 0xBA, 0xE8, 0x10       /* bts $16, %eax (WP) */
    .byte 0x0F, 0xBA, 0xE8, 0x1F       /* bts $31, %eax (PG) */
    .byte 0x0F, 0x22, 0xC0             /* mov %eax, %cr0 */

    /* Far jump to 64-bit Long Mode (Selector 0x18, Offset start64) */
    .byte 0xEA
    .long 0x8000 + (start64 - ap_trampoline_start)
    .word 0x0018

.balign 16
/* --- 64-Bit Long Mode Entry --- */
start64:
    .byte 0x66, 0xB8, 0x10, 0x00       /* mov $0x10, %ax */
    .byte 0x8E, 0xD8                   /* mov %eax, %ds */
    .byte 0x8E, 0xC0                   /* mov %eax, %es */
    .byte 0x8E, 0xD0                   /* mov %eax, %ss */

    /* Load per-CPU kernel stack pointer from mailbox_stack into RSP */
    .byte 0x48, 0x8B, 0x24, 0x25
    .long 0x8000 + (ap_mailbox_stack - ap_trampoline_start)

    /* Pass CPU ID from mailbox_cpu_id in RDI (1st argument in System V ABI) */
    .byte 0x48, 0x8B, 0x3C, 0x25
    .long 0x8000 + (ap_mailbox_cpu_id - ap_trampoline_start)

    /* Signal online status to BSP: write 1 to mailbox_status */
    .byte 0xC7, 0x04, 0x25
    .long 0x8000 + (ap_mailbox_status - ap_trampoline_start)
    .long 0x00000001

    /* Load Rust kernel AP main entry point from mailbox_entry into RAX and jump */
    .byte 0x48, 0x8B, 0x04, 0x25
    .long 0x8000 + (ap_mailbox_entry - ap_trampoline_start)
    .byte 0xFF, 0xE0                   /* jmp *%rax */

.balign 16
ap_trampoline_end:
