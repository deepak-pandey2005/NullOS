#include <semaphore.h>
#include <sys/futex.h>

int sem_init(sem_t *sem, int pshared, unsigned int value) {
    (void)pshared;
    if (!sem) return -1;
    sem->value = value;
    sem->waiters = 0;
    return 0;
}

int sem_wait(sem_t *sem) {
    if (!sem) return -1;

    while (1) {
        uint32_t cur = sem->value;
        while (cur > 0) {
            if (__atomic_compare_exchange_n(&sem->value, &cur, cur - 1, 0, __ATOMIC_ACQ_REL, __ATOMIC_ACQUIRE)) {
                return 0;
            }
        }

        /* Value is 0, must wait via futex */
        __atomic_add_fetch(&sem->waiters, 1, __ATOMIC_RELAXED);
        futex_wait(&sem->value, 0);
        __atomic_sub_fetch(&sem->waiters, 1, __ATOMIC_RELAXED);
    }
}

int sem_trywait(sem_t *sem) {
    if (!sem) return -1;
    uint32_t cur = sem->value;
    while (cur > 0) {
        if (__atomic_compare_exchange_n(&sem->value, &cur, cur - 1, 0, __ATOMIC_ACQ_REL, __ATOMIC_ACQUIRE)) {
            return 0;
        }
    }
    return -1;
}

int sem_post(sem_t *sem) {
    if (!sem) return -1;
    __atomic_add_fetch(&sem->value, 1, __ATOMIC_RELEASE);
    if (sem->waiters > 0) {
        futex_wake(&sem->value, 1);
    }
    return 0;
}

int sem_destroy(sem_t *sem) {
    if (!sem) return -1;
    sem->value = 0;
    sem->waiters = 0;
    return 0;
}

int sem_getvalue(sem_t *sem, int *sval) {
    if (!sem || !sval) return -1;
    *sval = (int)sem->value;
    return 0;
}
