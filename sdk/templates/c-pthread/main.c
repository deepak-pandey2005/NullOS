#include <stdio.h>
#include <pthread.h>
#include <nullos/sdk.h>

static void *worker(void *arg) {
    (void)arg;
    printf("[THREAD] Worker running cleanly inside pthreads template!\n");
    return (void *)42;
}

int main(void) {
    pthread_t th;
    void *res = NULL;
    if (pthread_create(&th, NULL, worker, NULL) == 0) {
        pthread_join(th, &res);
    }
    printf("[MAIN] Joined worker result=%llu\n", (unsigned long long)(uintptr_t)res);
    return 0;
}

