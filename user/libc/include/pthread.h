#ifndef _PTHREAD_H
#define _PTHREAD_H

#include <stdint.h>
#include <stddef.h>

typedef unsigned long pthread_t;

typedef struct {
    size_t stack_size;
    void *stack_addr;
} pthread_attr_t;

typedef struct {
    uint32_t state;     /* 0 = unlocked, 1 = locked uncontended, 2 = locked contended */
    uint32_t recursive;
    pthread_t owner;
} pthread_mutex_t;

#define PTHREAD_MUTEX_INITIALIZER { 0, 0, 0 }

typedef struct {
    uint32_t seq;
} pthread_cond_t;

#define PTHREAD_COND_INITIALIZER { 0 }

/* Thread Management */
int pthread_create(pthread_t *thread, const pthread_attr_t *attr, void *(*start_routine)(void *), void *arg);
void pthread_exit(void *retval) __attribute__((noreturn));
int pthread_join(pthread_t thread, void **retval);
pthread_t pthread_self(void);

/* Mutex Primitives */
int pthread_mutex_init(pthread_mutex_t *mutex, const void *attr);
int pthread_mutex_lock(pthread_mutex_t *mutex);
int pthread_mutex_trylock(pthread_mutex_t *mutex);
int pthread_mutex_unlock(pthread_mutex_t *mutex);
int pthread_mutex_destroy(pthread_mutex_t *mutex);

/* Condition Variable Primitives */
int pthread_cond_init(pthread_cond_t *cond, const void *attr);
int pthread_cond_wait(pthread_cond_t *cond, pthread_mutex_t *mutex);
int pthread_cond_signal(pthread_cond_t *cond);
int pthread_cond_broadcast(pthread_cond_t *cond);
int pthread_cond_destroy(pthread_cond_t *cond);

#endif /* _PTHREAD_H */
