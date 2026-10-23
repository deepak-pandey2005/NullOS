#ifndef _SYS_FUTEX_H
#define _SYS_FUTEX_H

#include <stdint.h>
#include <sys/syscall.h>

#define FUTEX_WAIT          0
#define FUTEX_WAKE          1
#define FUTEX_REQUEUE       3
#define FUTEX_PRIVATE_FLAG  128

static inline long futex_wait(uint32_t *uaddr, uint32_t val) {
    return syscall4(SYS_FUTEX, (long)uaddr, FUTEX_WAIT, val, 0);
}

static inline long futex_wake(uint32_t *uaddr, uint32_t count) {
    return syscall4(SYS_FUTEX, (long)uaddr, FUTEX_WAKE, count, 0);
}

static inline long futex_requeue(uint32_t *uaddr1, uint32_t count, uint32_t *uaddr2, uint32_t req_count) {
    return syscall6(SYS_FUTEX, (long)uaddr1, FUTEX_REQUEUE, count, (long)uaddr2, req_count, 0);
}

#endif /* _SYS_FUTEX_H */
