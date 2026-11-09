/**
 * @file stdlib.h
 * @brief NullOS Standard Library General Utilities & Memory Management
 *
 * This header defines dynamic memory management routines (`malloc`, `free`,
 * `calloc`, `realloc`), process termination functions (`exit`, `abort`), and
 * numeric conversion algorithms (`abs`, `atoi`, `atol`, `strtol`, `itoa`).
 */

#ifndef _STDLIB_H
#define _STDLIB_H

#include <stddef.h>
#include <stdint.h>

/** Successful process termination status code. */
#define EXIT_SUCCESS 0

/** Unsuccessful process termination status code. */
#define EXIT_FAILURE 1

/* ============================================================================
 * Memory Allocation Prototypes
 * ============================================================================ */

/** Allocates size bytes of uninitialized memory from heap. */
void *malloc(size_t size);

/** Deallocates a memory block previously allocated on the heap. */
void  free(void *ptr);

/** Allocates memory for an array of nmemb elements and clears them to zero. */
void *calloc(size_t nmemb, size_t size);

/** Changes the size of a previously allocated heap memory block. */
void *realloc(void *ptr, size_t size);

/* ============================================================================
 * Process Termination Prototypes
 * ============================================================================ */

/** Terminates calling process normally with exit status code. */
void  exit(int status) __attribute__((noreturn));

/** Terminates calling process abnormally (raising SIGABRT status). */
void  abort(void) __attribute__((noreturn));

/* ============================================================================
 * Numeric Conversions & Utilities Prototypes
 * ============================================================================ */

/** Computes the absolute value of an integer. */
int   abs(int j);

/** Converts a decimal ASCII string to an int. */
int   atoi(const char *nptr);

/** Converts a decimal ASCII string to a long. */
long  atol(const char *nptr);

/** Converts an ASCII string to a long integer using the given radix base. */
long  strtol(const char *nptr, char **endptr, int base);

/** Converts an integer to a null-terminated string in the specified radix. */
char *itoa(int value, char *str, int base);

#endif /* _STDLIB_H */
