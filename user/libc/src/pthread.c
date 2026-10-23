#include <pthread.h>
#include <sys/syscall.h>
#include <sys/futex.h>
#include <stdlib.h>

#define STACK_SIZE 65536

int pthread_create(pthread_t *thread, const pthread_attr_t *attr, void *(*start_routine)(void *), void *arg) {
    (void)attr;
    /* Allocate dedicated user stack via SYS_MMAP (PROT_READ | PROT_WRITE = 3) */
    long stack_addr = syscall4(SYS_MMAP, 0, STACK_SIZE, 3, 0);
    if (stack_addr == -1L || stack_addr == 0) {
        return -1;
    }

    uint64_t stack_top = (uint64_t)stack_addr + STACK_SIZE - 16;

    long tid = syscall6(SYS_CLONE, (long)start_routine, (long)stack_top, (long)arg, 0, 0, 0);
    if (tid <= 0) {
        return -1;
    }

    if (thread) {
        *thread = (pthread_t)tid;
    }
    return 0;
}

void pthread_exit(void *retval) {
    syscall1(SYS_THREAD_EXIT, (long)retval);
    while (1) {
        syscall0(SYS_YIELD);
    }
}

int pthread_join(pthread_t thread, void **retval) {
    int status = 0;
    long ret = syscall2(SYS_THREAD_JOIN, (long)thread, (long)&status);
    if (retval) {
        *retval = (void *)(long)status;
    }
    return (int)ret;
}

pthread_t pthread_self(void) {
    return (pthread_t)syscall0(SYS_GETTID);
}

int pthread_mutex_init(pthread_mutex_t *mutex, const void *attr) {
    (void)attr;
    if (!mutex) return -1;
    mutex->state = 0;
    mutex->recursive = 0;
    mutex->owner = 0;
    return 0;
}

int pthread_mutex_lock(pthread_mutex_t *mutex) {
    if (!mutex) return -1;

    /* Fast path: uncontended transition 0 -> 1 */
    uint32_t expected = 0;
    if (__atomic_compare_exchange_n(&mutex->state, &expected, 1, 0, __ATOMIC_ACQUIRE, __ATOMIC_RELAXED)) {
        mutex->owner = pthread_self();
        return 0;
    }

    /* Slow path: contended lock with futex sleep */
    while (1) {
        if (mutex->state == 2) {
            futex_wait(&mutex->state, 2);
        } else if (__atomic_exchange_n(&mutex->state, 2, __ATOMIC_ACQUIRE) == 0) {
            mutex->owner = pthread_self();
            return 0;
        } else {
            futex_wait(&mutex->state, 2);
        }

        expected = 0;
        if (__atomic_compare_exchange_n(&mutex->state, &expected, 2, 0, __ATOMIC_ACQUIRE, __ATOMIC_RELAXED)) {
            mutex->owner = pthread_self();
            return 0;
        }
    }
}

int pthread_mutex_trylock(pthread_mutex_t *mutex) {
    if (!mutex) return -1;
    uint32_t expected = 0;
    if (__atomic_compare_exchange_n(&mutex->state, &expected, 1, 0, __ATOMIC_ACQUIRE, __ATOMIC_RELAXED)) {
        mutex->owner = pthread_self();
        return 0;
    }
    return -1;
}

int pthread_mutex_unlock(pthread_mutex_t *mutex) {
    if (!mutex) return -1;
    mutex->owner = 0;
    uint32_t prev = __atomic_exchange_n(&mutex->state, 0, __ATOMIC_RELEASE);
    if (prev == 2) {
        /* Wake one waiting thread */
        futex_wake(&mutex->state, 1);
    }
    return 0;
}

int pthread_mutex_destroy(pthread_mutex_t *mutex) {
    if (!mutex) return -1;
    mutex->state = 0;
    return 0;
}

int pthread_cond_init(pthread_cond_t *cond, const void *attr) {
    (void)attr;
    if (!cond) return -1;
    cond->seq = 0;
    return 0;
}

int pthread_cond_wait(pthread_cond_t *cond, pthread_mutex_t *mutex) {
    if (!cond || !mutex) return -1;
    uint32_t cur_seq = cond->seq;
    pthread_mutex_unlock(mutex);
    futex_wait(&cond->seq, cur_seq);
    pthread_mutex_lock(mutex);
    return 0;
}

int pthread_cond_signal(pthread_cond_t *cond) {
    if (!cond) return -1;
    __atomic_add_fetch(&cond->seq, 1, __ATOMIC_RELEASE);
    futex_wake(&cond->seq, 1);
    return 0;
}

int pthread_cond_broadcast(pthread_cond_t *cond) {
    if (!cond) return -1;
    __atomic_add_fetch(&cond->seq, 1, __ATOMIC_RELEASE);
    futex_wake(&cond->seq, 1024);
    return 0;
}

int pthread_cond_destroy(pthread_cond_t *cond) {
    (void)cond;
    return 0;
}
