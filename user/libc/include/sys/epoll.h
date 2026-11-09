/**
 * @file sys/epoll.h
 * @brief NullOS Scalable I/O Event Notification Facility (epoll)
 *
 * This header defines the userspace data structures, constants, and system call
 * wrappers for the epoll event multiplexing subsystem in NullOS, compatible with
 * the standard Linux epoll(7) API.
 */

#ifndef _SYS_EPOLL_H
#define _SYS_EPOLL_H

#include <stdint.h>
#include <sys/syscall.h>

/* ============================================================================
 * Epoll Control Operation Codes (epoll_ctl op)
 * ============================================================================ */

/** Register a target file descriptor on the epoll instance. */
#define EPOLL_CTL_ADD 1

/** Remove a target file descriptor from the epoll instance. */
#define EPOLL_CTL_DEL 2

/** Change the event mask associated with a registered file descriptor. */
#define EPOLL_CTL_MOD 3

/* ============================================================================
 * Epoll Event Types
 * ============================================================================ */

/** The associated file is available for read operations. */
#define EPOLLIN       0x0001

/** There is urgent or high-priority data available to read. */
#define EPOLLPRI      0x0002

/** The associated file is available for write operations. */
#define EPOLLOUT      0x0004

/** Error condition happened on the associated file descriptor. */
#define EPOLLERR      0x0008

/** Hang up happened on the associated file descriptor. */
#define EPOLLHUP      0x0010

/** Sets Edge Triggered behavior for the associated file descriptor. */
#define EPOLLET       (1U << 31)

/** Sets one-shot notification behavior: unregisters interest after first event. */
#define EPOLLONESHOT  (1U << 30)

/* ============================================================================
 * Epoll Flags (epoll_create1)
 * ============================================================================ */

/** Set the close-on-exec (FD_CLOEXEC) flag on the new file descriptor. */
#define EPOLL_CLOEXEC 02000000

/** Set O_NONBLOCK status flag on the open file description. */
#define EPOLL_NONBLOCK 04000

/**
 * @union epoll_data
 * @brief Opaque 64-bit user data payload attached to a registered file descriptor.
 */
typedef union epoll_data {
    void    *ptr; /**< Pointer to user-allocated context object. */
    int      fd;  /**< File descriptor integer. */
    uint32_t u32; /**< 32-bit unsigned integer tag. */
    uint64_t u64; /**< 64-bit unsigned integer tag. */
} epoll_data_t;

/**
 * @struct epoll_event
 * @brief Structure returned by `epoll_wait` describing a ready event.
 */
struct epoll_event {
    uint32_t     events; /**< Epoll event bitmask (EPOLLIN, EPOLLOUT, etc.). */
    epoll_data_t data;   /**< User data payload associated with this descriptor. */
} __attribute__((packed));

/**
 * @brief Creates an epoll file descriptor.
 *
 * @param size Historical hint specifying expected number of descriptors (must be > 0).
 * @return New file descriptor representing the epoll instance, or negative on error.
 */
static inline int epoll_create(int size) {
    return (int)syscall1(SYS_EPOLL_CREATE, (long)size);
}

/**
 * @brief Creates an epoll file descriptor with specific creation flags.
 *
 * @param flags Flags such as EPOLL_CLOEXEC.
 * @return New epoll file descriptor, or negative on error.
 */
static inline int epoll_create1(int flags) {
    return (int)syscall1(SYS_EPOLL_CREATE1, (long)flags);
}

/**
 * @brief Controls an epoll file descriptor (adds, modifies, or removes interest).
 *
 * @param epfd  The epoll instance file descriptor.
 * @param op    Operation: EPOLL_CTL_ADD, EPOLL_CTL_MOD, or EPOLL_CTL_DEL.
 * @param fd    The target file descriptor to monitor.
 * @param event Pointer to event structure configuring monitored events and user data.
 * @return 0 on success, or negative errno on error.
 */
static inline int epoll_ctl(int epfd, int op, int fd, struct epoll_event *event) {
    return (int)syscall4(SYS_EPOLL_CTL, (long)epfd, (long)op, (long)fd, (long)event);
}

/**
 * @brief Waits for I/O events on an epoll instance.
 *
 * Blocks the calling thread until at least one monitored file descriptor reports
 * readiness, or until the timeout interval expires.
 *
 * @param epfd      The epoll file descriptor.
 * @param events    Array of `epoll_event` structures where ready events are written.
 * @param maxevents Maximum number of events to return (capacity of `events` array).
 * @param timeout   Timeout in milliseconds (-1 for infinite blocking, 0 for non-blocking poll).
 * @return Number of ready file descriptors returned, 0 on timeout, or negative on error.
 */
static inline int epoll_wait(int epfd, struct epoll_event *events, int maxevents, int timeout) {
    return (int)syscall4(SYS_EPOLL_WAIT, (long)epfd, (long)events, (long)maxevents, (long)timeout);
}

#endif /* _SYS_EPOLL_H */
