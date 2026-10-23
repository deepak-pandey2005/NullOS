#ifndef _SYS_SYSCALL_H
#define _SYS_SYSCALL_H

#include <stdint.h>

/* NullOS System Call Numbers matching kernel ABI */
#define SYS_READ        0
#define SYS_WRITE       1
#define SYS_OPEN        2
#define SYS_CLOSE       3
#define SYS_STAT        4
#define SYS_LSEEK       8
#define SYS_MMAP        9
#define SYS_PIPE        22
#define SYS_YIELD       24
#define SYS_GETPID      39
#define SYS_CLONE       56
#define SYS_THREAD_EXIT 57
#define SYS_THREAD_JOIN 58
#define SYS_EXIT        60
#define SYS_WAITPID     61
#define SYS_KILL        62
#define SYS_SLEEP       100
#define SYS_ARCH_PRCTL  158
#define SYS_GETTID      186
#define SYS_FUTEX       202

#define ARCH_SET_GS     0x1001
#define ARCH_SET_FS     0x1002
#define ARCH_GET_FS     0x1003
#define ARCH_GET_GS     0x1004

/*
 * x86_64 Fast System Call Invocations (SYSCALL / SYSRET ABI).
 * Arguments:
 *   rax: Syscall Number
 *   rdi: Argument 1
 *   rsi: Argument 2
 *   rdx: Argument 3
 *   r10: Argument 4 (note: r10 instead of rcx because syscall destroys rcx)
 *   r8:  Argument 5
 *   r9:  Argument 6
 * Return: rax
 * Clobbered: rcx, r11
 */

static inline long syscall0(long n) {
    long ret;
    __asm__ __volatile__(
        "syscall"
        : "=a"(ret)
        : "a"(n)
        : "rcx", "r11", "memory"
    );
    return ret;
}

static inline long syscall1(long n, long a1) {
    long ret;
    __asm__ __volatile__(
        "syscall"
        : "=a"(ret)
        : "a"(n), "D"(a1)
        : "rcx", "r11", "memory"
    );
    return ret;
}

static inline long syscall2(long n, long a1, long a2) {
    long ret;
    __asm__ __volatile__(
        "syscall"
        : "=a"(ret)
        : "a"(n), "D"(a1), "S"(a2)
        : "rcx", "r11", "memory"
    );
    return ret;
}

static inline long syscall3(long n, long a1, long a2, long a3) {
    long ret;
    __asm__ __volatile__(
        "syscall"
        : "=a"(ret)
        : "a"(n), "D"(a1), "S"(a2), "d"(a3)
        : "rcx", "r11", "memory"
    );
    return ret;
}

static inline long syscall4(long n, long a1, long a2, long a3, long a4) {
    long ret;
    register long r10 __asm__("r10") = a4;
    __asm__ __volatile__(
        "syscall"
        : "=a"(ret)
        : "a"(n), "D"(a1), "S"(a2), "d"(a3), "r"(r10)
        : "rcx", "r11", "memory"
    );
    return ret;
}

static inline long syscall5(long n, long a1, long a2, long a3, long a4, long a5) {
    long ret;
    register long r10 __asm__("r10") = a4;
    register long r8  __asm__("r8")  = a5;
    __asm__ __volatile__(
        "syscall"
        : "=a"(ret)
        : "a"(n), "D"(a1), "S"(a2), "d"(a3), "r"(r10), "r"(r8)
        : "rcx", "r11", "memory"
    );
    return ret;
}

static inline long syscall6(long n, long a1, long a2, long a3, long a4, long a5, long a6) {
    long ret;
    register long r10 __asm__("r10") = a4;
    register long r8  __asm__("r8")  = a5;
    register long r9  __asm__("r9")  = a6;
    __asm__ __volatile__(
        "syscall"
        : "=a"(ret)
        : "a"(n), "D"(a1), "S"(a2), "d"(a3), "r"(r10), "r"(r8), "r"(r9)
        : "rcx", "r11", "memory"
    );
    return ret;
}

/* Generic syscall function */
long syscall(long number, ...);

#endif /* _SYS_SYSCALL_H */
