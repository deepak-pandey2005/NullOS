/**
 * @file unistd.h
 * @brief NullOS Standard Symbolic Constants and Primitive System Interfaces
 *
 * This header defines standard file descriptor numbers (`STDIN_FILENO`,
 * `STDOUT_FILENO`, `STDERR_FILENO`), seek directives (`SEEK_SET`, `SEEK_CUR`,
 * `SEEK_END`), and function prototypes for standard low-level I/O, process
 * control, and thread sleeping.
 */

#ifndef _UNISTD_H
#define _UNISTD_H

#include <stddef.h>
#include <stdint.h>
#include <sys/types.h>

/* ============================================================================
 * Standard File Descriptor Numbers
 * ============================================================================ */

/** Standard input file descriptor (keyboard/serial stream). */
#define STDIN_FILENO  0

/** Standard output file descriptor (console/display stream). */
#define STDOUT_FILENO 1

/** Standard error file descriptor (diagnostic logging stream). */
#define STDERR_FILENO 2

/* ============================================================================
 * Seek Position Directives (lseek whence)
 * ============================================================================ */

/** Seek relative to beginning of file. */
#define SEEK_SET 0

/** Seek relative to current file offset cursor. */
#define SEEK_CUR 1

/** Seek relative to end of file. */
#define SEEK_END 2

/* ============================================================================
 * File and Low-Level I/O Prototypes
 * ============================================================================ */

/** Reads up to count bytes from file descriptor fd into buffer buf. */
ssize_t read(int fd, void *buf, size_t count);

/** Writes up to count bytes from buffer buf to file descriptor fd. */
ssize_t write(int fd, const void *buf, size_t count);

/** Closes an open file descriptor. */
int     close(int fd);

/** Repositions the read/write file offset cursor. */
off_t   lseek(int fd, off_t offset, int whence);

/** Tests whether a file descriptor refers to an interactive console terminal. */
int     isatty(int fd);

/* ============================================================================
 * Process Management Prototypes
 * ============================================================================ */

/** Returns the process ID of the calling process. */
pid_t   getpid(void);

/** Terminates the calling process with exit status code. */
void    exit(int status) __attribute__((noreturn));

/** Voluntarily relinquishes the processor to another ready thread. */
int     yield(void);

/** Suspends calling process until a child process changes state. */
pid_t   waitpid(pid_t pid, int *status, int options);

/* ============================================================================
 * Timing & Sleeping Prototypes
 * ============================================================================ */

/** Suspends execution of calling thread for specified integer seconds. */
unsigned int sleep(unsigned int seconds);

/** Suspends execution of calling thread for specified microseconds. */
int          usleep(unsigned long usec);

#endif /* _UNISTD_H */
