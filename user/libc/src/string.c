/**
 * @file string.c
 * @brief NullOS Standard C Library String & Memory Operations
 *
 * This file implements the standard ISO C string and raw memory manipulation
 * functions for NullOS userspace programs. It includes null-terminated string
 * length, copying, concatenation, comparison, searching, and duplicating, as well
 * as block memory setting, copying (with overlapping region handling in `memmove`),
 * comparison, searching, and error number translation (`strerror`).
 */

#include <string.h>
#include <stdlib.h>
#include <stdint.h>

/**
 * @brief Computes the length of a null-terminated string.
 *
 * Iterates through memory until the terminating null byte ('\0') is found.
 * Does not include the null byte itself in the count.
 *
 * @param s Pointer to null-terminated string.
 * @return Number of characters preceding the terminating null character.
 */
size_t strlen(const char *s) {
    size_t len = 0;
    while (s[len] != '\0') {
        len++;
    }
    return len;
}

/**
 * @brief Copies a null-terminated string from source to destination.
 *
 * Copies the string pointed to by `src`, including the terminating null character,
 * to the buffer pointed to by `dest`. The destination array must be large enough
 * to receive the copy.
 *
 * @param dest Pointer to destination buffer.
 * @param src  Pointer to source null-terminated string.
 * @return Pointer to destination buffer (`dest`).
 */
char *strcpy(char *dest, const char *src) {
    char *orig = dest;
    while ((*dest++ = *src++) != '\0') {
        /* Copy loop copies bytes including the trailing null terminator */
    }
    return orig;
}

/**
 * @brief Copies up to `n` characters from source string to destination buffer.
 *
 * If `src` has fewer than `n` characters, the remainder of `dest` is padded with
 * null bytes. If `src` has `n` or more characters, `dest` is NOT null-terminated.
 *
 * @param dest Pointer to destination buffer.
 * @param src  Pointer to source string.
 * @param n    Maximum number of bytes to copy.
 * @return Pointer to destination buffer (`dest`).
 */
char *strncpy(char *dest, const char *src, size_t n) {
    char *orig = dest;
    size_t i = 0;
    for (; i < n && src[i] != '\0'; i++) {
        dest[i] = src[i];
    }
    for (; i < n; i++) {
        dest[i] = '\0';
    }
    return orig;
}

/**
 * @brief Appends the source string to the destination string.
 *
 * Overwrites the null byte at the end of `dest`, appends characters from `src`,
 * and adds a terminating null byte. The caller must ensure `dest` has sufficient
 * capacity.
 *
 * @param dest Pointer to destination null-terminated string.
 * @param src  Pointer to source string to append.
 * @return Pointer to destination string (`dest`).
 */
char *strcat(char *dest, const char *src) {
    char *orig = dest;
    while (*dest != '\0') {
        dest++;
    }
    while ((*dest++ = *src++) != '\0') {
        /* copy src characters */
    }
    return orig;
}

/**
 * @brief Appends at most `n` characters from source to destination string.
 *
 * Appends up to `n` bytes from `src` to `dest`, and always appends a terminating
 * null byte ('\0') even if the copy was truncated.
 *
 * @param dest Pointer to destination null-terminated string.
 * @param src  Pointer to source string.
 * @param n    Maximum number of characters to append.
 * @return Pointer to destination string (`dest`).
 */
char *strncat(char *dest, const char *src, size_t n) {
    char *orig = dest;
    while (*dest != '\0') {
        dest++;
    }
    size_t i = 0;
    while (i < n && src[i] != '\0') {
        dest[i] = src[i];
        i++;
    }
    dest[i] = '\0';
    return orig;
}

/**
 * @brief Lexicographically compares two null-terminated strings.
 *
 * Compares bytes as unsigned characters so that \200 is greater than \0.
 *
 * @param s1 First string.
 * @param s2 Second string.
 * @return < 0 if s1 < s2, 0 if s1 == s2, > 0 if s1 > s2.
 */
int strcmp(const char *s1, const char *s2) {
    while (*s1 && (*s1 == *s2)) {
        s1++;
        s2++;
    }
    return *(const unsigned char *)s1 - *(const unsigned char *)s2;
}

/**
 * @brief Lexicographically compares up to `n` characters of two strings.
 *
 * @param s1 First string.
 * @param s2 Second string.
 * @param n  Maximum number of characters to compare.
 * @return < 0 if s1 < s2, 0 if s1 == s2, > 0 if s1 > s2.
 */
int strncmp(const char *s1, const char *s2, size_t n) {
    for (size_t i = 0; i < n; i++) {
        if (s1[i] != s2[i] || s1[i] == '\0') {
            return (unsigned char)s1[i] - (unsigned char)s2[i];
        }
    }
    return 0;
}

/**
 * @brief Locates the first occurrence of a character in a string.
 *
 * The terminating null character is considered part of the string; therefore,
 * searching for '\0' will return a pointer to the terminating null byte.
 *
 * @param s The string to search.
 * @param c The character to search for (interpreted as char).
 * @return Pointer to matching character in string, or NULL if not found.
 */
char *strchr(const char *s, int c) {
    char ch = (char)c;
    while (*s != '\0') {
        if (*s == ch) {
            return (char *)s;
        }
        s++;
    }
    return (ch == '\0') ? (char *)s : NULL;
}

/**
 * @brief Locates the last occurrence of a character in a string.
 *
 * Searches backward from the terminating null character.
 *
 * @param s The string to search.
 * @param c The character to search for.
 * @return Pointer to last occurrence of character in string, or NULL if not found.
 */
