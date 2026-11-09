/**
 * @file threads.c
 * @brief NullOS Multi-Threaded POSIX Concurrency & Synchronization Test Program
 *
 * This test suite validates the integration and correctness of multi-threading
 * primitives in NullOS userspace:
 * 1. Thread Creation & Execution: Spawns two concurrent worker threads via `pthread_create`,
 *    allocating separate user stacks and invoking `SYS_CLONE`.
 * 2. Mutual Exclusion (Mutex): Tests atomicity of critical sections where both threads
 *    concurrently increment a global shared variable (`shared_counter`) under `pthread_mutex_lock`.
 * 3. Counting Semaphore (`sem_t`): Worker threads signal progress using `sem_post`; the main
 *    thread blocks in `sem_wait` until both threads complete their work.
 * 4. Condition Variables (`pthread_cond_t`): Thread #2 signals a condition variable; the main
 *    thread verifies atomic unlock-and-wait semantics via `pthread_cond_wait`.
 * 5. Thread Joining & Status Reaping: The main thread joins both workers via `pthread_join`,
 *    retrieving their respective exit codes and verifying the final state of the shared counter.
 */

#include <stdio.h>
#include <stdlib.h>
#include <pthread.h>
#include <semaphore.h>
#include <sys/syscall.h>

/** Global test mutex protecting critical section updates to shared_counter. */
static pthread_mutex_t test_mutex = PTHREAD_MUTEX_INITIALIZER;

/** Global condition variable used to synchronize thread completion signal. */
static pthread_cond_t test_cond = PTHREAD_COND_INITIALIZER;

/** Global counting semaphore tracking worker thread progress. */
static sem_t test_sem;

/** Shared counter variable updated concurrently by worker threads. */
static volatile int shared_counter = 0;

/** Flag indicating whether the condition variable has been signaled. */
static volatile int cond_signaled = 0;

/**
 * @brief Worker thread routine executed concurrently by Worker #1 and Worker #2.
 *
 * @param arg Pointer-sized integer identifier for the worker thread.
 * @return Thread return value passed through `pthread_exit`.
 */
void *worker_routine(void *arg) {
    long id = (long)arg;
    printf("[USER THREAD #%ld] Starting execution (self TID: %lu)\n", id, (unsigned long)pthread_self());

    /* 1. Test Mutex-protected critical section (50 iterations per thread) */
    for (int i = 0; i < 50; i++) {
        pthread_mutex_lock(&test_mutex);
        shared_counter++;
        pthread_mutex_unlock(&test_mutex);
    }

    /* 2. Test Semaphore signaling: post permit to main thread */
    sem_post(&test_sem);

    /* 3. Test Condition Variable: Worker #2 signals condition variable */
    if (id == 2) {
        pthread_mutex_lock(&test_mutex);
        cond_signaled = 1;
        printf("[USER THREAD #%ld] Signaling condition variable...\n", id);
        pthread_cond_signal(&test_cond);
        pthread_mutex_unlock(&test_mutex);
    }

    printf("[USER THREAD #%ld] Work completed, exiting cleanly\n", id);
    pthread_exit((void *)(id * 100));
}

/**
 * @brief Main coordinator routine spawning, synchronizing, and reaping worker threads.
 *
 * @param argc Argument count.
 * @param argv Argument vector.
 * @return 0 on complete test success, non-zero on failure.
 */
int main(int argc, char **argv) {
    (void)argc; (void)argv;
    printf("====================================================\n");
    printf("   NullOS Multi-Threaded POSIX Runtime Demo        \n");
    printf("====================================================\n");

    pthread_t t1, t2;
    sem_init(&test_sem, 0, 0);

    /* 1. Spawn Worker Thread #1 */
    printf("[MAIN] Spawning Worker Thread #1...\n");
    int ret1 = pthread_create(&t1, NULL, worker_routine, (void *)1L);
    if (ret1 != 0) {
        printf("[MAIN ERROR] Failed to create Thread #1!\n");
        return 1;
    }

    /* 2. Spawn Worker Thread #2 */
    printf("[MAIN] Spawning Worker Thread #2...\n");
    int ret2 = pthread_create(&t2, NULL, worker_routine, (void *)2L);
    if (ret2 != 0) {
        printf("[MAIN ERROR] Failed to create Thread #2!\n");
        return 1;
    }

    /* 3. Wait for semaphore posts from both threads */
    printf("[MAIN] Waiting on semaphore from workers...\n");
    sem_wait(&test_sem);
    sem_wait(&test_sem);
    printf("[MAIN] Both workers posted to semaphore successfully!\n");

    /* 4. Wait for condition variable signal from Worker #2 */
    pthread_mutex_lock(&test_mutex);
    while (!cond_signaled) {
        printf("[MAIN] Waiting on condition variable signal...\n");
        pthread_cond_wait(&test_cond, &test_mutex);
    }
    pthread_mutex_unlock(&test_mutex);
    printf("[MAIN] Condition variable received!\n");

    /* 5. Join both threads and verify exit return values */
    void *res1 = NULL;
    void *res2 = NULL;
    pthread_join(t1, &res1);
    pthread_join(t2, &res2);

    printf("[MAIN] Worker #1 reaped (result=%ld)\n", (long)res1);
    printf("[MAIN] Worker #2 reaped (result=%ld)\n", (long)res2);
    printf("[MAIN] Final shared counter: %d (expected 100)\n", shared_counter);

    /* 6. Verify shared counter arithmetic correctness */
    if (shared_counter == 100) {
        printf("[SUCCESS] All Multi-Threading, Mutex, Sem, and Cond tests PASSED!\n");
    } else {
        printf("[FAILURE] Counter mismatch: %d != 100\n", shared_counter);
        return 2;
    }

    return 0;
}
