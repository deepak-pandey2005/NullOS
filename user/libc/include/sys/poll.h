/**
 * @file sys/poll.h
 * @brief NullOS Synchronous I/O Multiplexing Definitions (poll)
 *
 * This header defines constants, data structures, and inline wrappers for
 * monitoring multiple file descriptors for readiness events via `poll(2)`.
 */

#ifndef _SYS_POLL_H
#define _SYS_POLL_H

#include <stdint.h>
#include <sys/syscall.h>

/* ============================================================================
 * Poll Event Bitmasks
 * ============================================================================ */

/** There is data to read. */
#define POLLIN      0x0001

/** There is urgent data to read. */
#define POLLPRI     0x0002

/** Writing is now possible without blocking. */
#define POLLOUT     0x0004

/** Error condition (output only in revents). */
#define POLLERR     0x0008

/** Hang up (output only in revents). */
#define POLLHUP     0x0010

/** Invalid request: fd is not open (output only in revents). */
#define POLLNVAL    0x0020

/**
 * @struct pollfd
 * @brief Data structure describing a file descriptor to be polled.
 */
struct pollfd {
    int   fd;      /**< Open file descriptor to monitor. */
    short events;  /**< Requested event bitmask (input from caller). */
    short revents; /**< Returned event bitmask (populated by kernel). */
};

/** Integer type representing the number of file descriptors in poll array. */
typedef unsigned long nfds_t;

/**
 * @brief Waits for one of a set of file descriptors to become ready to perform I/O.
 *
 * Invokes the `SYS_POLL` (7) system call.
 *
 * @param fds     Array of `pollfd` structures.
 * @param nfds    Number of items in the `fds` array.
 * @param timeout Milliseconds to wait before returning (-1 for infinite, 0 for immediate poll).
 * @return Positive count of descriptors with non-zero revents, 0 on timeout, or negative on error.
 */
static inline int poll(struct pollfd *fds, nfds_t nfds, int timeout) {
    return (int)syscall3(SYS_POLL, (long)fds, (long)nfds, (long)timeout);
}

#endif /* _SYS_POLL_H */
