/**
 * @file errno.h
 * @brief NullOS Standard POSIX Error Numbers
 */

#ifndef _ERRNO_H
#define _ERRNO_H

extern int errno;

#define EPERM        1   /* Operation not permitted */
#define ENOENT       2   /* No such file or directory */
#define ESRCH        3   /* No such process */
#define EINTR        4   /* Interrupted system call */
#define EIO          5   /* I/O error */
#define ENXIO        6   /* No such device or address */
#define E2BIG        7   /* Argument list too long */
#define ENOEXEC      8   /* Exec format error */
#define EBADF        9   /* Bad file descriptor */
#define ECHILD      10   /* No child processes */
#define EAGAIN      11   /* Try again / Resource temporarily unavailable */
#define ENOMEM      12   /* Out of memory */
#define EACCES      13   /* Permission denied */
#define EFAULT      14   /* Bad address */
#define EBUSY       16   /* Device or resource busy */
#define EEXIST      17   /* File exists */
#define ENODEV      19   /* No such device */
#define ENOTDIR     20   /* Not a directory */
#define EISDIR      21   /* Is a directory */
#define EINVAL      22   /* Invalid argument */
#define EMFILE      24   /* Too many open files */
#define ENOSPC      28   /* No space left on device */
#define EPIPE       32   /* Broken pipe */
#define ERANGE      34   /* Math result not representable */
#define ENOSYS      38   /* Function not implemented */
#define ETIMEDOUT  110   /* Connection timed out */

#endif /* _ERRNO_H */

