/**
 * @file pthread.c
 * @brief NullOS POSIX Threads (pthreads) Implementation
 *
 * This file implements the core POSIX Thread API for NullOS userspace:
 * - Thread lifecycle management: `pthread_create`, `pthread_exit`, `pthread_join`,
 *   and `pthread_self`.
 * - Mutual exclusion locks (Mutexes): `pthread_mutex_init`, `pthread_mutex_lock`,
 *   `pthread_mutex_trylock`, `pthread_mutex_unlock`, and `pthread_mutex_destroy`.
 * - Condition variables: `pthread_cond_init`, `pthread_cond_wait`, `pthread_cond_signal`,
 *   `pthread_cond_broadcast`, and `pthread_cond_destroy`.
 *
 * ============================================================================
 * SYNCHRONIZATION & THREADING MODEL:
 * ============================================================================
 * 1. Thread Creation:
 *    - Each new pthread receives its own private 64 KiB user stack allocated via
 *      the `SYS_MMAP` system call with `PROT_READ | PROT_WRITE`.
 *    - Invokes `SYS_CLONE` to create a lightweight execution context (thread)
 *      sharing the parent process's virtual memory address space, file descriptor
 *      table, and signal handlers.
 *
 * 2. Tri-State Futex Mutex:
 *    - `state == 0`: Unlocked.
 *    - `state == 1`: Locked without contention (no waiting threads).
 *    - `state == 2`: Locked with contention (at least one thread sleeping in kernel).
 *    - Fast-path lock: Atomic CAS 0 -> 1 with Acquire semantics. If successful,
 *      no system call is issued.
 *    - Slow-path lock: If already locked, transitions state to 2 (contended) and
 *      invokes `futex_wait(&mutex->state, 2)`.
 *    - Fast-path unlock: Atomic exchange state -> 0 with Release semantics. If the
 *      previous state was 2, invokes `futex_wake(&mutex->state, 1)` to wake one waiter.
 *
 * 3. Futex Condition Variable:
 *    - Uses a monotonically increasing sequence counter `cond->seq`.
 *    - `pthread_cond_wait` snapshots the current sequence number, unlocks the associated
 *      mutex, enters `futex_wait(&cond->seq, cur_seq)`, and re-acquires the mutex
 *      upon wakeup.
 *    - `pthread_cond_signal` atomically increments `seq` and wakes 1 sleeping waiter.
 *    - `pthread_cond_broadcast` atomically increments `seq` and wakes up all sleeping waiters.
 */

#include <pthread.h>
#include <sys/syscall.h>
#include <sys/futex.h>
#include <stdlib.h>

/** Default stack size allocated for newly spawned POSIX threads (64 KiB). */
#define STACK_SIZE 65536

/**
 * @brief Creates a new thread within the calling process.
 *
 * Allocates a dedicated 64 KiB user stack via `SYS_MMAP`, calculates the 16-byte
 * aligned stack top, and invokes the `SYS_CLONE` (56) system call with the start
 * routine and argument pointer.
 *
 * @param thread        Pointer where the new thread ID (pthread_t) will be stored.
 * @param attr          Thread attributes (e.g., custom stack; currently NULL for defaults).
 * @param start_routine Function pointer invoked by the newly created thread.
 * @param arg           Single argument passed to `start_routine`.
 * @return 0 on success, or -1 on allocation/clone failure.
 */
int pthread_create(pthread_t *thread, const pthread_attr_t *attr, void *(*start_routine)(void *), void *arg) {
    (void)attr;
    /* Allocate dedicated user stack via SYS_MMAP (PROT_READ | PROT_WRITE = 3) */
    long stack_addr = syscall4(SYS_MMAP, 0, STACK_SIZE, 3, 0);
    if (stack_addr == -1L || stack_addr == 0) {
        return -1;
    }

    /* Stack grows downwards on x86_64; calculate stack top aligned to 16 bytes */
    uint64_t stack_top = (uint64_t)stack_addr + STACK_SIZE - 16;

    /*
     * Invoke kernel clone syscall:
     * - arg1: Entry point function pointer (start_routine)
     * - arg2: Initial user stack pointer (stack_top)
     * - arg3: Argument passed to start routine (arg)
     */
    long tid = syscall6(SYS_CLONE, (long)start_routine, (long)stack_top, (long)arg, 0, 0, 0);
    if (tid <= 0) {
        return -1;
    }

    if (thread) {
        *thread = (pthread_t)tid;
    }
    return 0;
}

/**
 * @brief Terminates the calling thread.
 *
 * Stores the return value and invokes the `SYS_THREAD_EXIT` (57) system call.
 * The kernel cleans up the thread's execution context and notifies any thread
 * blocked in `pthread_join()`. This function never returns.
 *
 * @param retval Return value made available to any successful join call.
 */
void pthread_exit(void *retval) {
    syscall1(SYS_THREAD_EXIT, (long)retval);
    while (1) {
        syscall0(SYS_YIELD);
    }
}

/**
 * @brief Waits for thread termination and reaps its return status.
 *
 * Invokes the `SYS_THREAD_JOIN` (58) system call. Suspends the calling thread
 * until the target thread `thread` has terminated.
 *
 * @param thread Target thread identifier to wait for.
 * @param retval If non-NULL, receives the exit status returned by target thread.
 * @return 0 on success, or negative errno on failure.
 */
int pthread_join(pthread_t thread, void **retval) {
    int status = 0;
    long ret = syscall2(SYS_THREAD_JOIN, (long)thread, (long)&status);
    if (retval) {
        *retval = (void *)(long)status;
    }
    return (int)ret;
}

