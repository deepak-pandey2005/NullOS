/**
 * @file sys/futex.h
 * @brief NullOS Fast Userspace Mutex (futex) System Interface
 *
 * This header defines operations and inline helper functions for interacting
 * with the NullOS kernel's futex wait-queue subsystem (`kernel/src/sync/futex.rs`),
 * providing foundational primitives for userspace mutexes, condition variables,
 * and semaphores.
 */

#ifndef _SYS_FUTEX_H
#define _SYS_FUTEX_H

#include <stdint.h>
#include <sys/syscall.h>

/* ============================================================================
 * Futex Operations
 * ============================================================================ */

/** Suspend thread if the 32-bit integer at uaddr matches val. */
#define FUTEX_WAIT          0

/** Wake at most count threads currently suspended at uaddr. */
#define FUTEX_WAKE          1

/** Requeue waiting threads from uaddr1 to uaddr2 without waking them. */
#define FUTEX_REQUEUE       3

/** Flag indicating futex is process-private (avoiding inter-process overhead). */
#define FUTEX_PRIVATE_FLAG  128

/**
 * @brief Suspends the calling thread if the value at `uaddr` equals `val`.
 *
 * Atomically checks whether `*uaddr == val`. If true, the kernel suspends the
 * calling thread on the futex wait queue until awakened via `FUTEX_WAKE` or interrupted.
 *
 * @param uaddr Pointer to userspace 32-bit integer futex word.
 * @param val   Expected current value at `uaddr`.
 * @return 0 on successful wake, or negative errno on error.
 */
static inline long futex_wait(uint32_t *uaddr, uint32_t val) {
    return syscall4(SYS_FUTEX, (long)uaddr, FUTEX_WAIT, val, 0);
}

/**
 * @brief Wakes up to `count` threads waiting on the futex at `uaddr`.
 *
 * @param uaddr Pointer to userspace 32-bit integer futex word.
 * @param count Maximum number of waiting threads to wake up (e.g. 1 for signal, INT_MAX for broadcast).
 * @return Number of threads successfully awakened, or negative errno on error.
 */
static inline long futex_wake(uint32_t *uaddr, uint32_t count) {
    return syscall4(SYS_FUTEX, (long)uaddr, FUTEX_WAKE, count, 0);
}

/**
 * @brief Wakes `count` threads at `uaddr1` and moves `req_count` remaining threads to `uaddr2`.
 *
 * Used primarily by condition variables to avoid thundering-herd wakeups on the mutex.
 *
 * @param uaddr1    Source futex address.
 * @param count     Number of threads to awaken immediately at uaddr1.
 * @param uaddr2    Destination futex address where remaining threads are moved.
 * @param req_count Maximum number of threads to requeue to uaddr2.
 * @return Number of threads awakened or requeued, or negative errno on error.
 */
static inline long futex_requeue(uint32_t *uaddr1, uint32_t count, uint32_t *uaddr2, uint32_t req_count) {
    return syscall6(SYS_FUTEX, (long)uaddr1, FUTEX_REQUEUE, count, (long)uaddr2, req_count, 0);
}

#endif /* _SYS_FUTEX_H */
