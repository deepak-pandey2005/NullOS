/**
 * @file fcntl.h
 * @brief NullOS File Control Operations & Access Modes
 *
 * This header defines file status and access mode constants passed to `open()`
 * and file control routines in NullOS.
 */

#ifndef _FCNTL_H
#define _FCNTL_H

/* ============================================================================
 * File Access & Creation Flags (open flags)
 * ============================================================================ */

/** Open file for reading only. */
#define O_RDONLY    0x0000

/** Open file for writing only. */
#define O_WRONLY    0x0001

/** Open file for reading and writing. */
#define O_RDWR      0x0002

/** Create file if it does not exist. */
#define O_CREAT     0x0040

/** Ensure that this call creates the file: fail if file already exists. */
#define O_EXCL      0x0080

/** Truncate file length to 0 bytes if it already exists. */
#define O_TRUNC     0x0200

/** The file offset is repositioned to the end of the file prior to each write. */
#define O_APPEND    0x0400

/** Open file in non-blocking mode: operations return immediately rather than sleep. */
#define O_NONBLOCK  0x0800

/**
 * @brief Opens and possibly creates a file.
 *
 * @param pathname Null-terminated path string in VFS.
 * @param flags    Access and status flags (O_RDONLY, O_WRONLY, O_RDWR, etc.).
 * @param ...      Optional file creation mode permissions.
 * @return File descriptor integer on success, or -1 on error.
 */
int open(const char *pathname, int flags, ...);

#endif /* _FCNTL_H */
