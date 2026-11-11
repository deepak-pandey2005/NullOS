/*
 * sdk/crt/crt0.s - Official NullOS SDK C Runtime Startup (x86_64 Ring 3)
 * Invokes static constructors in .init_array (when linked via nullos-app.ld),
 * aligns %rsp to 16 bytes per System V AMD64 ABI, passes argc/argv to main(),
 * and exits cleanly via libc `exit()` / SYS_EXIT fallback.
 */

.section .text
.global _start
.type _start, @function
.weak __init_array_start
.weak __init_array_end

_start:
    /* Terminate frame pointer chain for stack unwinders */
    xorq %rbp, %rbp

    /* Save argc (%rdi) and argv (%rsi) per System V AMD64 ABI before aligning stack */
    movq (%rsp), %r12
    leaq 8(%rsp), %r13

    /* Enforce 16-byte stack alignment prior to calls */
    andq $-16, %rsp

    /* Invoke global constructors in .init_array if linked with nullos-app.ld */
    leaq __init_array_start(%rip), %rbx
    leaq __init_array_end(%rip), %r14
.Linit_loop:
    cmpq %r14, %rbx
    jae .Linit_done
    movq (%rbx), %rax
    testq %rax, %rax
    jz .Linit_next
    call *%rax
.Linit_next:
    addq $8, %rbx
    jmp .Linit_loop
.Linit_done:

    /* Restore argc (%rdi) and argv (%rsi) and invoke user main(argc, argv) */
    movq %r12, %rdi
    movq %r13, %rsi
    call main

    /* Exit with main()'s return code */
    movq %rax, %rdi
    call exit

.Lhang:
    movq $60, %rax
    syscall
    jmp .Lhang

