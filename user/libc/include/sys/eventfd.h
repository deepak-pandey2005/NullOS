#ifndef _SYS_EVENTFD_H
#define _SYS_EVENTFD_H

#include <stdint.h>
#include <sys/syscall.h>

#define EFD_SEMAPHORE 1
#define EFD_CLOEXEC   02000000
#define EFD_NONBLOCK  04000

typedef uint64_t eventfd_t;

static inline int eventfd(unsigned int initval, int flags) {
    return (int)syscall2(SYS_EVENTFD2, (long)initval, (long)flags);
}

static inline int eventfd_read(int fd, eventfd_t *value) {
    return syscall3(SYS_READ, (long)fd, (long)value, sizeof(eventfd_t)) == (long)sizeof(eventfd_t) ? 0 : -1;
}

static inline int eventfd_write(int fd, eventfd_t value) {
    return syscall3(SYS_WRITE, (long)fd, (long)&value, sizeof(eventfd_t)) == (long)sizeof(eventfd_t) ? 0 : -1;
}

#endif /* _SYS_EVENTFD_H */

