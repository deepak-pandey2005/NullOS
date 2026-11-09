/**
 * @file sys/stat.h
 * @brief NullOS File Status and Inode Metadata Definitions (stat / fstat)
 *
 * This header defines the `stat` metadata structure, file type bitmasks,
 * classification test macros (`S_ISREG`, `S_ISDIR`, etc.), and function
 * declarations for querying file status.
 */

#ifndef _SYS_STAT_H
#define _SYS_STAT_H

#include <stdint.h>
#include <sys/types.h>

/**
 * @struct stat
 * @brief Inode and file metadata returned by `stat()` and `fstat()`.
 */
struct stat {
    dev_t     st_dev;     /**< ID of device containing file. */
    ino_t     st_ino;     /**< Inode number. */
    mode_t    st_mode;    /**< File type and permission mode bits. */
    nlink_t   st_nlink;   /**< Number of hard links to the file. */
    uid_t     st_uid;     /**< User ID of owner. */
    gid_t     st_gid;     /**< Group ID of owner. */
    dev_t     st_rdev;    /**< Device ID (if special character/block device). */
    off_t     st_size;    /**< Total size in bytes. */
    long      st_blksize; /**< Block size for filesystem I/O operations. */
    long      st_blocks;  /**< Number of 512B blocks allocated. */
};

/* ============================================================================
 * File Type Bitmasks (st_mode)
 * ============================================================================ */

/** Bitmask for the file type bit fields. */
#define S_IFMT   0170000

/** Socket file. */
#define S_IFSOCK 0140000

/** Symbolic link. */
#define S_IFLNK  0120000

/** Regular file. */
#define S_IFREG  0100000

/** Block device. */
#define S_IFBLK  0060000

/** Directory. */
#define S_IFDIR  0040000

/** Character device. */
#define S_IFCHR  0020000

/** FIFO / named pipe. */
#define S_IFIFO  0010000

/* ============================================================================
 * File Type Test Macros
 * ============================================================================ */

/** True if file is a regular file. */
#define S_ISREG(m)  (((m) & S_IFMT) == S_IFREG)

/** True if file is a directory. */
#define S_ISDIR(m)  (((m) & S_IFMT) == S_IFDIR)

/** True if file is a character device. */
#define S_ISCHR(m)  (((m) & S_IFMT) == S_IFCHR)

/** True if file is a block device. */
#define S_ISBLK(m)  (((m) & S_IFMT) == S_IFBLK)

/** True if file is a FIFO / pipe. */
#define S_ISFIFO(m) (((m) & S_IFMT) == S_IFIFO)

/** True if file is a symbolic link. */
#define S_ISLNK(m)  (((m) & S_IFMT) == S_IFLNK)

/** True if file is a UNIX domain socket. */
#define S_ISSOCK(m) (((m) & S_IFMT) == S_IFSOCK)

/**
 * @brief Obtains file status given a path name in the VFS.
 *
 * @param pathname Null-terminated file system path.
 * @param statbuf  Pointer to buffer where file metadata is written.
 * @return 0 on success, or -1 on error.
 */
int stat(const char *pathname, struct stat *statbuf);

/**
 * @brief Obtains file status given an open file descriptor.
 *
 * @param fd      Open file descriptor.
 * @param statbuf Pointer to buffer where file metadata is written.
 * @return 0 on success, or -1 on error.
 */
int fstat(int fd, struct stat *statbuf);

#endif /* _SYS_STAT_H */
