#include <unistd.h>
#include <fcntl.h>
#include <sys/syscall.h>

ssize_t read(int fd, void *buf, size_t count) {
    return (ssize_t)syscall3(SYS_READ, fd, (long)buf, (long)count);
}

ssize_t write(int fd, const void *buf, size_t count) {
    return (ssize_t)syscall3(SYS_WRITE, fd, (long)buf, (long)count);
}

int open(const char *pathname, int flags, ...) {
    return (int)syscall2(SYS_OPEN, (long)pathname, (long)flags);
}

int close(int fd) {
    return (int)syscall1(SYS_CLOSE, fd);
}

off_t lseek(int fd, off_t offset, int whence) {
    return (off_t)syscall3(SYS_LSEEK, fd, (long)offset, (long)whence);
}

void exit(int status) {
    syscall1(SYS_EXIT, status);
    while (1) {
        __asm__ __volatile__("hlt");
    }
}

pid_t getpid(void) {
    return (pid_t)syscall0(SYS_GETPID);
}

int yield(void) {
    return (int)syscall0(SYS_YIELD);
}

pid_t waitpid(pid_t pid, int *status, int options) {
    (void)options;
    return (pid_t)syscall2(SYS_WAITPID, (long)pid, (long)status);
}

unsigned int sleep(unsigned int seconds) {
    /* NullOS APIC timer runs at ~100Hz (10ms per tick) */
    syscall1(SYS_SLEEP, (long)(seconds * 100));
    return 0;
}

int usleep(unsigned long usec) {
    /* Convert microseconds to ticks (~10,000 usec per tick) */
    long ticks = (long)(usec / 10000);
    if (ticks == 0 && usec > 0) {
        ticks = 1;
    }
    syscall1(SYS_SLEEP, ticks);
    return 0;
}

int isatty(int fd) {
    /* In NullOS, file descriptors 0 (stdin), 1 (stdout), and 2 (stderr) are consoles */
    if (fd >= 0 && fd <= 2) {
        return 1;
    }
    return 0;
}
