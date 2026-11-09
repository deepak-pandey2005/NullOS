/**
 * @file stddef.h
 * @brief NullOS Standard Type Definitions & Pointer Utilities
 *
 * This header defines fundamental C types (`size_t`, `ssize_t`, `ptrdiff_t`),
 * the null pointer constant (`NULL`), and the member offset calculation macro (`offsetof`).
 */

#ifndef _STDDEF_H
#define _STDDEF_H

#ifndef _SIZE_T_DEFINED
#define _SIZE_T_DEFINED
/** Unsigned integer type returned by sizeof operator. */
typedef unsigned long size_t;
#endif

#ifndef _SSIZE_T_DEFINED
#define _SSIZE_T_DEFINED
/** Signed integer type used for byte counts or -1 error indicators. */
typedef long ssize_t;
#endif

#ifndef _PTRDIFF_T_DEFINED
#define _PTRDIFF_T_DEFINED
/** Signed integer type representing difference between two memory pointers. */
typedef long ptrdiff_t;
#endif

#ifndef NULL
/** Null pointer constant representation. */
#define NULL ((void *)0)
#endif

#ifndef offsetof
/** Evaluates to the offset in bytes of a structure member from the beginning of its structure. */
#define offsetof(type, member) __builtin_offsetof(type, member)
#endif

#endif /* _STDDEF_H */
