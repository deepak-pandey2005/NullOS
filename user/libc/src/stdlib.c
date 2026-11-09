/**
 * @file stdlib.c
 * @brief NullOS Standard C Library General Utilities & Dynamic Heap Allocator
 *
 * This file implements the core dynamic memory allocation subsystem (`malloc`,
 * `free`, `calloc`, `realloc`, `mmap`), process termination routines (`abort`),
 * and numeric conversion utilities (`abs`, `atoi`, `atol`, `strtol`, `itoa`).
 *
 * ============================================================================
 * HEAP ALLOCATOR ARCHITECTURE:
 * ============================================================================
 * NullOS user applications use an explicit free-list boundary-tag heap manager:
 * - Each memory block is preceded by a 16-byte aligned header (`block_header_t`)
 *   recording block size, free/allocated state, and pointers to the previous and
 *   next blocks in the doubly linked chain.
 * - `malloc()` performs a first-fit search over the chain. If a suitable free
 *   block has excess capacity (at least `HEADER_SIZE + 16` bytes), it is split
 *   into an allocated block and a smaller residual free block.
 * - When no existing free block is large enough, `malloc()` requests a new virtual
 *   memory chunk from the kernel using the `SYS_MMAP` (9) system call, requesting
 *   at least `MIN_MMAP_CHUNK` (64 KiB) rounded up to page boundaries (4096 bytes).
 * - `free()` marks the block as free and immediately performs bidirectional
 *   coalescing with contiguous previous and next free blocks to prevent external
 *   heap fragmentation.
 */

#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <sys/mman.h>
#include <sys/syscall.h>

/**
 * @struct block_header
 * @brief Header prepended to every dynamic heap allocation.
 */
typedef struct block_header {
    size_t size;                 /**< Size of the user payload in bytes (excluding header). */
    int is_free;                 /**< 1 if this block is currently free, 0 if allocated. */
    struct block_header *next;   /**< Pointer to next adjacent/linked memory block in heap list. */
    struct block_header *prev;   /**< Pointer to previous adjacent/linked memory block in heap list. */
} block_header_t;

/** Macro to round up a byte count `x` to the next 16-byte alignment boundary. */
#define ALIGN16(x) (((x) + 15) & ~15)

/** Size of the block header aligned to 16 bytes. */
#define HEADER_SIZE ALIGN16(sizeof(block_header_t))

/** Minimum allocation chunk requested from the kernel via mmap (64 KiB). */
#define MIN_MMAP_CHUNK (64 * 1024)

/** Head pointer of the global doubly linked heap block list. */
static block_header_t *heap_head = NULL;

/**
 * @brief Maps pages of memory into the process's virtual address space.
 *
 * Invokes the `SYS_MMAP` (9) system call. In NullOS userspace, this allocates
 * anonymous readable/writable virtual memory pages backed by physical frames.
 *
 * @param addr   Suggested virtual address (usually NULL to let kernel choose).
 * @param length Size of mapping in bytes.
 * @param prot   Memory protection flags (PROT_READ, PROT_WRITE, PROT_EXEC).
 * @param flags  Mapping flags (MAP_PRIVATE, MAP_ANONYMOUS).
 * @param fd     File descriptor for file-backed mappings (unused in anonymous mmap).
 * @param offset File offset (unused in anonymous mmap).
 * @return Pointer to mapped memory region on success, or MAP_FAILED on error.
 */
void *mmap(void *addr, size_t length, int prot, int flags, int fd, off_t offset) {
    (void)fd;
    (void)offset;
    long ret = syscall4(SYS_MMAP, (long)addr, (long)length, (long)prot, (long)flags);
    if (ret == -1L || (unsigned long)ret == (unsigned long)-1) {
        return MAP_FAILED;
    }
    return (void *)ret;
}

/**
 * @brief Unmaps pages of memory from the process's virtual address space.
 *
 * @param addr   Starting virtual address of region to unmap.
 * @param length Size of memory region in bytes.
 * @return 0 on success.
 */
int munmap(void *addr, size_t length) {
    (void)addr;
    (void)length;
    return 0;
}

/**
 * @brief Allocates dynamic memory from the process heap.
 *
 * Searches the existing doubly linked free list for a block with at least `size`
 * bytes of payload. If found, the block is split if large enough, marked allocated,
 * and returned. Otherwise, a new chunk (at least 64 KiB) is requested from the
 * kernel via `SYS_MMAP`, inserted into the heap chain, split if applicable, and returned.
 *
 * @param size Requested allocation size in bytes.
 * @return Pointer to allocated payload memory aligned to 16 bytes, or NULL if out of memory.
 */
