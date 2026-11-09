/**
 * @file stdio.h
 * @brief NullOS Standard Input/Output Stream Definitions & Declarations
 *
 * This header defines the `FILE` stream structure, standard stream handles
 * (`stdin`, `stdout`, `stderr`), EOF marker, and formatted/character I/O functions.
 */

#ifndef _STDIO_H
#define _STDIO_H

#include <stddef.h>
#include <stdint.h>
#include <sys/types.h>

/** End-of-file indicator constant (-1). */
#define EOF (-1)

/**
 * @struct FILE
 * @brief Stream handle structure managing buffered and unbuffered file I/O.
 */
typedef struct FILE {
    int   fd;    /**< Underlying operating system file descriptor. */
    int   flags; /**< File open access mode flags. */
    int   error; /**< Sticky error indicator flag (1 if an I/O error occurred). */
    int   eof;   /**< Sticky end-of-file indicator flag (1 if EOF was reached). */
} FILE;

/** Pre-allocated stream pointer for standard input (file descriptor 0). */
extern FILE *stdin;

/** Pre-allocated stream pointer for standard output (file descriptor 1). */
extern FILE *stdout;

/** Pre-allocated stream pointer for standard error (file descriptor 2). */
extern FILE *stderr;

/* ============================================================================
 * Character & String I/O Prototypes
 * ============================================================================ */

/** Writes a single character to standard output. */
int  putchar(int c);

/** Reads a single character from standard input. */
int  getchar(void);

/** Writes a string followed by a newline to standard output. */
int  puts(const char *s);

/** Writes a character to a specified stream. */
int  fputc(int c, FILE *stream);

/** Reads a character from a specified stream. */
int  fgetc(FILE *stream);

/** Writes a string to a specified stream without appending a newline. */
int  fputs(const char *s, FILE *stream);

/* ============================================================================
 * Formatted Output Prototypes
 * ============================================================================ */

/** Formats output and prints to standard output. */
int  printf(const char *format, ...);

/** Formats output and writes into a character string buffer (unbounded). */
int  sprintf(char *str, const char *format, ...);

/** Formats output and writes up to size bytes into a character buffer. */
int  snprintf(char *str, size_t size, const char *format, ...);

/** Formats output to standard output using a va_list. */
int  vprintf(const char *format, __builtin_va_list ap);

/** Formats output to a string buffer using a va_list. */
int  vsprintf(char *str, const char *format, __builtin_va_list ap);

/** Formats output to a sized string buffer using a va_list. */
int  vsnprintf(char *str, size_t size, const char *format, __builtin_va_list ap);

/* ============================================================================
 * File Stream Management Prototypes
 * ============================================================================ */

/** Opens a file path and creates an associated FILE stream object. */
FILE  *fopen(const char *pathname, const char *mode);

/** Closes a FILE stream and flushes its descriptor. */
int    fclose(FILE *stream);

/** Reads nmemb binary objects of specified size from stream into memory. */
size_t fread(void *ptr, size_t size, size_t nmemb, FILE *stream);

/** Writes nmemb binary objects of specified size from memory into stream. */
size_t fwrite(const void *ptr, size_t size, size_t nmemb, FILE *stream);

/** Repositions the file stream offset indicator. */
int    fseek(FILE *stream, long offset, int whence);

/** Returns the current byte offset of the stream indicator. */
long   ftell(FILE *stream);

/** Flushes any unwritten buffered stream data to disk. */
int    fflush(FILE *stream);

/** Returns non-zero if stream has encountered end-of-file. */
int    feof(FILE *stream);

/** Returns non-zero if an error occurred on the stream. */
int    ferror(FILE *stream);

#endif /* _STDIO_H */
