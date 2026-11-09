/**
 * @file stdio.c
 * @brief NullOS Standard I/O (Input/Output) Library Implementation
 *
 * This file provides the standard stream-oriented I/O facilities for NullOS user
 * programs, including:
 * - Pre-initialized standard streams: `stdin` (fd 0), `stdout` (fd 1), `stderr` (fd 2).
 * - Character and string I/O: `putchar`, `getchar`, `puts`, `fputc`, `fgetc`, `fputs`.
 * - Formatted output engine: `vsnprintf`, `snprintf`, `sprintf`, `printf`, supporting
 *   integer base conversion (hex, decimal), width padding, '0' padding, pointers,
 *   strings, characters, and length modifiers ('l', 'll').
 * - Stream file management: `fopen`, `fclose`, `fread`, `fwrite`, `fseek`, `ftell`,
 *   `fflush`, `feof`, and `ferror`.
 */

#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <fcntl.h>
#include <stdint.h>

/** Static FILE object backing standard input (file descriptor 0). */
static FILE _stdin_obj  = { .fd = STDIN_FILENO,  .flags = 0, .error = 0, .eof = 0 };

/** Static FILE object backing standard output (file descriptor 1). */
static FILE _stdout_obj = { .fd = STDOUT_FILENO, .flags = 0, .error = 0, .eof = 0 };

/** Static FILE object backing standard error (file descriptor 2). */
static FILE _stderr_obj = { .fd = STDERR_FILENO, .flags = 0, .error = 0, .eof = 0 };

/** Global stream pointer for standard input. */
FILE *stdin  = &_stdin_obj;

/** Global stream pointer for standard output. */
FILE *stdout = &_stdout_obj;

/** Global stream pointer for standard error. */
FILE *stderr = &_stderr_obj;

/**
 * @brief Writes a character to standard output.
 *
 * @param c The character (cast to unsigned char) to write.
 * @return The character written as an unsigned char cast to int, or EOF on error.
 */
int putchar(int c) {
    unsigned char ch = (unsigned char)c;
    if (write(STDOUT_FILENO, &ch, 1) == 1) {
        return c;
    }
    return EOF;
}

/**
 * @brief Writes a string and a trailing newline to standard output.
 *
 * @param s Null-terminated string to output.
 * @return 0 on success, or negative on error.
 */
int puts(const char *s) {
    size_t len = strlen(s);
    write(STDOUT_FILENO, s, len);
    write(STDOUT_FILENO, "\n", 1);
    return 0;
}

/**
 * @brief Reads a single character from standard input.
 *
 * @return The character read as an unsigned char cast to int, or EOF on end-of-file or error.
 */
int getchar(void) {
    unsigned char ch;
    ssize_t n = read(STDIN_FILENO, &ch, 1);
    if (n == 1) {
        return (int)ch;
    }
    return EOF;
}

/**
 * @brief Writes a character to the specified file stream.
 *
 * @param c      Character to write.
 * @param stream Target FILE stream pointer.
 * @return The character written, or EOF on failure.
 */
int fputc(int c, FILE *stream) {
    if (!stream) return EOF;
    unsigned char ch = (unsigned char)c;
    if (write(stream->fd, &ch, 1) == 1) {
        return c;
    }
    stream->error = 1;
    return EOF;
}

/**
 * @brief Reads the next character from the specified file stream.
 *
 * @param stream Target FILE stream pointer.
 * @return Character read as int, or EOF on end-of-file / read error.
 */
int fgetc(FILE *stream) {
    if (!stream) return EOF;
    unsigned char ch;
    ssize_t n = read(stream->fd, &ch, 1);
    if (n == 1) {
        return (int)ch;
    }
    if (n == 0) {
        stream->eof = 1;
    } else {
        stream->error = 1;
    }
    return EOF;
}

/**
 * @brief Writes a string to the specified file stream without appending a newline.
 *
 * @param s      Null-terminated string.
 * @param stream Target FILE stream.
 * @return 0 on success, or EOF on error.
 */
int fputs(const char *s, FILE *stream) {
    if (!stream) return EOF;
    size_t len = strlen(s);
    if (write(stream->fd, s, len) == (ssize_t)len) {
        return 0;
    }
    stream->error = 1;
    return EOF;
}

/**
 * @brief Formats an unsigned 64-bit integer into a string buffer with given base and padding.
 *
 * Internal formatting helper for `vsnprintf`.
 *
 * @param buf       Output string buffer.
 * @param max       Maximum capacity of buffer.
 * @param val       Unsigned 64-bit value to convert.
 * @param base      Radix base (typically 10 or 16).
 * @param uppercase 1 for uppercase hex digits ("ABCDEF"), 0 for lowercase ("abcdef").
 * @param min_width Minimum field width.
 * @param pad_char  Padding character (' ' or '0').
 * @return Number of characters written into `buf`.
 */
