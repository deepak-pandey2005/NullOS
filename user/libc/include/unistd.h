#ifndef _UNISTD_H
#define _UNISTD_H

#include <stddef.h>
#include <stdint.h>
#include <sys/types.h>

#define STDIN_FILENO  0
#define STDOUT_FILENO 1
#define STDERR_FILENO 2

#define SEEK_SET 0
#define SEEK_CUR 1
#define SEEK_END 2

/* File and I/O Operations */
ssize_t read(int fd, void *buf, size_t count);
ssize_t write(int fd, const void *buf, size_t count);
int     close(int fd);
off_t   lseek(int fd, off_t offset, int whence);
int     isatty(int fd);

/* Process Management */
pid_t   getpid(void);
void    exit(int status) __attribute__((noreturn));
int     yield(void);
pid_t   waitpid(pid_t pid, int *status, int options);

/* Timing */
unsigned int sleep(unsigned int seconds);
int          usleep(unsigned long usec);

#endif /* _UNISTD_H */
