#ifndef _STDLIB_H
#define _STDLIB_H

#include <stddef.h>
#include <stdint.h>

#define EXIT_SUCCESS 0
#define EXIT_FAILURE 1

/* Memory Allocation */
void *malloc(size_t size);
void  free(void *ptr);
void *calloc(size_t nmemb, size_t size);
void *realloc(void *ptr, size_t size);

/* Process Termination */
void  exit(int status) __attribute__((noreturn));
void  abort(void) __attribute__((noreturn));

/* Numeric Conversions & Utilities */
int   abs(int j);
int   atoi(const char *nptr);
long  atol(const char *nptr);
long  strtol(const char *nptr, char **endptr, int base);
char *itoa(int value, char *str, int base);

#endif /* _STDLIB_H */
