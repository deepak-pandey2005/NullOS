/**
 * @file sys/eventfd.h
 * @brief NullOS Event Notification File Descriptor Interface (eventfd)
 *
 * This header defines constants, types, and inline convenience functions for
 * creating and operating on 64-bit event notification descriptors in NullOS,
 * compatible with Linux eventfd(2).
 */

#ifndef _SYS_EVENTFD_H
#define _SYS_EVENTFD_H

#include <stdint.h>
#include <sys/syscall.h>

/* ============================================================================
 * Eventfd Flags (eventfd2)
 * ============================================================================ */

/** Provide semaphore-like read semantics: each read decrements counter by 1. */
#define EFD_SEMAPHORE 1

/** Set the close-on-exec flag on the new eventfd descriptor. */
#define EFD_CLOEXEC   02000000

/** Set non-blocking I/O mode on the eventfd descriptor. */
#define EFD_NONBLOCK  04000

/** 64-bit integer type representing an event counter value. */
typedef uint64_t eventfd_t;

/**
 * @brief Creates an event notification file descriptor.
 *
 * Initializes an internal 64-bit counter to `initval`.
 *
 * @param initval Initial counter value.
 * @param flags   Creation flags (EFD_SEMAPHORE, EFD_CLOEXEC, EFD_NONBLOCK).
 * @return New file descriptor on success, or -1 on error.
 */
static inline int eventfd(unsigned int initval, int flags) {
    return (int)syscall2(SYS_EVENTFD2, (long)initval, (long)flags);
}

/**
 * @brief Reads the current 64-bit counter value from an eventfd descriptor.
 *
 * If the counter is non-zero, returns the value and resets counter to zero
 * (or decrements by 1 if EFD_SEMAPHORE was set). If counter is 0, blocks until
 * an increment is written or returns EAGAIN if non-blocking.
 *
 * @param fd    Eventfd file descriptor.
 * @param value Pointer to variable receiving the 64-bit counter value.
 * @return 0 on success, or -1 on error.
 */
static inline int eventfd_read(int fd, eventfd_t *value) {
    return syscall3(SYS_READ, (long)fd, (long)value, sizeof(eventfd_t)) == (long)sizeof(eventfd_t) ? 0 : -1;
}

/**
 * @brief Adds a 64-bit value to an eventfd descriptor's internal counter.
 *
 * Atomically increments the counter and unblocks any waiting readers or epoll/poll waiters.
 *
 * @param fd    Eventfd file descriptor.
 * @param value 64-bit unsigned integer to add to counter (must be non-zero and not 0xFFFFFFFFFFFFFFFF).
 * @return 0 on success, or -1 on error.
 */
static inline int eventfd_write(int fd, eventfd_t value) {
    return syscall3(SYS_WRITE, (long)fd, (long)&value, sizeof(eventfd_t)) == (long)sizeof(eventfd_t) ? 0 : -1;
}

#endif /* _SYS_EVENTFD_H */
