/**
 * @file unistd.c
 * @brief NullOS Standard Symbolic Constants & System Types Implementation
 *
 * This file provides standard POSIX operating system API entry points for
 * NullOS user applications, including low-level file I/O (`read`, `write`, `close`,
 * `lseek`), process control (`getpid`, `yield`, `waitpid`, `exit`), and sleep/timing
 * functions (`sleep`, `usleep`).
 *
 * Each function wraps a corresponding NullOS kernel system call, abstracting
 * architecture-specific hardware calling conventions into standard C interfaces.
 */

#include <unistd.h>
#include <fcntl.h>
#include <sys/syscall.h>

/**
 * @brief Reads data from an open file descriptor into a buffer.
 *
 * Invokes the `SYS_READ` (0) system call. The kernel transfers up to `count`
 * bytes from the file description associated with `fd` into the userspace buffer `buf`.
 *
 * @param fd    File descriptor to read from (e.g. STDIN_FILENO, open file, or socket).
 * @param buf   Pointer to destination memory buffer in userspace.
 * @param count Maximum number of bytes to read.
 * @return Number of bytes actually read on success, 0 on end-of-file (EOF),
 *         or a negative errno value on error.
 */
ssize_t read(int fd, void *buf, size_t count) {
    return (ssize_t)syscall3(SYS_READ, fd, (long)buf, (long)count);
}

/**
 * @brief Writes data from a buffer to an open file descriptor.
 *
 * Invokes the `SYS_WRITE` (1) system call. The kernel transfers up to `count`
 * bytes from the userspace buffer `buf` to the file or device represented by `fd`.
 *
 * @param fd    File descriptor to write to (e.g. STDOUT_FILENO, STDERR_FILENO, file).
 * @param buf   Pointer to source data buffer in userspace.
 * @param count Number of bytes to write.
 * @return Number of bytes actually written on success, or a negative errno value on error.
 */
ssize_t write(int fd, const void *buf, size_t count) {
    return (ssize_t)syscall3(SYS_WRITE, fd, (long)buf, (long)count);
}

/**
 * @brief Opens and possibly creates a file by path name.
 *
 * Invokes the `SYS_OPEN` (2) system call. Resolves the given path within the
 * Virtual File System (VFS) and allocates a new file descriptor for the process.
 *
 * @param pathname Null-terminated string containing the file system path.
 * @param flags    Access mode and creation flags (e.g., O_RDONLY, O_WRONLY, O_CREAT).
 * @param ...      Optional file creation mode permission bits (ignored in basic VFS).
 * @return Non-negative file descriptor on success, or a negative errno on failure.
 */
int open(const char *pathname, int flags, ...) {
    return (int)syscall2(SYS_OPEN, (long)pathname, (long)flags);
}

/**
 * @brief Closes an open file descriptor.
 *
 * Invokes the `SYS_CLOSE` (3) system call. Deallocates the file descriptor from
 * the process file table and decrements the underlying file object reference count.
 *
 * @param fd The file descriptor to release.
 * @return 0 on success, or a negative errno on failure.
 */
int close(int fd) {
    return (int)syscall1(SYS_CLOSE, fd);
}

/**
 * @brief Repositions the read/write file offset of an open file descriptor.
 *
 * Invokes the `SYS_LSEEK` (8) system call. Adjusts the byte cursor for future
 * read and write operations.
 *
 * @param fd     The open file descriptor.
 * @param offset The byte offset relative to the directive in `whence`.
 * @param whence Position directive: SEEK_SET (absolute offset), SEEK_CUR
 *               (relative to current offset), or SEEK_END (relative to file size).
 * @return The resulting file offset in bytes from the start of the file, or negative errno on error.
 */
off_t lseek(int fd, off_t offset, int whence) {
    return (off_t)syscall3(SYS_LSEEK, fd, (long)offset, (long)whence);
}

/**
 * @brief Terminates the calling process immediately.
 *
 * Invokes the `SYS_EXIT` (60) system call with the specified status code.
 * The kernel cleans up process resources, frees virtual memory address space,
 * closes open file descriptors, and notifies the parent process or wakes up waitpid.
 * This function never returns.
 *
 * @param status The exit status code returned to the parent process.
 */
void exit(int status) {
    syscall1(SYS_EXIT, status);
    /* In the impossible event that the exit system call returns, halt execution */
    while (1) {
        __asm__ __volatile__("hlt");
    }
}

/**
 * @brief Obtains the process ID of the calling process.
 *
 * Invokes the `SYS_GETPID` (39) system call to query the current process ID.
 *
 * @return The unique process ID (pid_t) of the caller.
 */
pid_t getpid(void) {
    return (pid_t)syscall0(SYS_GETPID);
}

/**
 * @brief Voluntarily relinquishes the CPU to allow other threads or processes to run.
 *
 * Invokes the `SYS_YIELD` (24) system call, triggering a cooperative context
 * switch in the kernel scheduler.
 *
 * @return 0 on success.
 */
int yield(void) {
    return (int)syscall0(SYS_YIELD);
}

/**
 * @brief Waits for state changes in a child process.
 *
 * Invokes the `SYS_WAITPID` (61) system call. Suspends the calling process until
 * a child process specified by `pid` has terminated or changed state.
 *
 * @param pid     Process ID to wait for (or -1 to wait for any child).
 * @param status  Pointer to integer where child's exit status is stored.
 * @param options Options flags (e.g. WNOHANG; currently unused in basic libc).
 * @return The process ID of the child whose state changed, or negative errno on error.
 */
pid_t waitpid(pid_t pid, int *status, int options) {
    (void)options;
    return (pid_t)syscall2(SYS_WAITPID, (long)pid, (long)status);
}

/**
 * @brief Suspends the execution of the calling thread for a specified number of seconds.
 *
 * NullOS's kernel timer tick runs at approximately 100 Hz (10 ms per tick).
 * Converts seconds into timer ticks and invokes the `SYS_SLEEP` (100) system call.
 *
 * @param seconds Number of whole seconds to sleep.
 * @return 0 on completion.
 */
unsigned int sleep(unsigned int seconds) {
    /* NullOS APIC timer runs at ~100Hz (10ms per tick) */
    syscall1(SYS_SLEEP, (long)(seconds * 100));
    return 0;
}

/**
 * @brief Suspends the execution of the calling thread for a specified number of microseconds.
 *
 * Converts microseconds into kernel timer ticks (~10,000 microseconds per tick
 * for a 100 Hz timer). Ensures that any non-zero microsecond sleep request sleeps
 * for at least one tick.
 *
 * @param usec Number of microseconds to suspend execution.
 * @return 0 on completion.
 */
int usleep(unsigned long usec) {
    /* Convert microseconds to ticks (~10,000 usec per tick) */
    long ticks = (long)(usec / 10000);
    if (ticks == 0 && usec > 0) {
        ticks = 1;
    }
    syscall1(SYS_SLEEP, ticks);
    return 0;
}

/**
 * @brief Determines whether a file descriptor refers to a terminal/console device.
 *
 * In NullOS, file descriptors 0 (stdin), 1 (stdout), and 2 (stderr) are connected
 * directly to the interactive serial / VGA console stream.
 *
 * @param fd The file descriptor to test.
 * @return 1 if fd refers to a terminal, 0 otherwise.
 */
int isatty(int fd) {
    /* In NullOS, file descriptors 0 (stdin), 1 (stdout), and 2 (stderr) are consoles */
    if (fd >= 0 && fd <= 2) {
        return 1;
    }
    return 0;
}