static int format_uint(char *buf, size_t max, uint64_t val, int base, int uppercase, int min_width, char pad_char) {
    char temp[65];
    int pos = 0;
    const char *digits = uppercase ? "0123456789ABCDEF" : "0123456789abcdef";

    if (val == 0) {
        temp[pos++] = '0';
    } else {
        while (val > 0) {
            temp[pos++] = digits[val % base];
            val /= base;
        }
    }

    int written = 0;
    int pad_len = min_width - pos;
    while (pad_len > 0 && written < (int)max - 1) {
        buf[written++] = pad_char;
        pad_len--;
    }

    while (pos > 0 && written < (int)max - 1) {
        buf[written++] = temp[--pos];
    }
    buf[written] = '\0';
    return written;
}

/**
 * @brief Writes formatted output to a sized string buffer using a va_list.
 *
 * Handles format specifiers:
 * - `%s`: Null-terminated string (with optional minimum width padding).
 * - `%c`: Single character.
 * - `%d`, `%i`: Signed decimal integer (with 'l' / 'll' length modifiers and width).
 * - `%u`: Unsigned decimal integer.
 * - `%x`, `%X`: Unsigned hexadecimal integer (lowercase / uppercase).
 * - `%p`: Pointer address formatted as 0x0000... (16-char padded hex).
 * - `%%`: Literal percent sign.
 *
 * @param str    Destination buffer.
 * @param size   Maximum number of bytes to store in `str` (including null terminator).
 * @param format Format control string.
 * @param ap     Variable argument list pointer.
 * @return Total number of characters that would have been written (excluding null terminator).
 */
int vsnprintf(char *str, size_t size, const char *format, __builtin_va_list ap) {
    if (!str && size > 0) {
        return -1;
    }

    size_t written = 0;
    const char *p = format;

    #define EMIT_CHAR(c) do { \
        if (str && written + 1 < size) { \
            str[written] = (c); \
        } \
        written++; \
    } while(0)

    while (*p) {
        if (*p != '%') {
            EMIT_CHAR(*p++);
            continue;
        }

        p++; /* Skip '%' */

        /* Zero padding flag */
        char pad_char = ' ';
        if (*p == '0') {
            pad_char = '0';
            p++;
        }

        /* Minimum width */
        int min_width = 0;
        while (*p >= '0' && *p <= '9') {
            min_width = min_width * 10 + (*p - '0');
            p++;
        }

        /* Length modifiers: 'l' or 'll' */
        int is_long = 0;
        if (*p == 'l') {
            is_long++;
            p++;
            if (*p == 'l') {
                is_long++;
                p++;
            }
        }

        switch (*p) {
            case 's': {
                const char *s = __builtin_va_arg(ap, const char *);
                if (!s) s = "(null)";
                int len = strlen(s);
                int pad = min_width - len;
                while (pad > 0) {
                    EMIT_CHAR(' ');
                    pad--;
                }
                while (*s) {
                    EMIT_CHAR(*s++);
                }
                break;
            }
            case 'c': {
                char c = (char)__builtin_va_arg(ap, int);
                EMIT_CHAR(c);
                break;
            }
            case 'd':
            case 'i': {
                int64_t val = (is_long >= 2) ? __builtin_va_arg(ap, long long) :
                              (is_long == 1) ? (int64_t)__builtin_va_arg(ap, long) :
                              (int64_t)__builtin_va_arg(ap, int);
                if (val < 0) {
                    EMIT_CHAR('-');
                    val = -val;
                    if (min_width > 0) min_width--;
                }
                char num_buf[65];
                int nlen = format_uint(num_buf, sizeof(num_buf), (uint64_t)val, 10, 0, min_width, pad_char);
                for (int i = 0; i < nlen; i++) {
                    EMIT_CHAR(num_buf[i]);
                }
                break;
            }
            case 'u': {
                uint64_t val = (is_long >= 2) ? __builtin_va_arg(ap, unsigned long long) :
                               (is_long == 1) ? (uint64_t)__builtin_va_arg(ap, unsigned long) :
                               (uint64_t)__builtin_va_arg(ap, unsigned int);
                char num_buf[65];
                int nlen = format_uint(num_buf, sizeof(num_buf), val, 10, 0, min_width, pad_char);
                for (int i = 0; i < nlen; i++) {
                    EMIT_CHAR(num_buf[i]);
                }
                break;
            }
            case 'x':
            case 'X': {
                uint64_t val = (is_long >= 2) ? __builtin_va_arg(ap, unsigned long long) :
                               (is_long == 1) ? (uint64_t)__builtin_va_arg(ap, unsigned long) :
                               (uint64_t)__builtin_va_arg(ap, unsigned int);
                char num_buf[65];
                int nlen = format_uint(num_buf, sizeof(num_buf), val, 16, (*p == 'X'), min_width, pad_char);
                for (int i = 0; i < nlen; i++) {
                    EMIT_CHAR(num_buf[i]);
                }
                break;
            }
            case 'p': {
                uintptr_t ptr = (uintptr_t)__builtin_va_arg(ap, void *);
                EMIT_CHAR('0');
                EMIT_CHAR('x');
                char num_buf[65];
                int nlen = format_uint(num_buf, sizeof(num_buf), (uint64_t)ptr, 16, 0, 16, '0');
                for (int i = 0; i < nlen; i++) {
                    EMIT_CHAR(num_buf[i]);
                }
                break;
            }
            case '%': {
                EMIT_CHAR('%');
                break;
            }
            default: {
                EMIT_CHAR('%');
                if (*p) {
                    EMIT_CHAR(*p);
                }
                break;
            }
        }
        if (*p) p++;
    }

    #undef EMIT_CHAR

    if (str && size > 0) {
        if (written < size) {
            str[written] = '\0';
        } else {
            str[size - 1] = '\0';
        }
    }

    return (int)written;
}