/**
 * @brief Obtains the thread identifier of the calling thread.
 *
 * Invokes the `SYS_GETTID` (186) system call.
 *
 * @return The thread ID (pthread_t) of the caller.
 */
pthread_t pthread_self(void) {
    return (pthread_t)syscall0(SYS_GETTID);
}

/**
 * @brief Initializes a mutual exclusion lock object.
 *
 * Resets lock state to 0 (unlocked), recursive counter to 0, and owner to 0.
 *
 * @param mutex Pointer to mutex object to initialize.
 * @param attr  Mutex attributes (unused).
 * @return 0 on success, or -1 on NULL pointer.
 */
int pthread_mutex_init(pthread_mutex_t *mutex, const void *attr) {
    (void)attr;
    if (!mutex) return -1;
    mutex->state = 0;
    mutex->recursive = 0;
    mutex->owner = 0;
    return 0;
}

/**
 * @brief Locks a mutual exclusion lock, blocking if already locked.
 *
 * Uses the standard tri-state futex pattern:
 * - Fast path: Tries atomic compare-and-swap 0 -> 1. If successful, caller
 *   acquires the lock without entering kernel space.
 * - Slow path: If the lock was already held, atomically sets the state to 2
 *   (contended) and suspends in the kernel via `futex_wait`. Upon waking,
 *   attempts to acquire the lock again.
 *
 * @param mutex Pointer to initialized mutex.
 * @return 0 on success, or -1 on NULL pointer.
 */
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

/**
 * @brief Attempts to lock a mutex without blocking.
 *
 * Performs an atomic compare-and-swap 0 -> 1. If uncontended, acquires the lock
 * and returns 0. If already locked, returns -1 immediately without sleeping.
 *
 * @param mutex Pointer to mutex object.
 * @return 0 if lock was acquired, or -1 if lock was already held.
 */
int pthread_mutex_trylock(pthread_mutex_t *mutex) {
    if (!mutex) return -1;
    uint32_t expected = 0;
    if (__atomic_compare_exchange_n(&mutex->state, &expected, 1, 0, __ATOMIC_ACQUIRE, __ATOMIC_RELAXED)) {
        mutex->owner = pthread_self();
        return 0;
    }
    return -1;
}

/**
 * @brief Unlocks a mutual exclusion lock.
 *
 * Clears the owner ID and atomically exchanges `state` to 0 with Release semantics.
 * If the previous state was 2 (indicating threads were waiting), issues a
 * `futex_wake` system call to awaken one waiting thread.
 *
 * @param mutex Pointer to locked mutex.
 * @return 0 on success, or -1 on NULL pointer.
 */
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

/**
 * @brief Destroys a mutual exclusion lock.
 *
 * Resets lock state to 0.
 *
 * @param mutex Pointer to mutex to destroy.
 * @return 0 on success, or -1 on NULL pointer.
 */
int pthread_mutex_destroy(pthread_mutex_t *mutex) {
    if (!mutex) return -1;
    mutex->state = 0;
    return 0;
}

/**
 * @brief Initializes a condition variable object.
 *
 * Resets sequence number to 0.
 *
 * @param cond Pointer to condition variable structure.
 * @param attr Attributes (unused).
 * @return 0 on success, or -1 on NULL pointer.
 */
int pthread_cond_init(pthread_cond_t *cond, const void *attr) {
    (void)attr;
    if (!cond) return -1;
    cond->seq = 0;
    return 0;
}

/**
 * @brief Blocks on a condition variable until signaled.
 *
 * Atomically releases the associated mutex and suspends the calling thread
 * via `futex_wait(&cond->seq, cur_seq)`. Upon waking, re-locks the mutex
 * before returning to caller.
 *
 * @param cond  Pointer to condition variable.
 * @param mutex Pointer to associated locked mutex.
 * @return 0 on success, or -1 on invalid argument.
 */
int pthread_cond_wait(pthread_cond_t *cond, pthread_mutex_t *mutex) {
    if (!cond || !mutex) return -1;
    uint32_t cur_seq = cond->seq;
    pthread_mutex_unlock(mutex);
    futex_wait(&cond->seq, cur_seq);
    pthread_mutex_lock(mutex);
    return 0;
}

/**
 * @brief Unblocks at least one thread waiting on the condition variable.
 *
 * Atomically increments `cond->seq` and issues `futex_wake(&cond->seq, 1)`
 * to unblock one waiting thread.
 *
 * @param cond Pointer to condition variable.
 * @return 0 on success, or -1 on invalid argument.
 */
int pthread_cond_signal(pthread_cond_t *cond) {
    if (!cond) return -1;
    __atomic_add_fetch(&cond->seq, 1, __ATOMIC_RELEASE);
    futex_wake(&cond->seq, 1);
    return 0;
}

/**
 * @brief Unblocks all threads currently waiting on the condition variable.
 *
 * Atomically increments `cond->seq` and issues `futex_wake(&cond->seq, 1024)`
 * to awaken all threads sleeping on the condition queue.
 *
 * @param cond Pointer to condition variable.
 * @return 0 on success, or -1 on invalid argument.
 */
int pthread_cond_broadcast(pthread_cond_t *cond) {
    if (!cond) return -1;
    __atomic_add_fetch(&cond->seq, 1, __ATOMIC_RELEASE);
    futex_wake(&cond->seq, 1024);
    return 0;
}

/**
 * @brief Destroys a condition variable.
 *
 * @param cond Pointer to condition variable.
 * @return 0 on success.
 */
int pthread_cond_destroy(pthread_cond_t *cond) {
    (void)cond;
    return 0;
}
