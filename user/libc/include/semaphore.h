/**
 * @file semaphore.h
 * @brief NullOS POSIX Counting Semaphore Interface Definitions
 *
 * This header declares the `sem_t` counting semaphore structure and API prototypes
 * for thread and process synchronization using kernel futexes.
 */

#ifndef _SEMAPHORE_H
#define _SEMAPHORE_H

#include <stdint.h>

/**
 * @struct sem_t
 * @brief POSIX counting semaphore synchronization object.
 */
typedef struct {
    uint32_t value;   /**< Available resource permits count. */
    uint32_t waiters; /**< Number of threads currently sleeping on futex wait queue. */
} sem_t;

/**
 * @brief Initializes an unnamed semaphore object.
 *
 * @param sem     Pointer to semaphore structure.
 * @param pshared Shared between processes if non-zero; thread-private if zero.
 * @param value   Initial permit count.
 * @return 0 on success, or -1 on error.
 */
int sem_init(sem_t *sem, int pshared, unsigned int value);

/**
 * @brief Locks (decrements) a semaphore, blocking if permits count is 0.
 *
 * @param sem Pointer to initialized semaphore.
 * @return 0 on success, or -1 on error.
 */
int sem_wait(sem_t *sem);

/**
 * @brief Tries to lock (decrement) a semaphore without blocking.
 *
 * @param sem Pointer to initialized semaphore.
 * @return 0 on success, or -1 if no permits were available.
 */
int sem_trywait(sem_t *sem);

/**
 * @brief Unlocks (increments) a semaphore, waking up any blocked thread.
 *
 * @param sem Pointer to initialized semaphore.
 * @return 0 on success, or -1 on error.
 */
int sem_post(sem_t *sem);

/**
 * @brief Destroys an unnamed semaphore.
 *
 * @param sem Pointer to semaphore.
 * @return 0 on success, or -1 on error.
 */
int sem_destroy(sem_t *sem);

/**
 * @brief Retrieves the current permit value of the semaphore.
 *
 * @param sem  Pointer to semaphore.
 * @param sval Pointer to integer where current value will be placed.
 * @return 0 on success, or -1 on error.
 */
int sem_getvalue(sem_t *sem, int *sval);

#endif /* _SEMAPHORE_H */
