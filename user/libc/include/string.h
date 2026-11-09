/**
 * @file string.h
 * @brief NullOS Standard String and Raw Memory Manipulation Declarations
 *
 * This header defines prototypes for standard ISO C string inspection, copying,
 * concatenation, searching, lexicographic comparison, raw memory setting and copying,
 * and error string mapping (`strerror`).
 */

#ifndef _STRING_H
#define _STRING_H

#include <stddef.h>

/* ============================================================================
 * String Manipulation Prototypes
 * ============================================================================ */

/** Computes the byte length of a null-terminated string. */
size_t strlen(const char *s);

/** Copies source string including null terminator into destination buffer. */
char  *strcpy(char *dest, const char *src);

/** Copies up to n characters from source to destination buffer. */
char  *strncpy(char *dest, const char *src, size_t n);

/** Appends source string onto end of destination string. */
char  *strcat(char *dest, const char *src);

/** Appends up to n characters from source string onto end of destination. */
char  *strncat(char *dest, const char *src, size_t n);

/* ============================================================================
 * String Comparison Prototypes
 * ============================================================================ */

/** Lexicographically compares two null-terminated strings. */
int    strcmp(const char *s1, const char *s2);

/** Lexicographically compares up to n characters of two strings. */
int    strncmp(const char *s1, const char *s2, size_t n);

/* ============================================================================
 * String Searching Prototypes
 * ============================================================================ */

/** Finds first occurrence of character c in string s. */
char  *strchr(const char *s, int c);

/** Finds last occurrence of character c in string s. */
char  *strrchr(const char *s, int c);

/** Locates first occurrence of substring needle in string haystack. */
char  *strstr(const char *haystack, const char *needle);

/** Duplicates string s by dynamically allocating memory via malloc(). */
char  *strdup(const char *s);

/* ============================================================================
 * Raw Memory Operations Prototypes
 * ============================================================================ */

/** Fills first n bytes of memory area s with constant byte c. */
void  *memset(void *s, int c, size_t n);

/** Copies n bytes between non-overlapping memory areas. */
void  *memcpy(void *dest, const void *src, size_t n);

/** Copies n bytes between memory areas that may safely overlap. */
void  *memmove(void *dest, const void *src, size_t n);

/** Compares first n bytes of two memory areas. */
int    memcmp(const void *s1, const void *s2, size_t n);

/** Scans first n bytes of memory for first occurrence of byte c. */
void  *memchr(const void *s, int c, size_t n);

/* ============================================================================
 * Diagnostics Prototypes
 * ============================================================================ */

/** Returns human-readable error string corresponding to errno code errnum. */
char  *strerror(int errnum);

#endif /* _STRING_H */