/**
 * @brief Formats output and writes it to a fixed-size character buffer.
 *
 * @param str    Destination buffer.
 * @param size   Capacity of buffer.
 * @param format Format string.
 * @param ...    Format arguments.
 * @return Number of characters written (or would be written).
 */
int snprintf(char *str, size_t size, const char *format, ...) {
    __builtin_va_list ap;
    __builtin_va_start(ap, format);
    int ret = vsnprintf(str, size, format, ap);
    __builtin_va_end(ap);
    return ret;
}

/**
 * @brief Formats output to a string buffer using a va_list (unbounded).
 *
 * @param str    Destination buffer.
 * @param format Format string.
 * @param ap     Variable argument list.
 * @return Number of characters written.
 */
int vsprintf(char *str, const char *format, __builtin_va_list ap) {
    return vsnprintf(str, (size_t)-1, format, ap);
}

/**
 * @brief Formats output to an unbounded string buffer.
 *
 * @param str    Destination string buffer.
 * @param format Format string.
 * @param ...    Format arguments.
 * @return Number of characters written.
 */
int sprintf(char *str, const char *format, ...) {
    __builtin_va_list ap;
    __builtin_va_start(ap, format);
    int ret = vsprintf(str, format, ap);
    __builtin_va_end(ap);
    return ret;
}

/**
 * @brief Formats output and writes directly to standard output via va_list.
 *
 * @param format Format string.
 * @param ap     Variable argument list.
 * @return Number of characters output.
 */
int vprintf(const char *format, __builtin_va_list ap) {
    char buf[1024];
    int len = vsnprintf(buf, sizeof(buf), format, ap);
    if (len > 0) {
        size_t to_write = (len < (int)sizeof(buf)) ? (size_t)len : sizeof(buf) - 1;
        write(STDOUT_FILENO, buf, to_write);
    }
    return len;
}

/**
 * @brief Prints formatted output to standard output (STDOUT_FILENO).
 *
 * @param format Format control string.
 * @param ...    Variable arguments matching format specifiers.
 * @return Number of characters printed.
 */
int printf(const char *format, ...) {
    __builtin_va_list ap;
    __builtin_va_start(ap, format);
    int ret = vprintf(format, ap);
    __builtin_va_end(ap);
    return ret;
}

/**
 * @brief Opens the file whose name is specified by `pathname` and associates a stream with it.
 *
 * Parses modes: "r", "w", "a", "r+", "w+". Allocates a heap `FILE` structure
 * and opens the file descriptor via `open()`.
 *
 * @param pathname Path to file in VFS.
 * @param mode     Access mode string ("r", "w", "a", etc.).
 * @return Pointer to FILE stream object, or NULL on error.
 */
