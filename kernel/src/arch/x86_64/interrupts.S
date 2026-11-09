/*
 * Low-Level Assembly Interrupt Service Routine (ISR) Stubs.
 *
 * Hardware Stack Alignment Contract:
 * When an interrupt or exception triggers:
 * 1. The CPU automatically pushes:
 *      [SS] -> [RSP] -> [RFLAGS] -> [CS] -> [RIP] (and optional Error Code)
 * 2. If transitioning from Ring 3 to Ring 0, %rsp is loaded from TSS.rsp0 or IST.
 * 3. Some exceptions push an Error Code (vectors 8, 10, 11, 12, 13, 14, 17, 21).
 *    For exceptions that do NOT push an error code, `ISR_NOERR` pushes a dummy 0
 *    so all frames conform identically to Rust's `ExceptionContext` structure.
 */

.section .text
.extern exception_handler

/*
 * Macro for CPU exceptions that do NOT push a hardware error code.
 * Pushes dummy 0 on the stack before pushing the interrupt vector number.
 */
.macro ISR_NOERR index
.global isr\index
.type isr\index, @function
isr\index:
    push $0               // Push dummy error code to normalize stack frame
    push $\index          // Push interrupt vector number
    jmp isr_common_stub
.endm

/*
 * Macro for CPU exceptions that DO push a hardware error code.
 * Pushes only the interrupt vector number (error code is already on the stack).
 */
.macro ISR_ERR index
.global isr\index
.type isr\index, @function
isr\index:
    push $\index          // Push interrupt vector number
    jmp isr_common_stub
.endm

/* Standard x86_64 Architectural Exception Vectors (0..31) */
ISR_NOERR 0   // #DE Divide-by-zero Error
ISR_NOERR 1   // #DB Debug Exception
ISR_NOERR 2   // NMI Non-Maskable Interrupt
ISR_NOERR 3   // #BP Breakpoint Exception (INT3)
ISR_NOERR 4   // #OF Overflow Exception (INTO)
ISR_NOERR 5   // #BR BOUND Range Exceeded
ISR_NOERR 6   // #UD Invalid Opcode (Undefined Opcode)
ISR_NOERR 7   // #NM Device Not Available (No Math Coprocessor)
ISR_ERR   8   // #DF Double Fault (Pushes error code 0, routed via TSS IST1)
ISR_ERR   10  // #TS Invalid Task State Segment (TSS)
ISR_ERR   11  // #NP Segment Not Present
ISR_ERR   12  // #SS Stack Segment Fault
ISR_ERR   13  // #GP General Protection Fault
ISR_ERR   14  // #PF Page Fault (Fault address loaded in CR2)
ISR_NOERR 16  // #MF x87 FPU Floating-Point Error (Math Fault)
ISR_ERR   17  // #AC Alignment Check
ISR_NOERR 18  // #MC Machine Check
ISR_NOERR 19  // #XM SIMD Floating-Point Exception
ISR_NOERR 20  // #VE Virtualization Exception
ISR_ERR   21  // #CP Control Protection Exception

/*
 * Vector 32: Hardware Local APIC Periodic Timer Interrupt.
 *
 * Saves full register context, clears direction flag per SysV ABI,
 * invokes high-level Rust `timer_handler()`, and performs `iretq`.
 */
.extern timer_handler
.global isr32
.type isr32, @function
isr32:
    push $0               // Dummy error code
    push $32              // Interrupt vector 32
    push %rax
    push %rbx
    push %rcx
    push %rdx
    push %rsi
    push %rdi
    push %rbp
    push %r8
    push %r9
    push %r10
    push %r11
    push %r12
    push %r13
    push %r14
    push %r15
    cld                   // Clear direction flag (SysV ABI contract: DF=0 for string ops)
    call timer_handler
    pop %r15
    pop %r14
    pop %r13
    pop %r12
    pop %r11
    pop %r10
    pop %r9
    pop %r8
    pop %rbp
    pop %rdi
    pop %rsi
    pop %rdx
    pop %rcx
    pop %rbx
    pop %rax
    add $16, %rsp         // Pop vector and dummy error code
    iretq

/*
 * Vector 255: Spurious Interrupt Handler.
 * Does NOT send an EOI to the Local APIC (spurious interrupts must not be acknowledged).
 */
.global isr255
.type isr255, @function
isr255:
    iretq

/*
 * Common Exception Dispatch Stub.
 *
 * Saves all 15 general-purpose registers, loads %rsp into %rdi as the pointer
 * to `ExceptionContext`, calls Rust `exception_handler()`, restores registers,
 * and executes `iretq`.
 */
isr_common_stub:
    push %rax
    push %rbx
    push %rcx
    push %rdx
    push %rsi
    push %rdi
    push %rbp
    push %r8
    push %r9
    push %r10
    push %r11
    push %r12
    push %r13
    push %r14
    push %r15

    /* Pass pointer to ExceptionContext as first argument in %rdi (SysV ABI) */
    mov %rsp, %rdi
    cld                   // Ensure forward string operation direction
    call exception_handler

    /* Restore general-purpose registers in reverse order if handler resumes */
    pop %r15
    pop %r14
    pop %r13
    pop %r12
    pop %r11
    pop %r10
    pop %r9
    pop %r8
    pop %rbp
    pop %rdi
    pop %rsi
    pop %rdx
    pop %rcx
    pop %rbx
    pop %rax

    /* Clean up vector and error code before executing hardware return */
    add $16, %rsp
    iretq
