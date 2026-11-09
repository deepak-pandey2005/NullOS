/*
 * crt0.s - C Runtime Startup for NullOS x86_64 User Mode (Ring 3) Programs
 *
 * ============================================================================
 * ARCHITECTURAL CONTEXT & EXECUTION ENVIRONMENT:
 * ============================================================================
 * In NullOS, when an ELF executable is launched into user mode (Ring 3) via the
 * execve/clone/spawn system calls, the kernel sets up the initial virtual address
 * space and constructs an initial user stack according to the System V AMD64 ABI.
 *
 * The kernel pushes the initial program environment onto the user stack:
 *     [High Addresses]
 *         ... string tables for environment and argv ...
 *         NULL (end of envp)
 *         envp[n] ... envp[0]
 *         NULL (end of argv)
 *         argv[argc-1] ... argv[0]   <-- pointers to argument strings
 *         argc                       <-- 8-byte integer at (%rsp)
 *     [Low Addresses, current %rsp]
 *
 * The entry point specified in the ELF header points directly to `_start`.
 * Because `_start` is invoked directly by kernel mode via `sysretq` or `iretq`
 * (rather than via a `call` instruction), the stack pointer `%rsp` is 8-byte
 * aligned rather than 16-byte aligned. The System V AMD64 ABI requires that:
 *     "The stack is 16-byte aligned immediately before the call instruction is
 *      executed, so that on function entry (after the return address is pushed),
 *      the stack is 8 mod 16 aligned."
 *
 * Therefore, `_start` performs four crucial initialization duties:
 *   1. Zero the frame pointer (%rbp) to terminate the debugger/unwinder backtrace.
 *   2. Unpack `argc` and `argv` from the initial stack layout into registers
 *      (%rdi = argc, %rsi = argv) according to the System V AMD64 calling convention.
 *   3. Enforce 16-byte stack alignment via `andq $-16, %rsp` before invoking `main`.
 *   4. Transfer control to `main(int argc, char **argv)`.
 *   5. On return from `main`, pass the integer return code to `exit(int status)`.
 *   6. Provide a fallback infinite `sys_exit` loop in case `exit()` ever returns.
 */

.section .text
.global _start
.type _start, @function

_start:
    /*
     * Step 1: Clear the base/frame pointer register (%rbp).
     *
     * Per System V AMD64 ABI conventions, the innermost frame pointer should
     * be cleared to zero so that stack trace unwinders (such as gdb, libunwind,
     * or kernel crash dumpers) know where to stop walking up the call stack.
     */
    xorq %rbp, %rbp

    /*
     * Step 2: Extract command-line argument count (argc) and vector (argv).
     *
     * At entry point:
     *   - (%rsp) contains `argc` (64-bit integer, count of argument strings).
     *   - 8(%rsp) contains `argv[0]` (pointer to the first string, usually program name).
     *
     * We populate the standard System V AMD64 argument registers:
     *   - %rdi: 1st argument -> argc
     *   - %rsi: 2nd argument -> argv (pointer to argv[0])
     */
    movq (%rsp), %rdi         # %rdi = argc (1st function argument to main)
    leaq 8(%rsp), %rsi        # %rsi = &argv[0] (2nd function argument to main)

    /*
     * Step 3: Align stack pointer to a 16-byte boundary.
     *
     * The AMD64 ABI mandates that before a `call` instruction, `%rsp` must be
     * a multiple of 16 bytes. The bitwise operation `andq $-16, %rsp` clears
     * the lowest 4 bits, ensuring 16-byte alignment.
     */
    andq $-16, %rsp

    /*
     * Step 4: Invoke the standard user entry point:
     *   int main(int argc, char **argv);
     *
     * In accordance with the calling convention:
     *   - %rdi holds argc
     *   - %rsi holds argv
     *   - Return value (exit status code) will be returned in %rax.
     */
    call main

    /*
     * Step 5: Gracefully terminate the process via exit(status).
     *
     * The integer return value of `main` is stored in %rax. We move it into
     * %rdi (the 1st argument register for `exit`) and invoke `exit(int status)`.
     * `exit()` flushes standard streams, runs registered exit handlers, and
     * performs the `SYS_EXIT` syscall.
     */
    movq %rax, %rdi
    call exit

    /*
     * Step 6: Fallback emergency trap.
     *
     * `exit()` is marked `__attribute__((noreturn))` and should never return.
     * If, due to a severe libc corruption or bug, `exit` does return, we enter
     * an unrecoverable fallback loop: directly execute the SYS_EXIT (60) system
     * call with exit status 1, halt the CPU, and branch back to self.
     */
.Lhang:
    movq $60, %rax            # %rax = SYS_EXIT system call number (60)
    movq $1, %rdi             # %rdi = exit status code (1 indicates abnormal error)
    syscall                   # Trigger kernel syscall dispatcher
    hlt                       # Halt instruction (fails in Ring 3 if IOPL != 3, raising GP)
    jmp .Lhang                # Infinite trap loop to prevent execution of unmapped memory

.size _start, . - _start
