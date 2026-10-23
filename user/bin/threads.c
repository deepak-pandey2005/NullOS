#include <stdio.h>
#include <stdlib.h>
#include <pthread.h>
#include <semaphore.h>
#include <sys/syscall.h>

static pthread_mutex_t test_mutex = PTHREAD_MUTEX_INITIALIZER;
static pthread_cond_t test_cond = PTHREAD_COND_INITIALIZER;
static sem_t test_sem;

static volatile int shared_counter = 0;
static volatile int cond_signaled = 0;

void *worker_routine(void *arg) {
    long id = (long)arg;
    printf("[USER THREAD #%ld] Starting execution (self TID: %lu)\n", id, (unsigned long)pthread_self());

    /* 1. Test Mutex-protected critical section */
    for (int i = 0; i < 50; i++) {
        pthread_mutex_lock(&test_mutex);
        shared_counter++;
        pthread_mutex_unlock(&test_mutex);
    }

    /* 2. Test Semaphore signaling */
    sem_post(&test_sem);

    /* 3. Test Condition Variable */
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

int main(int argc, char **argv) {
    (void)argc; (void)argv;
    printf("====================================================\n");
    printf("   NullOS Multi-Threaded POSIX Runtime Demo        \n");
    printf("====================================================\n");

    pthread_t t1, t2;
    sem_init(&test_sem, 0, 0);

    printf("[MAIN] Spawning Worker Thread #1...\n");
    int ret1 = pthread_create(&t1, NULL, worker_routine, (void *)1L);
    if (ret1 != 0) {
        printf("[MAIN ERROR] Failed to create Thread #1!\n");
        return 1;
    }

    printf("[MAIN] Spawning Worker Thread #2...\n");
    int ret2 = pthread_create(&t2, NULL, worker_routine, (void *)2L);
    if (ret2 != 0) {
        printf("[MAIN ERROR] Failed to create Thread #2!\n");
        return 1;
    }

    /* Wait for semaphore posts from both threads */
    printf("[MAIN] Waiting on semaphore from workers...\n");
    sem_wait(&test_sem);
    sem_wait(&test_sem);
    printf("[MAIN] Both workers posted to semaphore successfully!\n");

    /* Wait for condition variable */
    pthread_mutex_lock(&test_mutex);
    while (!cond_signaled) {
        printf("[MAIN] Waiting on condition variable signal...\n");
        pthread_cond_wait(&test_cond, &test_mutex);
    }
    pthread_mutex_unlock(&test_mutex);
    printf("[MAIN] Condition variable received!\n");

    /* Join both threads */
    void *res1 = NULL;
    void *res2 = NULL;
    pthread_join(t1, &res1);
    pthread_join(t2, &res2);

    printf("[MAIN] Worker #1 reaped (result=%ld)\n", (long)res1);
    printf("[MAIN] Worker #2 reaped (result=%ld)\n", (long)res2);
    printf("[MAIN] Final shared counter: %d (expected 100)\n", shared_counter);

    if (shared_counter == 100) {
        printf("[SUCCESS] All Multi-Threading, Mutex, Sem, and Cond tests PASSED!\n");
    } else {
        printf("[FAILURE] Counter mismatch: %d != 100\n", shared_counter);
        return 2;
    }

    return 0;
}
