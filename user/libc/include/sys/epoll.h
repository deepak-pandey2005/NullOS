#ifndef _SYS_EPOLL_H
#define _SYS_EPOLL_H

#include <stdint.h>
#include <sys/syscall.h>

#define EPOLL_CTL_ADD 1
#define EPOLL_CTL_DEL 2
#define EPOLL_CTL_MOD 3

#define EPOLLIN       0x0001
#define EPOLLPRI      0x0002
#define EPOLLOUT      0x0004
#define EPOLLERR      0x0008
#define EPOLLHUP      0x0010
#define EPOLLET       (1U << 31)
#define EPOLLONESHOT  (1U << 30)

#define EPOLL_CLOEXEC 02000000
#define EPOLL_NONBLOCK 04000

typedef union epoll_data {
    void    *ptr;
    int      fd;
    uint32_t u32;
    uint64_t u64;
} epoll_data_t;

struct epoll_event {
    uint32_t     events;
    epoll_data_t data;
} __attribute__((packed));

static inline int epoll_create(int size) {
    return (int)syscall1(SYS_EPOLL_CREATE, (long)size);
}

static inline int epoll_create1(int flags) {
    return (int)syscall1(SYS_EPOLL_CREATE1, (long)flags);
}

static inline int epoll_ctl(int epfd, int op, int fd, struct epoll_event *event) {
    return (int)syscall4(SYS_EPOLL_CTL, (long)epfd, (long)op, (long)fd, (long)event);
}

static inline int epoll_wait(int epfd, struct epoll_event *events, int maxevents, int timeout) {
    return (int)syscall4(SYS_EPOLL_WAIT, (long)epfd, (long)events, (long)maxevents, (long)timeout);
}

#endif /* _SYS_EPOLL_H */

