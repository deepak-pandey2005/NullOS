.section .text
.extern exception_handler

// Macro for exceptions without CPU error code
.macro ISR_NOERR index
.global isr\index
.type isr\index, @function
isr\index:
    push $0               // Push dummy error code
    push $\index          // Push interrupt vector
    jmp isr_common_stub
.endm

// Macro for exceptions that push a CPU error code
.macro ISR_ERR index
.global isr\index
.type isr\index, @function
isr\index:
    push $\index          // Push interrupt vector
    jmp isr_common_stub
.endm

// Define CPU exception stubs 0..31
ISR_NOERR 0   // #DE Divide-by-zero
ISR_NOERR 1   // #DB Debug
ISR_NOERR 2   // NMI
ISR_NOERR 3   // #BP Breakpoint
ISR_NOERR 4   // #OF Overflow
ISR_NOERR 5   // #BR Bound Range
ISR_NOERR 6   // #UD Invalid Opcode
ISR_NOERR 7   // #NM Device Not Available
ISR_ERR   8   // #DF Double Fault (pushes error code 0)
ISR_ERR   10  // #TS Invalid TSS
ISR_ERR   11  // #NP Segment Not Present
ISR_ERR   12  // #SS Stack-Segment Fault
ISR_ERR   13  // #GP General Protection Fault
ISR_ERR   14  // #PF Page Fault
ISR_NOERR 16  // #MF x87 Floating-Point
ISR_ERR   17  // #AC Alignment Check
ISR_NOERR 18  // #MC Machine Check
ISR_NOERR 19  // #XM SIMD Floating-Point
ISR_NOERR 20  // #VE Virtualization
ISR_ERR   21  // #CP Control Protection

// Hardware / APIC interrupts
.extern timer_handler
.global isr32
.type isr32, @function
isr32:
    push $0
    push $32
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
    cld
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
    add $16, %rsp
    iretq

.global isr255
.type isr255, @function
isr255:
    iretq


// Common stub that saves all general-purpose registers
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

    // Pass pointer to ExceptionContext as first argument in %rdi (SysV ABI)
    mov %rsp, %rdi
    cld
    call exception_handler

    // Restore registers (if handler ever returns)
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

    // Clean up vector and error code
    add $16, %rsp
    iretq
