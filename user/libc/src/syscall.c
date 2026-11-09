/**
 * @file syscall.c
 * @brief NullOS Userspace Variadic System Call Wrapper
 *
 * This file implements the generic, variadic `syscall(long number, ...)` wrapper
 * function in the NullOS C library. It bridges variable-argument userspace function
 * calls to the fixed-register x86_64 system call interface (`syscall6`).
 *
 * In x86_64 System V AMD64 ABI:
 * - Up to 6 integer or pointer arguments are passed into system calls via registers:
 *   - RAX: System call number (identifies kernel operation)
 *   - RDI: 1st argument
 *   - RSI: 2nd argument
 *   - RDX: 3rd argument
 *   - R10: 4th argument (Note: R10 is used instead of RCX because the x86 `syscall`
 *          instruction overwrites RCX with the saved RIP)
 *   - R8:  5th argument
 *   - R9:  6th argument
 * - Return value is placed in RAX.
 * - RCX and R11 are clobbered by the processor hardware.
 */

#include <sys/syscall.h>
#include <stdint.h>

/**
 * @brief Dispatches a generic system call with a variable number of arguments.
 *
 * Extracts up to six 64-bit word arguments from the variadic argument list (`...`)
 * and forwards them to `syscall6`. If the system call expects fewer than 6 arguments,
 * the remaining registers will hold the arbitrary stack/register contents unpacked
 * from the variadic list, which the kernel will ignore based on the system call's
 * specific signature.
 *
 * @param number The system call identifier (e.g. `SYS_READ`, `SYS_WRITE`, `SYS_EXIT`).
 * @param ...    Variable arguments (up to 6 `long` values or pointers).
 * @return The 64-bit return value from the kernel (typically 0 on success, or a
 *         negative error code on failure).
 */
long syscall(long number, ...) {
    __builtin_va_list ap;
    __builtin_va_start(ap, number);

    /*
     * Extract up to 6 64-bit integer arguments from the variadic argument list.
     * In the AMD64 ABI, variadic arguments beyond the fixed parameters are passed
     * in the remaining general-purpose argument registers or on the stack.
     */
    long a1 = __builtin_va_arg(ap, long);
    long a2 = __builtin_va_arg(ap, long);
    long a3 = __builtin_va_arg(ap, long);
    long a4 = __builtin_va_arg(ap, long);
    long a5 = __builtin_va_arg(ap, long);
    long a6 = __builtin_va_arg(ap, long);

    __builtin_va_end(ap);

    /*
     * Delegate to the inline-assembly 6-argument dispatcher defined in <sys/syscall.h>,
     * which marshals arguments into RAX, RDI, RSI, RDX, R10, R8, and R9 before
     * executing the `syscall` instruction.
     */
    return syscall6(number, a1, a2, a3, a4, a5, a6);
}
