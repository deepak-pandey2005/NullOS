/**
 * @file sys/mman.h
 * @brief NullOS Memory Management Declarations & Flags (mmap / munmap)
 *
 * This header defines memory protection flags, mapping attributes, and function
 * prototypes for virtual memory management in NullOS userspace.
 */

#ifndef _SYS_MMAN_H
#define _SYS_MMAN_H

#include <stddef.h>
#include <sys/types.h>

/* ============================================================================
 * Memory Protection Flags (prot)
 * ============================================================================ */

/** Pages cannot be accessed. */
#define PROT_NONE   0x0

/** Pages can be read. */
#define PROT_READ   0x1

/** Pages can be written. */
#define PROT_WRITE  0x2

/** Pages can be executed as CPU instructions. */
#define PROT_EXEC   0x4

/* ============================================================================
 * Mapping Flags (flags)
 * ============================================================================ */

/** Share this mapping: modifications are visible to other processes. */
#define MAP_SHARED    0x01

/** Create a private copy-on-write mapping. */
#define MAP_PRIVATE   0x02

/** Mapping is not backed by any file; contents are zero-initialized. */
#define MAP_ANONYMOUS 0x20

/** Alias for MAP_ANONYMOUS. */
#define MAP_ANON      MAP_ANONYMOUS

/** Return value representing memory mapping error ((void *)-1). */
#define MAP_FAILED ((void *)-1)

/**
 * @brief Creates a new virtual memory mapping in the calling process's address space.
 *
 * @param addr   Starting address recommendation (usually NULL).
 * @param length Size of mapping in bytes (page-aligned by kernel).
 * @param prot   Protection bits (PROT_READ | PROT_WRITE, etc.).
 * @param flags  Mapping flags (MAP_PRIVATE | MAP_ANONYMOUS).
 * @param fd     File descriptor (if file-backed mapping, -1 for anonymous).
 * @param offset Byte offset in file.
 * @return Pointer to mapped memory area, or MAP_FAILED on failure.
 */
void *mmap(void *addr, size_t length, int prot, int flags, int fd, off_t offset);

/**
 * @brief Deletes mappings for the specified virtual address range.
 *
 * @param addr   Starting virtual address of pages to unmap.
 * @param length Size of memory range in bytes.
 * @return 0 on success, or -1 on failure.
 */
int munmap(void *addr, size_t length);

#endif /* _SYS_MMAN_H */
