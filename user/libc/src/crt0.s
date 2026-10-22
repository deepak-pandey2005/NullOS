/*
 * crt0.s - C Runtime Startup for NullOS x86_64 User Mode Programs
 */

.section .text
.global _start
.type _start, @function

_start:
    # 1. Clear frame pointer to mark the outermost stack frame for debuggers
    xorq %rbp, %rbp

    # 2. Extract argc and argv from the stack
    movq (%rsp), %rdi         # %rdi = argc
    leaq 8(%rsp), %rsi        # %rsi = argv

    # 3. Align the stack to 16 bytes before calling main per System V AMD64 ABI
    andq $-16, %rsp
    subq $8, %rsp             # 8-byte padding so after 'call' %rsp is 16-byte aligned

    # 4. Invoke user entry point: int main(int argc, char **argv)
    call main

    # 5. Call exit(status) with main's return value (in %rax)
    movq %rax, %rdi
    call exit

    # 6. Fallback if exit() ever returns: direct SYS_EXIT syscall
.Lhang:
    movq $60, %rax            # SYS_EXIT = 60
    movq $1, %rdi             # exit status = 1
    syscall
    hlt
    jmp .Lhang

.size _start, . - _start
