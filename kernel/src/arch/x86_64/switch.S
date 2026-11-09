/*
 * Thread Context Switch Assembly Primitive for x86_64.
 *
 * Prototype (System V AMD64 ABI):
 *   extern "C" fn switch_threads(prev_rsp_ptr: *mut u64, next_rsp: u64);
 *
 * Parameters:
 *   %rdi: Pointer to `u64` where the outgoing thread's current %rsp is written.
 *   %rsi: The incoming thread's target %rsp to be loaded into the CPU.
 *
 * Mechanics:
 * Under System V AMD64 ABI, registers %rax, %rcx, %rdx, %rsi, %rdi, %r8..%r11 are
 * caller-saved (scratch registers). Functions that yield or are preempted expect
 * only the callee-saved registers (%rbx, %rbp, %r12, %r13, %r14, %r15, and RFLAGS)
 * to be preserved across context switches.
 */

.global switch_threads
.type switch_threads, @function

switch_threads:
    /* Step 1: Save outgoing thread CPU flags and disable interrupts during stack pivot */
    pushfq
    cli

    /* Step 2: Push System V AMD64 callee-preserved registers onto the outgoing stack */
    push %r15
    push %r14
    push %r13
    push %r12
    push %rbx
    push %rbp

    /* Step 3: Save the outgoing thread's top of stack pointer into *prev_rsp_ptr */
    mov %rsp, (%rdi)

    /* Step 4: Atomically pivot the CPU stack pointer to the incoming thread's stack */
    mov %rsi, %rsp

    /* Step 5: Restore callee-preserved registers from the incoming thread's stack */
    pop %rbp
    pop %rbx
    pop %r12
    pop %r13
    pop %r14
    pop %r15

    /* Step 6: Sanitize flags: mask off Trap Flag (0x100) and Nested Task (0x4000), restore RFLAGS */
    andq $~0x4100, (%rsp)
    popfq

    /* Step 7: Pop target thread's saved %rip from the stack and resume execution */
    ret