char *strrchr(const char *s, int c) {
    char ch = (char)c;
    const char *last = NULL;
    do {
        if (*s == ch) {
            last = s;
        }
    } while (*s++);
    return (char *)last;
}

/**
 * @brief Locates a substring within a string.
 *
 * Finds the first occurrence of the substring `needle` in the string `haystack`.
 * The terminating null bytes are not compared.
 *
 * @param haystack The enclosing string to search within.
 * @param needle   The target substring to search for.
 * @return Pointer to the beginning of the located substring, or NULL if not found.
 *         If needle is empty, returns haystack.
 */
char *strstr(const char *haystack, const char *needle) {
    if (!*needle) {
        return (char *)haystack;
    }
    for (; *haystack != '\0'; haystack++) {
        if (*haystack == *needle) {
            const char *h = haystack;
            const char *n = needle;
            while (*h && *n && *h == *n) {
                h++;
                n++;
            }
            if (!*n) {
                return (char *)haystack;
            }
        }
    }
    return NULL;
}

/**
 * @brief Duplicates a string by dynamically allocating memory via malloc().
 *
 * Calculates the length of `s`, allocates `len + 1` bytes via `malloc`, copies
 * the string into the new memory buffer, and returns it.
 *
 * @param s The source string to duplicate.
 * @return Pointer to dynamically allocated duplicated string, or NULL on allocation failure.
 */
char *strdup(const char *s) {
    size_t len = strlen(s) + 1;
    char *dup = (char *)malloc(len);
    if (dup) {
        memcpy(dup, s, len);
    }
    return dup;
}

/**
 * @brief Fills the first `n` bytes of the memory area with the constant byte `c`.
 *
 * @param s Memory area to fill.
 * @param c Constant byte value.
 * @param n Number of bytes to fill.
 * @return Pointer to the memory area `s`.
 */
void *memset(void *s, int c, size_t n) {
    unsigned char *p = (unsigned char *)s;
    unsigned char val = (unsigned char)c;
    for (size_t i = 0; i < n; i++) {
        p[i] = val;
    }
    return s;
}

/**
 * @brief Copies `n` bytes between non-overlapping memory areas.
 *
 * @param dest Destination memory pointer.
 * @param src  Source memory pointer.
 * @param n    Number of bytes to copy.
 * @return Pointer to destination (`dest`).
 */
void *memcpy(void *dest, const void *src, size_t n) {
    unsigned char *d = (unsigned char *)dest;
    const unsigned char *s = (const unsigned char *)src;
    for (size_t i = 0; i < n; i++) {
        d[i] = s[i];
    }
    return dest;
}

/**
 * @brief Copies `n` bytes between memory areas that may overlap safely.
 *
 * If the destination address is lower than source address, copies forwards
 * from byte 0 to n-1. If destination is higher than source address, copies backwards
 * from byte n-1 down to 0 to prevent overwriting unread source data.
 *
 * @param dest Destination memory buffer.
 * @param src  Source memory buffer.
 * @param n    Number of bytes to copy.
 * @return Pointer to destination (`dest`).
 */
void *memmove(void *dest, const void *src, size_t n) {
    unsigned char *d = (unsigned char *)dest;
    const unsigned char *s = (const unsigned char *)src;
    if (d < s) {
        /* Forward copy */
        for (size_t i = 0; i < n; i++) {
            d[i] = s[i];
        }
    } else if (d > s) {
        /* Backward copy to avoid clobbering overlapping region */
        for (size_t i = n; i > 0; i--) {
            d[i - 1] = s[i - 1];
        }
    }
    return dest;
}

/**
 * @brief Compares the first `n` bytes of two memory areas.
 *
 * Bytes are interpreted as unsigned char.
 *
 * @param s1 First memory block.
 * @param s2 Second memory block.
 * @param n  Number of bytes to compare.
 * @return < 0 if s1 < s2, 0 if s1 == s2, > 0 if s1 > s2.
 */
int memcmp(const void *s1, const void *s2, size_t n) {
    const unsigned char *p1 = (const unsigned char *)s1;
    const unsigned char *p2 = (const unsigned char *)s2;
    for (size_t i = 0; i < n; i++) {
        if (p1[i] != p2[i]) {
            return p1[i] - p2[i];
        }
    }
    return 0;
}

/**
 * @brief Scans the first `n` bytes of memory for the first occurrence of byte `c`.
 *
 * @param s Memory block to scan.
 * @param c Target byte value (interpreted as unsigned char).
 * @param n Number of bytes to scan.
 * @return Pointer to matching byte, or NULL if byte does not occur within `n` bytes.
 */
void *memchr(const void *s, int c, size_t n) {
    const unsigned char *p = (const unsigned char *)s;
    unsigned char val = (unsigned char)c;
    for (size_t i = 0; i < n; i++) {
        if (p[i] == val) {
            return (void *)(p + i);
        }
    }
    return NULL;
}

/**
 * @brief Returns a textual description of an error number.
 *
 * Maps standard POSIX errno codes to human-readable English strings.
 *
 * @param errnum The error number (e.g., EPERM, ENOENT, EIO, EBADF, ENOMEM).
 * @return Constant string describing the error.
 */
char *strerror(int errnum) {
    switch (errnum) {
        case 0:  return "Success";
        case 1:  return "Operation not permitted";
        case 2:  return "No such file or directory";
        case 5:  return "Input/output error";
        case 9:  return "Bad file descriptor";
        case 12: return "Cannot allocate memory";
        case 13: return "Permission denied";
        case 14: return "Bad address";
        case 21: return "Is a directory";
        case 22: return "Invalid argument";
        default: return "Unknown error";
    }
}
