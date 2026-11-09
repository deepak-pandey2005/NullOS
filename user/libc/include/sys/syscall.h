/**
 * @file sys/syscall.h
 * @brief NullOS System Call Numbers and Fast x86_64 Invocation Macros
 *
 * This header defines the standard numeric identifiers for all NullOS system
 * calls supported by the kernel's Ring 3 -> Ring 0 dispatcher (`kernel/src/arch/x86_64/syscall.rs`),
 * as well as inline assembly functions (`syscall0` through `syscall6`) that implement
 * the System V AMD64 system call ABI using the hardware `syscall` instruction.
 *
 * ============================================================================
 * x86_64 SYSCALL / SYSRET HARDWARE CALLING CONVENTION:
 * ============================================================================
 * When executing the `syscall` instruction:
 *   - %rax: System call identification number (e.g., SYS_READ=0, SYS_WRITE=1).
 *   - %rdi: 1st argument.
 *   - %rsi: 2nd argument.
 *   - %rdx: 3rd argument.
 *   - %r10: 4th argument (NOTE: The standard userspace calling convention uses %rcx
 *           for the 4th argument, but the x86 `syscall` hardware instruction automatically
 *           overwrites %rcx with the saved %rip; therefore, the kernel ABI substitutes %r10).
 *   - %r8:  5th argument.
 *   - %r9:  6th argument.
 *
 * Return value:
 *   - %rax holds the 64-bit return code or negative errno.
 *
 * Processor Clobbers:
 *   - %rcx (stores saved user %rip)
 *   - %r11 (stores saved user %rflags)
 */

#ifndef _SYS_SYSCALL_H
#define _SYS_SYSCALL_H

#include <stdint.h>

/* ============================================================================
 * System Call Identifiers (Matching NullOS Kernel ABI)
 * ============================================================================ */

/** Read from a file descriptor (`fd`, `buf`, `count`). */
#define SYS_READ            0

/** Write to a file descriptor (`fd`, `buf`, `count`). */
#define SYS_WRITE           1

/** Open or create a file (`pathname`, `flags`, `mode`). */
#define SYS_OPEN            2

/** Close an open file descriptor (`fd`). */
#define SYS_CLOSE           3

/** Get file status (`pathname`, `statbuf`). */
#define SYS_STAT            4

/** Wait for events on file descriptors (`fds`, `nfds`, `timeout`). */
#define SYS_POLL            7

/** Reposition read/write file offset (`fd`, `offset`, `whence`). */
#define SYS_LSEEK           8

/** Map pages of memory (`addr`, `length`, `prot`, `flags`, `fd`, `offset`). */
#define SYS_MMAP            9

/** Read data into multiple buffers via scatter-gather (`fd`, `iov`, `iovcnt`). */
#define SYS_READV           19

/** Write data from multiple buffers via scatter-gather (`fd`, `iov`, `iovcnt`). */
#define SYS_WRITEV          20

/** Create an inter-process pipe unidirectional channel (`pipefd[2]`). */
#define SYS_PIPE            22

/** Yield processor execution to another runnable thread. */
#define SYS_YIELD           24

/** Get process identifier of calling process. */
#define SYS_GETPID          39

/** Create a new lightweight execution thread (`fn`, `child_stack`, `arg`). */
#define SYS_CLONE           56

/** Terminate the calling thread with an exit status (`retval`). */
#define SYS_THREAD_EXIT     57

/** Wait for a specific thread to terminate and retrieve status (`tid`, `status`). */
#define SYS_THREAD_JOIN     58

/** Terminate the calling process with exit status (`status`). */
#define SYS_EXIT            60

/** Wait for child process state change (`pid`, `status`, `options`). */
#define SYS_WAITPID         61

/** Send a signal to a process or thread (`pid`, `sig`). */
#define SYS_KILL            62

/** Put calling thread to sleep for timer ticks (`ticks`). */
#define SYS_SLEEP           100

/** Set architecture-specific thread register state (e.g. FS/GS base). */
#define SYS_ARCH_PRCTL      158

