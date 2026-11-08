#ifndef _SYS_TIMERFD_H
#define _SYS_TIMERFD_H

#include <stdint.h>
#include <sys/syscall.h>

#define CLOCK_REALTIME  0
#define CLOCK_MONOTONIC 1

#define TFD_TIMER_ABSTIME 1
#define TFD_CLOEXEC       02000000
#define TFD_NONBLOCK      04000

struct itimerspec {
    struct {
        int64_t tv_sec;
        int64_t tv_nsec;
    } it_interval;
    struct {
        int64_t tv_sec;
        int64_t tv_nsec;
    } it_value;
};

static inline int timerfd_create(int clockid, int flags) {
    return (int)syscall2(SYS_TIMERFD_CREATE, (long)clockid, (long)flags);
}

static inline int timerfd_settime(int fd, int flags, const struct itimerspec *new_value, struct itimerspec *old_value) {
    return (int)syscall4(SYS_TIMERFD_SETTIME, (long)fd, (long)flags, (long)new_value, (long)old_value);
}

static inline int timerfd_gettime(int fd, struct itimerspec *curr_value) {
    return (int)syscall2(SYS_TIMERFD_GETTIME, (long)fd, (long)curr_value);
}

#endif /* _SYS_TIMERFD_H */

