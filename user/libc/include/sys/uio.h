#ifndef _SYS_UIO_H
#define _SYS_UIO_H

#include <stdint.h>
#include <stddef.h>
#include <sys/syscall.h>

struct iovec {
    void  *iov_base;
    size_t iov_len;
};

static inline long readv(int fd, const struct iovec *iov, int iovcnt) {
    return syscall3(SYS_READV, (long)fd, (long)iov, (long)iovcnt);
}

static inline long writev(int fd, const struct iovec *iov, int iovcnt) {
    return syscall3(SYS_WRITEV, (long)fd, (long)iov, (long)iovcnt);
}

#endif /* _SYS_UIO_H */

