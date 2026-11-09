/**
 * @file sys/uio.h
 * @brief NullOS Vectored (Scatter-Gather) I/O Operations (readv / writev)
 *
 * This header defines the `iovec` memory vector structure and inline wrappers
 * for reading into or writing from multiple non-contiguous memory buffers in
 * a single atomic system call.
 */

#ifndef _SYS_UIO_H
#define _SYS_UIO_H

#include <stdint.h>
#include <stddef.h>
#include <sys/syscall.h>

/**
 * @struct iovec
 * @brief Vector element representing a contiguous slice of memory.
 */
struct iovec {
    void  *iov_base; /**< Starting virtual memory address of the buffer. */
    size_t iov_len;  /**< Number of bytes in this buffer slice. */
};

/**
 * @brief Reads data from a file descriptor into multiple buffers (scatter input).
 *
 * Invokes the `SYS_READV` (19) system call.
 *
 * @param fd     File descriptor to read from.
 * @param iov    Array of `iovec` structures specifying destination buffers.
 * @param iovcnt Number of elements in `iov` array.
 * @return Total number of bytes read across all buffers, or negative on error.
 */
static inline long readv(int fd, const struct iovec *iov, int iovcnt) {
    return syscall3(SYS_READV, (long)fd, (long)iov, (long)iovcnt);
}

/**
 * @brief Writes data from multiple buffers to a file descriptor (gather output).
 *
 * Invokes the `SYS_WRITEV` (20) system call.
 *
 * @param fd     File descriptor to write to.
 * @param iov    Array of `iovec` structures specifying source data buffers.
 * @param iovcnt Number of elements in `iov` array.
 * @return Total number of bytes written across all buffers, or negative on error.
 */
static inline long writev(int fd, const struct iovec *iov, int iovcnt) {
    return syscall3(SYS_WRITEV, (long)fd, (long)iov, (long)iovcnt);
}

#endif /* _SYS_UIO_H */
