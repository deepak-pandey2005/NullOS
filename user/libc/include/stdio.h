#ifndef _STDIO_H
#define _STDIO_H

#include <stddef.h>
#include <stdint.h>
#include <sys/types.h>

#define EOF (-1)

typedef struct FILE {
    int   fd;
    int   flags;
    int   error;
    int   eof;
} FILE;

extern FILE *stdin;
extern FILE *stdout;
extern FILE *stderr;

/* Character & String I/O */
int  putchar(int c);
int  getchar(void);
int  puts(const char *s);
int  fputc(int c, FILE *stream);
int  fgetc(FILE *stream);
int  fputs(const char *s, FILE *stream);

/* Formatted Output */
int  printf(const char *format, ...);
int  sprintf(char *str, const char *format, ...);
int  snprintf(char *str, size_t size, const char *format, ...);
int  vprintf(const char *format, __builtin_va_list ap);
int  vsprintf(char *str, const char *format, __builtin_va_list ap);
int  vsnprintf(char *str, size_t size, const char *format, __builtin_va_list ap);

/* File Operations */
FILE  *fopen(const char *pathname, const char *mode);
int    fclose(FILE *stream);
size_t fread(void *ptr, size_t size, size_t nmemb, FILE *stream);
size_t fwrite(const void *ptr, size_t size, size_t nmemb, FILE *stream);
int    fseek(FILE *stream, long offset, int whence);
long   ftell(FILE *stream);
int    fflush(FILE *stream);
int    feof(FILE *stream);
int    ferror(FILE *stream);

#endif /* _STDIO_H */
