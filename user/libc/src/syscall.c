#include <sys/syscall.h>
#include <stdint.h>

long syscall(long number, ...) {
    __builtin_va_list ap;
    __builtin_va_start(ap, number);

    long a1 = __builtin_va_arg(ap, long);
    long a2 = __builtin_va_arg(ap, long);
    long a3 = __builtin_va_arg(ap, long);
    long a4 = __builtin_va_arg(ap, long);
    long a5 = __builtin_va_arg(ap, long);
    long a6 = __builtin_va_arg(ap, long);

    __builtin_va_end(ap);

    return syscall6(number, a1, a2, a3, a4, a5, a6);
}