/** Get thread identifier (TID) of the calling thread. */
#define SYS_GETTID          186

/** Fast userspace locking and waiting primitive (`uaddr`, `op`, `val`, `timeout`). */
#define SYS_FUTEX           202

/** Create an epoll event notification instance (`size`). */
#define SYS_EPOLL_CREATE    213

/** Wait for I/O events on an epoll file descriptor (`epfd`, `events`, `maxevents`, `timeout`). */
#define SYS_EPOLL_WAIT      232

/** Control interface for an epoll file descriptor (`epfd`, `op`, `fd`, `event`). */
#define SYS_EPOLL_CTL       233

/** Create an interval timer file descriptor (`clockid`, `flags`). */
#define SYS_TIMERFD_CREATE  283

/** Create an event notification counter file descriptor (`initval`, `flags`). */
#define SYS_EVENTFD         284

/** Arm or disarm a timer file descriptor (`fd`, `flags`, `new_val`, `old_val`). */
#define SYS_TIMERFD_SETTIME 286

/** Query current timer status (`fd`, `curr_val`). */
#define SYS_TIMERFD_GETTIME 287

/** Create an event notification counter file descriptor with flags. */
#define SYS_EVENTFD2        290

/** Create an epoll event notification instance with flags (`flags`). */
#define SYS_EPOLL_CREATE1   291

/* Architecture Prctl Sub-operations for x86_64 Segment Registers */
#define ARCH_SET_GS         0x1001
#define ARCH_SET_FS         0x1002
#define ARCH_GET_FS         0x1003
#define ARCH_GET_GS         0x1004

/* ============================================================================
 * Fast Inline Assembly System Call Dispatchers
 * ============================================================================ */

/**
 * @brief Executes a 0-argument system call via the `syscall` instruction.
 *
 * @param n System call number placed in %rax.
 * @return 64-bit kernel result returned in %rax.
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

/**
 * @brief Executes a 1-argument system call.
 *
 * @param n  System call number (%rax).
 * @param a1 1st argument (%rdi).
 * @return 64-bit kernel result (%rax).
 */
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

/**
 * @brief Executes a 2-argument system call.
 *
 * @param n  System call number (%rax).
 * @param a1 1st argument (%rdi).
 * @param a2 2nd argument (%rsi).
 * @return 64-bit kernel result (%rax).
 */
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

/**
 * @brief Executes a 3-argument system call.
 *
 * @param n  System call number (%rax).
 * @param a1 1st argument (%rdi).
 * @param a2 2nd argument (%rsi).
 * @param a3 3rd argument (%rdx).
 * @return 64-bit kernel result (%rax).
 */
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

/**
 * @brief Executes a 4-argument system call.
 *
 * Marshals the 4th argument explicitly into %r10 per x86_64 syscall ABI.
 *
 * @param n  System call number (%rax).
 * @param a1 1st argument (%rdi).
 * @param a2 2nd argument (%rsi).
 * @param a3 3rd argument (%rdx).
 * @param a4 4th argument (%r10).
 * @return 64-bit kernel result (%rax).
 */
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

/**
 * @brief Executes a 5-argument system call.
 *
 * @param n  System call number (%rax).
 * @param a1 1st argument (%rdi).
 * @param a2 2nd argument (%rsi).
 * @param a3 3rd argument (%rdx).
 * @param a4 4th argument (%r10).
 * @param a5 5th argument (%r8).
 * @return 64-bit kernel result (%rax).
 */
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

/**
 * @brief Executes a 6-argument system call.
 *
 * @param n  System call number (%rax).
 * @param a1 1st argument (%rdi).
 * @param a2 2nd argument (%rsi).
 * @param a3 3rd argument (%rdx).
 * @param a4 4th argument (%r10).
 * @param a5 5th argument (%r8).
 * @param a6 6th argument (%r9).
 * @return 64-bit kernel result (%rax).
 */
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

/** Generic variadic system call wrapper prototype. */
long syscall(long number, ...);

#endif /* _SYS_SYSCALL_H */
