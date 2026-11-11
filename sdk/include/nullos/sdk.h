/**
 * @file nullos/sdk.h
 * @brief NullOS Official User-Space SDK Header (Phase 18 - M39)
 *
 * Provides canonical NullOS ABI version constants, hardware cycle/PID helpers,
 * compiler attribute macros, and portable Ring 3 application primitives.
 */

#ifndef _NULLOS_SDK_H
#define _NULLOS_SDK_H

#include <stdint.h>
#include <stddef.h>
#include <sys/syscall.h>

#define NULLOS_SDK_VERSION_MAJOR 0
#define NULLOS_SDK_VERSION_MINOR 18
#define NULLOS_SDK_VERSION_PATCH 39
#define NULLOS_SDK_VERSION_STRING "0.18.39-m39"
#define NULLOS_ARCH_X86_64       1

#define NULLOS_ALIGNED(n)   __attribute__((aligned(n)))
#define NULLOS_PACKED       __attribute__((packed))
#define NULLOS_NORETURN     __attribute__((noreturn))
#define NULLOS_SECTION(s)   __attribute__((section(s)))

/** Reads the 64-bit processor Time Stamp Counter directly in Ring 3. */
static inline uint64_t nullos_rdtsc(void) {
    uint32_t lo, hi;
    __asm__ __volatile__("rdtsc" : "=a"(lo), "=d"(hi));
    return ((uint64_t)hi << 32) | lo;
}

/** Invokes fast O(1) SYS_GETPID via NullOS Ring 3 -> Ring 0 ABI. */
static inline uint64_t nullos_getpid(void) {
    return (uint64_t)syscall0(SYS_GETPID);
}

/** Voluntarily yields remaining CPU timeslice to the kernel scheduler. */
static inline void nullos_yield(void) {
    (void)syscall0(SYS_YIELD);
}

/** Suspends execution for `ticks` APIC timer intervals. */
static inline void nullos_sleep_ticks(uint64_t ticks) {
    (void)syscall1(SYS_SLEEP, (long)ticks);
}

#endif /* _NULLOS_SDK_H */

