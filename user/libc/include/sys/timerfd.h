/**
 * @file sys/timerfd.h
 * @brief NullOS Timers Driven by File Descriptors (timerfd)
 *
 * This header defines clock types, timing structures, and inline wrappers for
 * creating and manipulating timer file descriptors in NullOS, compatible with
 * Linux timerfd_create(2).
 */

#ifndef _SYS_TIMERFD_H
#define _SYS_TIMERFD_H

#include <stdint.h>
#include <sys/syscall.h>

/* ============================================================================
 * Clock Identifiers
 * ============================================================================ */

/** Settable system-wide real-time clock. */
#define CLOCK_REALTIME  0

/** Non-settable monotonically increasing clock since system boot. */
#define CLOCK_MONOTONIC 1

/* ============================================================================
 * Timerfd Flags
 * ============================================================================ */

/** Interpret new_value as an absolute expiration time rather than relative. */
#define TFD_TIMER_ABSTIME 1

/** Set close-on-exec flag on the new timer file descriptor. */
#define TFD_CLOEXEC       02000000

/** Set non-blocking mode on the timer file descriptor. */
#define TFD_NONBLOCK      04000

/**
 * @struct itimerspec
 * @brief Structure specifying interval and initial expiration for a timer.
 */
struct itimerspec {
    struct {
        int64_t tv_sec;  /**< Interval seconds for periodic timer (0 for one-shot). */
        int64_t tv_nsec; /**< Interval nanoseconds for periodic timer. */
    } it_interval;
    struct {
        int64_t tv_sec;  /**< Initial expiration seconds from arming. */
        int64_t tv_nsec; /**< Initial expiration nanoseconds from arming. */
    } it_value;
};

/**
 * @brief Creates a new timer object and returns an associated file descriptor.
 *
 * @param clockid Clock source: CLOCK_REALTIME or CLOCK_MONOTONIC.
 * @param flags   Creation flags (TFD_NONBLOCK, TFD_CLOEXEC).
 * @return New timer file descriptor, or negative on error.
 */
static inline int timerfd_create(int clockid, int flags) {
    return (int)syscall2(SYS_TIMERFD_CREATE, (long)clockid, (long)flags);
}

/**
 * @brief Arms or disarms the timer referred to by the file descriptor.
 *
 * Setting `new_value->it_value` to non-zero arms the timer; setting it to 0 disarms it.
 *
 * @param fd        Timer file descriptor.
 * @param flags     0 for relative time, or TFD_TIMER_ABSTIME for absolute time.
 * @param new_value Pointer to new timer configuration.
 * @param old_value If non-NULL, receives previous timer configuration.
 * @return 0 on success, or negative errno on error.
 */
static inline int timerfd_settime(int fd, int flags, const struct itimerspec *new_value, struct itimerspec *old_value) {
    return (int)syscall4(SYS_TIMERFD_SETTIME, (long)fd, (long)flags, (long)new_value, (long)old_value);
}

/**
 * @brief Returns the current setting of the timer referred to by file descriptor.
 *
 * @param fd         Timer file descriptor.
 * @param curr_value Pointer to structure receiving current expiration remaining.
 * @return 0 on success, or negative errno on error.
 */
static inline int timerfd_gettime(int fd, struct itimerspec *curr_value) {
    return (int)syscall2(SYS_TIMERFD_GETTIME, (long)fd, (long)curr_value);
}

#endif /* _SYS_TIMERFD_H */
