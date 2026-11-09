/**
 * @file sys/types.h
 * @brief NullOS Standard Data Types for System Entities
 *
 * This header defines primitive system data types representing process IDs,
 * file offsets, device IDs, inode numbers, and permission modes.
 */

#ifndef _SYS_TYPES_H
#define _SYS_TYPES_H

#include <stdint.h>
#include <stddef.h>

/** Signed integer type representing a process identifier. */
typedef long         pid_t;

/** Signed integer type representing a file offset or file size in bytes. */
typedef long         off_t;

/** Unsigned integer type representing file protection mode bits and file type. */
typedef unsigned int mode_t;

/** Unsigned integer type representing a user identifier. */
typedef unsigned int uid_t;

/** Unsigned integer type representing a group identifier. */
typedef unsigned int gid_t;

/** Unsigned integer type representing a device identifier (major/minor pair). */
typedef unsigned int dev_t;

/** Unsigned integer type representing a filesystem inode number. */
typedef unsigned int ino_t;

/** Unsigned integer type representing a hard link reference count. */
typedef unsigned int nlink_t;

#endif /* _SYS_TYPES_H */
