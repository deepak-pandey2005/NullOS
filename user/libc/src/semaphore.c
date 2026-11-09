/**
 * @file semaphore.c
 * @brief NullOS POSIX Counting Semaphore Implementation via Fast Userspace Mutexes (Futex)
 *
 * This file implements POSIX counting semaphores (`sem_t`) for user processes and
 * threads in NullOS.
 *
 * ============================================================================
 * CONCURRENCY & FUTEX SYNCHRONIZATION ALGORITHM:
 * ============================================================================
 * The semaphore structure comprises:
 *   - `value`: An unsigned 32-bit integer tracking available resource permits.
 *   - `waiters`: An unsigned 32-bit counter of threads currently sleeping in kernel space.
 *
 * Fast path (uncontended):
 *   - `sem_wait`: An atomic compare-and-swap (`__atomic_compare_exchange_n`)
 *     checks if `value > 0`. If so, it atomically decrements the count to `value - 1`
 *     with Acquire-Release memory ordering without entering the kernel.
 *   - `sem_post`: Atomically increments `value` with Release ordering. If no
 *     waiters are present (`waiters == 0`), it completes without any system call.
 *
 * Slow path (contended):
 *   - When `value == 0`, `sem_wait` atomically increments `waiters` and suspends
 *     the calling thread in the kernel scheduler via `futex_wait(&sem->value, 0)`.
 *     Upon wakeup, `waiters` is decremented and the thread re-attempts the CAS loop.
 *   - In `sem_post`, if `waiters > 0`, it invokes `futex_wake(&sem->value, 1)` to
 *     wake up one sleeping thread from the kernel futex wait queue.
 */

#include <semaphore.h>
#include <sys/futex.h>

/**
 * @brief Initializes an unnamed semaphore object.
 *
 * Sets the initial resource permit count and resets the waiter count to zero.
 *
 * @param sem     Pointer to semaphore structure to initialize.
 * @param pshared Sharing indicator (pshared != 0 for cross-process, 0 for thread;
 *                NullOS futexes support both address spaces).
 * @param value   Initial non-negative semaphore permit count.
 * @return 0 on success, or -1 on invalid argument (NULL pointer).
 */
int sem_init(sem_t *sem, int pshared, unsigned int value) {
    (void)pshared;
    if (!sem) return -1;
    sem->value = value;
    sem->waiters = 0;
    return 0;
}

/**
 * @brief Locks (decrements) a semaphore, blocking if the count is zero.
 *
 * Performs a loop:
 * 1. Checks if `sem->value > 0`. If so, attempts an atomic compare-exchange to
 *    decrement the value. If successful, acquires the permit and returns immediately.
 * 2. If the count is 0, the thread records itself in `sem->waiters` and sleeps
 *    in the kernel scheduler using the `futex_wait` system call until a permit
 *    becomes available.
 *
 * @param sem Pointer to initialized semaphore.
 * @return 0 on success, or -1 on error.
 */
int sem_wait(sem_t *sem) {
    if (!sem) return -1;

    while (1) {
        uint32_t cur = sem->value;
        while (cur > 0) {
            if (__atomic_compare_exchange_n(&sem->value, &cur, cur - 1, 0, __ATOMIC_ACQ_REL, __ATOMIC_ACQUIRE)) {
                return 0;
            }
        }

        /* Value is 0: increment waiters count and suspend via futex system call */
        __atomic_add_fetch(&sem->waiters, 1, __ATOMIC_RELAXED);
        futex_wait(&sem->value, 0);
        __atomic_sub_fetch(&sem->waiters, 1, __ATOMIC_RELAXED);
    }
}

/**
 * @brief Attempts to decrement (lock) a semaphore without blocking.
 *
 * If the semaphore currently has permits (`value > 0`), attempts an atomic
 * compare-exchange. If the permit cannot be acquired immediately, returns -1
 * without suspending the calling thread.
 *
 * @param sem Pointer to initialized semaphore.
 * @return 0 if permit was acquired, or -1 if the semaphore was unavailable.
 */
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

/**
 * @brief Unlocks (increments) a semaphore.
 *
 * Atomically increments the semaphore permit count with Release memory ordering.
 * If one or more threads are waiting in the kernel futex queue (`waiters > 0`),
 * issues a `futex_wake` system call to unblock one waiting thread.
 *
 * @param sem Pointer to initialized semaphore.
 * @return 0 on success, or -1 on error.
 */
int sem_post(sem_t *sem) {
    if (!sem) return -1;
    __atomic_add_fetch(&sem->value, 1, __ATOMIC_RELEASE);
    if (sem->waiters > 0) {
        futex_wake(&sem->value, 1);
    }
    return 0;
}

/**
 * @brief Destroys an unnamed semaphore.
 *
 * Resets the permit count and waiters count to zero.
 *
 * @param sem Pointer to semaphore to destroy.
 * @return 0 on success, or -1 on error.
 */
int sem_destroy(sem_t *sem) {
    if (!sem) return -1;
    sem->value = 0;
    sem->waiters = 0;
    return 0;
}

/**
 * @brief Retrieves the current permit value of a semaphore.
 *
 * @param sem  Pointer to semaphore.
 * @param sval Pointer to integer where the current permit value will be written.
 * @return 0 on success, or -1 on invalid arguments.
 */
int sem_getvalue(sem_t *sem, int *sval) {
    if (!sem || !sval) return -1;
    *sval = (int)sem->value;
    return 0;
}
