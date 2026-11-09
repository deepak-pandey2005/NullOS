/**
 * @file pthread.h
 * @brief NullOS POSIX Threads (pthreads) Interface Definitions
 *
 * This header declares thread identifiers, attributes, mutex and condition variable
 * types, static initializer macros, and API prototypes for concurrent multi-threaded
 * execution under NullOS.
 */

#ifndef _PTHREAD_H
#define _PTHREAD_H

#include <stdint.h>
#include <stddef.h>

/** Unsigned integer type representing a unique thread ID within a process. */
typedef unsigned long pthread_t;

/**
 * @struct pthread_attr_t
 * @brief Thread creation attributes (e.g., custom stack address and size).
 */
typedef struct {
    size_t stack_size; /**< Size of user stack in bytes. */
    void  *stack_addr; /**< Base pointer of caller-provided stack memory. */
} pthread_attr_t;

/**
 * @struct pthread_mutex_t
 * @brief Tri-state futex-backed mutual exclusion lock.
 */
typedef struct {
    uint32_t  state;     /**< Lock state: 0 = unlocked, 1 = locked uncontended, 2 = locked contended. */
    uint32_t  recursive; /**< Recursive lock count. */
    pthread_t owner;     /**< Thread ID of current mutex owner. */
} pthread_mutex_t;

/** Static initializer macro for an unlocked mutex. */
#define PTHREAD_MUTEX_INITIALIZER { 0, 0, 0 }

/**
 * @struct pthread_cond_t
 * @brief Futex-backed condition variable object.
 */
typedef struct {
    uint32_t seq; /**< Monotonically increasing sequence number for futex wakeups. */
} pthread_cond_t;

/** Static initializer macro for a condition variable. */
#define PTHREAD_COND_INITIALIZER { 0 }

/* ============================================================================
 * Thread Management Prototypes
 * ============================================================================ */

/**
 * @brief Creates a new thread within the process.
 *
 * @param thread        Pointer where new thread ID is stored.
 * @param attr          Attributes (or NULL for default 64 KiB stack).
 * @param start_routine Thread entry point function pointer.
 * @param arg           Argument passed to start routine.
 * @return 0 on success, or non-zero error code.
 */
int pthread_create(pthread_t *thread, const pthread_attr_t *attr, void *(*start_routine)(void *), void *arg);

/**
 * @brief Terminates the calling thread.
 *
 * @param retval Return value made available to joining threads.
 */
void pthread_exit(void *retval) __attribute__((noreturn));

/**
 * @brief Suspends execution until the target thread terminates.
 *
 * @param thread Target thread identifier.
 * @param retval Location where thread return value is written.
 * @return 0 on success, or non-zero error code.
 */
int pthread_join(pthread_t thread, void **retval);

/**
 * @brief Obtains the thread ID of the calling thread.
 *
 * @return Thread identifier.
 */
pthread_t pthread_self(void);

/* ============================================================================
 * Mutex Primitives Prototypes
 * ============================================================================ */

/** Initializes a mutex object. */
int pthread_mutex_init(pthread_mutex_t *mutex, const void *attr);

/** Locks a mutex, blocking until available. */
int pthread_mutex_lock(pthread_mutex_t *mutex);

/** Attempts to lock a mutex without blocking. */
int pthread_mutex_trylock(pthread_mutex_t *mutex);

/** Unlocks a mutex. */
int pthread_mutex_unlock(pthread_mutex_t *mutex);

/** Destroys a mutex. */
int pthread_mutex_destroy(pthread_mutex_t *mutex);

/* ============================================================================
 * Condition Variable Primitives Prototypes
 * ============================================================================ */

/** Initializes a condition variable. */
int pthread_cond_init(pthread_cond_t *cond, const void *attr);

/** Waits on a condition variable, releasing the mutex atomically. */
int pthread_cond_wait(pthread_cond_t *cond, pthread_mutex_t *mutex);

/** Unblocks at least one thread waiting on condition variable. */
int pthread_cond_signal(pthread_cond_t *cond);

/** Unblocks all threads waiting on condition variable. */
int pthread_cond_broadcast(pthread_cond_t *cond);

/** Destroys a condition variable. */
int pthread_cond_destroy(pthread_cond_t *cond);

#endif /* _PTHREAD_H */