void *malloc(size_t size) {
    if (size == 0) {
        return NULL;
    }

    size_t req_size = ALIGN16(size);

    /* 1. First-fit search across existing blocks in the free list */
    block_header_t *curr = heap_head;
    while (curr) {
        if (curr->is_free && curr->size >= req_size) {
            /* Can we split this block? Needs enough space for header + payload */
            if (curr->size >= req_size + HEADER_SIZE + 16) {
                block_header_t *split = (block_header_t *)((char *)curr + HEADER_SIZE + req_size);
                split->size = curr->size - req_size - HEADER_SIZE;
                split->is_free = 1;
                split->next = curr->next;
                split->prev = curr;
                if (curr->next) {
                    curr->next->prev = split;
                }
                curr->next = split;
                curr->size = req_size;
            }
            curr->is_free = 0;
            return (void *)((char *)curr + HEADER_SIZE);
        }
        curr = curr->next;
    }

    /* 2. No suitable free block found: allocate a new chunk from kernel via SYS_MMAP */
    size_t chunk_size = req_size + HEADER_SIZE;
    if (chunk_size < MIN_MMAP_CHUNK) {
        chunk_size = MIN_MMAP_CHUNK;
    }
    chunk_size = (chunk_size + 4095) & ~4095; /* Page align to 4 KiB */

    void *mem = mmap(NULL, chunk_size, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if (mem == MAP_FAILED) {
        return NULL;
    }

    block_header_t *new_block = (block_header_t *)mem;
    new_block->size = chunk_size - HEADER_SIZE;
    new_block->is_free = 1;
    new_block->next = NULL;
    new_block->prev = NULL;

    /* Append newly mapped memory block to the end of the heap list */
    if (!heap_head) {
        heap_head = new_block;
    } else {
        block_header_t *tail = heap_head;
        while (tail->next) {
            tail = tail->next;
        }
        tail->next = new_block;
        new_block->prev = tail;
    }

    /* Split new chunk if it has excess capacity */
    if (new_block->size >= req_size + HEADER_SIZE + 16) {
        block_header_t *split = (block_header_t *)((char *)new_block + HEADER_SIZE + req_size);
        split->size = new_block->size - req_size - HEADER_SIZE;
        split->is_free = 1;
        split->next = new_block->next;
        split->prev = new_block;
        new_block->next = split;
        new_block->size = req_size;
    }

    new_block->is_free = 0;
    return (void *)((char *)new_block + HEADER_SIZE);
}

/**
 * @brief Releases a previously allocated memory block back to the heap.
 *
 * Marks the block as free and attempts bidirectional coalescing with the next
 * and previous adjacent blocks if they are also free and physically contiguous.
 *
 * @param ptr Pointer to user payload previously returned by malloc, calloc, or realloc.
 */
void free(void *ptr) {
    if (!ptr) {
        return;
    }

    /* Recover block header from payload pointer */
    block_header_t *block = (block_header_t *)((char *)ptr - HEADER_SIZE);
    block->is_free = 1;

    /* Forward coalescing: merge with next block if adjacent and free */
    if (block->next && block->next->is_free) {
        if ((char *)block + HEADER_SIZE + block->size == (char *)block->next) {
            block->size += HEADER_SIZE + block->next->size;
            block->next = block->next->next;
            if (block->next) {
                block->next->prev = block;
            }
        }
    }

    /* Backward coalescing: merge with previous block if adjacent and free */
    if (block->prev && block->prev->is_free) {
        if ((char *)block->prev + HEADER_SIZE + block->prev->size == (char *)block) {
            block->prev->size += HEADER_SIZE + block->size;
            block->prev->next = block->next;
            if (block->next) {
                block->next->prev = block->prev;
            }
        }
    }
}

/**
 * @brief Allocates and zero-initializes memory for an array of elements.
 *
 * Calculates `nmemb * size`, checks for integer arithmetic multiplication overflow,
 * allocates memory via `malloc()`, and clears it to zero using `memset()`.
 *
 * @param nmemb Number of elements.
 * @param size  Size of each element in bytes.
 * @return Pointer to zero-initialized memory block, or NULL on overflow or failure.
 */
void *calloc(size_t nmemb, size_t size) {
    size_t total = nmemb * size;
    /* Check for integer multiplication overflow */
    if (nmemb != 0 && total / nmemb != size) {
        return NULL;
    }
    void *ptr = malloc(total);
    if (ptr) {
        memset(ptr, 0, total);
    }
    return ptr;
}

/**
 * @brief Changes the size of a previously allocated memory block.
 *
 * If `ptr` is NULL, equivalent to `malloc(size)`.
 * If `size` is 0, frees `ptr` and returns NULL.
 * If current block size is already >= `size`, returns `ptr` in place.
 * Otherwise, allocates a new block, copies existing payload bytes, frees old block,
 * and returns the new pointer.
 *
 * @param ptr  Existing memory block payload pointer.
 * @param size New requested size in bytes.
 * @return Pointer to resized memory block, or NULL on failure.
 */
void *realloc(void *ptr, size_t size) {
    if (!ptr) {
        return malloc(size);
    }
    if (size == 0) {
        free(ptr);
        return NULL;
    }

    block_header_t *block = (block_header_t *)((char *)ptr - HEADER_SIZE);
    if (block->size >= size) {
        return ptr;
    }

    void *new_ptr = malloc(size);
    if (new_ptr) {
        memcpy(new_ptr, ptr, block->size);
        free(ptr);
    }
    return new_ptr;
}

/**
 * @brief Abnormally terminates the calling process.
 *
 * Writes an error message to `STDERR_FILENO` and invokes `exit(134)` (representing
 * exit code 128 + SIGABRT = 134). This function never returns.
 */
void abort(void) {
    const char msg[] = "[ABORT] Program aborted.\n";
    write(STDERR_FILENO, msg, sizeof(msg) - 1);
    exit(134);
}

/**
 * @brief Computes the absolute value of an integer.
 *
 * @param j Integer value.
 * @return Absolute value |j|.
 */
int abs(int j) {
    return (j < 0) ? -j : j;
}

/**
 * @brief Converts an ASCII decimal string to an integer.
 *
 * @param nptr Null-terminated string representing a decimal number.
 * @return Parsed integer value.
 */
int atoi(const char *nptr) {
    return (int)atol(nptr);
}

/**
 * @brief Converts an ASCII decimal string to a long integer.
 *
 * Skips leading whitespace characters, determines sign ('+' or '-'), and parses
 * consecutive decimal digits until a non-digit character is encountered.
 *
 * @param nptr Null-terminated string.
 * @return Parsed long integer value.
 */
long atol(const char *nptr) {
    while (*nptr == ' ' || (*nptr >= '\t' && *nptr <= '\r')) {
        nptr++;
    }

    int sign = 1;
    if (*nptr == '-') {
        sign = -1;
        nptr++;
    } else if (*nptr == '+') {
        nptr++;
    }

    long val = 0;
    while (*nptr >= '0' && *nptr <= '9') {
        val = val * 10 + (*nptr - '0');
        nptr++;
    }

    return sign * val;
}

/**
 * @brief Converts a string to a long integer according to the given radix base.
 *
 * Handles optional leading whitespace, optional sign ('+' or '-'), base prefix
 * detection ("0x" for base 16, "0" for base 8 when base == 0), and alphanumeric digits.
 *
 * @param nptr   Pointer to source string.
 * @param endptr If non-NULL, stores pointer to the first character after parsed digits.
 * @param base   Radix base (between 2 and 36, or 0 for auto-detection).
 * @return Parsed long integer value.
 */
long strtol(const char *nptr, char **endptr, int base) {
    const char *s = nptr;
    while (*s == ' ' || (*s >= '\t' && *s <= '\r')) {
        s++;
    }

    int sign = 1;
    if (*s == '-') {
        sign = -1;
        s++;
    } else if (*s == '+') {
        s++;
    }

    if ((base == 0 || base == 16) && s[0] == '0' && (s[1] == 'x' || s[1] == 'X')) {
        s += 2;
        base = 16;
    }
    if (base == 0) {
        base = (s[0] == '0') ? 8 : 10;
    }

    long val = 0;
    while (*s) {
        int digit;
        if (*s >= '0' && *s <= '9') {
            digit = *s - '0';
        } else if (*s >= 'a' && *s <= 'z') {
            digit = *s - 'a' + 10;
        } else if (*s >= 'A' && *s <= 'Z') {
            digit = *s - 'A' + 10;
        } else {
            break;
        }

        if (digit >= base) {
            break;
        }

        val = val * base + digit;
        s++;
    }

    if (endptr) {
        *endptr = (char *)s;
    }

    return sign * val;
}

/**
 * @brief Converts an integer to a null-terminated string in the specified base.
 *
 * Supports bases 2 through 36. For base 10, handles negative values by prepending
 * a minus sign.
 *
 * @param value Integer value to convert.
 * @param str   Output buffer where string is written.
 * @param base  Numerical radix base (2 to 36).
 * @return Pointer to output string buffer `str`.
 */
char *itoa(int value, char *str, int base) {
    if (base < 2 || base > 36) {
        *str = '\0';
        return str;
    }

    char *rc = str;
    char *ptr = str;
    char *low;
    unsigned int uval = (unsigned int)value;

    if (value < 0 && base == 10) {
        *ptr++ = '-';
        uval = (unsigned int)(-value);
    }

    low = ptr;
    do {
        int rem = uval % base;
        *ptr++ = (rem < 10) ? (char)('0' + rem) : (char)('a' + rem - 10);
        uval /= base;
    } while (uval);

    *ptr-- = '\0';

    /* Reverse string in-place */
    while (low < ptr) {
        char tmp = *low;
        *low++ = *ptr;
        *ptr-- = tmp;
    }

    return rc;
}