FILE *fopen(const char *pathname, const char *mode) {
    int flags = 0;
    if (strcmp(mode, "r") == 0) {
        flags = O_RDONLY;
    } else if (strcmp(mode, "w") == 0) {
        flags = O_WRONLY | O_CREAT | O_TRUNC;
    } else if (strcmp(mode, "a") == 0) {
        flags = O_WRONLY | O_CREAT | O_APPEND;
    } else if (strcmp(mode, "r+") == 0) {
        flags = O_RDWR;
    } else if (strcmp(mode, "w+") == 0) {
        flags = O_RDWR | O_CREAT | O_TRUNC;
    } else {
        flags = O_RDONLY;
    }

    int fd = open(pathname, flags);
    if (fd < 0) {
        return NULL;
    }

    FILE *stream = (FILE *)malloc(sizeof(FILE));
    if (!stream) {
        close(fd);
        return NULL;
    }

    stream->fd = fd;
    stream->flags = flags;
    stream->error = 0;
    stream->eof = 0;
    return stream;
}

/**
 * @brief Closes a FILE stream and flushes any unwritten data.
 *
 * Closes the underlying file descriptor and frees the allocated `FILE` structure
 * (unless stream is standard stdin, stdout, or stderr).
 *
 * @param stream FILE pointer to close.
 * @return 0 on success, or EOF on error.
 */
int fclose(FILE *stream) {
    if (!stream) return EOF;
    int res = close(stream->fd);
    if (stream != stdin && stream != stdout && stream != stderr) {
        free(stream);
    }
    return res;
}

/**
 * @brief Reads up to `nmemb` elements, each of size `size`, from stream into buffer `ptr`.
 *
 * @param ptr    Destination memory buffer.
 * @param size   Size of each element in bytes.
 * @param nmemb  Number of elements to read.
 * @param stream Target FILE stream.
 * @return Number of full elements successfully read.
 */
size_t fread(void *ptr, size_t size, size_t nmemb, FILE *stream) {
    if (!stream || size == 0 || nmemb == 0) return 0;
    size_t total_bytes = size * nmemb;
    ssize_t bytes_read = read(stream->fd, ptr, total_bytes);
    if (bytes_read <= 0) {
        if (bytes_read == 0) stream->eof = 1;
        else stream->error = 1;
        return 0;
    }
    return (size_t)bytes_read / size;
}

/**
 * @brief Writes up to `nmemb` elements, each of size `size`, from buffer `ptr` to stream.
 *
 * @param ptr    Source memory buffer.
 * @param size   Size of each element in bytes.
 * @param nmemb  Number of elements to write.
 * @param stream Target FILE stream.
 * @return Number of full elements successfully written.
 */
size_t fwrite(const void *ptr, size_t size, size_t nmemb, FILE *stream) {
    if (!stream || size == 0 || nmemb == 0) return 0;
    size_t total_bytes = size * nmemb;
    ssize_t bytes_written = write(stream->fd, ptr, total_bytes);
    if (bytes_written <= 0) {
        stream->error = 1;
        return 0;
    }
    return (size_t)bytes_written / size;
}

/**
 * @brief Sets the file position indicator for the stream.
 *
 * @param stream Target FILE stream.
 * @param offset Byte offset.
 * @param whence Position reference: SEEK_SET, SEEK_CUR, or SEEK_END.
 * @return 0 on success, or -1 on error.
 */
int fseek(FILE *stream, long offset, int whence) {
    if (!stream) return -1;
    off_t res = lseek(stream->fd, (off_t)offset, whence);
    if (res < 0) {
        stream->error = 1;
        return -1;
    }
    stream->eof = 0;
    return 0;
}

/**
 * @brief Obtains the current value of the file position indicator for the stream.
 *
 * @param stream Target FILE stream.
 * @return Current byte offset in stream, or -1L on error.
 */
long ftell(FILE *stream) {
    if (!stream) return -1;
    return (long)lseek(stream->fd, 0, SEEK_CUR);
}

/**
 * @brief Flushes any buffered unwritten data to the stream's file descriptor.
 *
 * In NullOS's direct-write libc implementation, writes are synchronous; thus
 * fflush is a no-op returning 0.
 *
 * @param stream Target FILE stream.
 * @return 0 on success.
 */
int fflush(FILE *stream) {
    (void)stream;
    return 0;
}

/**
 * @brief Tests the end-of-file indicator for the stream.
 *
 * @param stream Target FILE stream.
 * @return Non-zero if EOF indicator is set, 0 otherwise.
 */
int feof(FILE *stream) {
    return stream ? stream->eof : 0;
}

/**
 * @brief Tests the error indicator for the stream.
 *
 * @param stream Target FILE stream.
 * @return Non-zero if error indicator is set, 0 otherwise.
 */
int ferror(FILE *stream) {
    return stream ? stream->error : 0;
}
