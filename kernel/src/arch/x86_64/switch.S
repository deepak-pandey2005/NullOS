/*
 * Thread Context Switch Assembly Primitive for x86_64.
 *
 * Prototype (SysV AMD64 ABI):
 *   extern "C" fn switch_threads(prev_rsp_ptr: *mut u64, next_rsp: u64);
 *
 * Parameters:
 *   %rdi: Pointer to location where the current thread's %rsp will be saved
 *   %rsi: The new thread's %rsp to switch to
 */

.global switch_threads
.type switch_threads, @function

switch_threads:
    # 1. Save callee-preserved registers and CPU flags on the old stack
    push %r15
    push %r14
    push %r13
    push %r12
    push %rbx
    push %rbp
    pushfq

    # 2. Save current %rsp into *prev_rsp_ptr
    mov %rsp, (%rdi)

    # 3. Switch CPU stack pointer to next thread's %rsp
    mov %rsi, %rsp

    # 4. Restore CPU flags and registers from the new thread's stack
    popfq
    pop %rbp
    pop %rbx
    pop %r12
    pop %r13
    pop %r14
    pop %r15

    # 5. Return into the new thread (pops %rip from stack)
    ret
