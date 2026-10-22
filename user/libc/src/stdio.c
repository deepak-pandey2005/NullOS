#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <fcntl.h>
#include <stdint.h>

static FILE _stdin_obj  = { .fd = STDIN_FILENO,  .flags = 0, .error = 0, .eof = 0 };
static FILE _stdout_obj = { .fd = STDOUT_FILENO, .flags = 0, .error = 0, .eof = 0 };
static FILE _stderr_obj = { .fd = STDERR_FILENO, .flags = 0, .error = 0, .eof = 0 };

FILE *stdin  = &_stdin_obj;
FILE *stdout = &_stdout_obj;
FILE *stderr = &_stderr_obj;

int putchar(int c) {
    unsigned char ch = (unsigned char)c;
    if (write(STDOUT_FILENO, &ch, 1) == 1) {
        return c;
    }
    return EOF;
}

int puts(const char *s) {
    size_t len = strlen(s);
    write(STDOUT_FILENO, s, len);
    write(STDOUT_FILENO, "\n", 1);
    return 0;
}

int getchar(void) {
    unsigned char ch;
    ssize_t n = read(STDIN_FILENO, &ch, 1);
    if (n == 1) {
        return (int)ch;
    }
    return EOF;
}

int fputc(int c, FILE *stream) {
    if (!stream) return EOF;
    unsigned char ch = (unsigned char)c;
    if (write(stream->fd, &ch, 1) == 1) {
        return c;
    }
    stream->error = 1;
    return EOF;
}

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

int fputs(const char *s, FILE *stream) {
    if (!stream) return EOF;
    size_t len = strlen(s);
    if (write(stream->fd, s, len) == (ssize_t)len) {
        return 0;
    }
    stream->error = 1;
    return EOF;
}

/* Helper to format an unsigned integer into a string buffer with given base */
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

int snprintf(char *str, size_t size, const char *format, ...) {
    __builtin_va_list ap;
    __builtin_va_start(ap, format);
    int ret = vsnprintf(str, size, format, ap);
    __builtin_va_end(ap);
    return ret;
}

int vsprintf(char *str, const char *format, __builtin_va_list ap) {
    return vsnprintf(str, (size_t)-1, format, ap);
}

int sprintf(char *str, const char *format, ...) {
    __builtin_va_list ap;
    __builtin_va_start(ap, format);
    int ret = vsprintf(str, format, ap);
    __builtin_va_end(ap);
    return ret;
}

int vprintf(const char *format, __builtin_va_list ap) {
    char buf[1024];
    int len = vsnprintf(buf, sizeof(buf), format, ap);
    if (len > 0) {
        size_t to_write = (len < (int)sizeof(buf)) ? (size_t)len : sizeof(buf) - 1;
        write(STDOUT_FILENO, buf, to_write);
    }
    return len;
}

int printf(const char *format, ...) {
    __builtin_va_list ap;
    __builtin_va_start(ap, format);
    int ret = vprintf(format, ap);
    __builtin_va_end(ap);
    return ret;
}

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

int fclose(FILE *stream) {
    if (!stream) return EOF;
    int res = close(stream->fd);
    if (stream != stdin && stream != stdout && stream != stderr) {
        free(stream);
    }
    return res;
}

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

long ftell(FILE *stream) {
    if (!stream) return -1;
    return (long)lseek(stream->fd, 0, SEEK_CUR);
}

int fflush(FILE *stream) {
    (void)stream;
    return 0;
}

int feof(FILE *stream) {
    return stream ? stream->eof : 0;
}

int ferror(FILE *stream) {
    return stream ? stream->error : 0;
}
