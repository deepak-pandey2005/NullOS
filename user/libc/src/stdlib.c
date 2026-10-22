#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <sys/mman.h>
#include <sys/syscall.h>

/* Memory block header for dynamic heap allocation */
typedef struct block_header {
    size_t size;                 /* Size of user payload in bytes */
    int is_free;                 /* 1 if block is free, 0 if allocated */
    struct block_header *next;   /* Next block in memory list */
    struct block_header *prev;   /* Previous block in memory list */
} block_header_t;

#define ALIGN16(x) (((x) + 15) & ~15)
#define HEADER_SIZE ALIGN16(sizeof(block_header_t))
#define MIN_MMAP_CHUNK (64 * 1024) /* 64 KiB */

static block_header_t *heap_head = NULL;

void *mmap(void *addr, size_t length, int prot, int flags, int fd, off_t offset) {
    (void)fd;
    (void)offset;
    long ret = syscall4(SYS_MMAP, (long)addr, (long)length, (long)prot, (long)flags);
    if (ret == -1L || (unsigned long)ret == (unsigned long)-1) {
        return MAP_FAILED;
    }
    return (void *)ret;
}

int munmap(void *addr, size_t length) {
    (void)addr;
    (void)length;
    return 0;
}

void *malloc(size_t size) {
    if (size == 0) {
        return NULL;
    }

    size_t req_size = ALIGN16(size);

    /* Search existing blocks in free list */
    block_header_t *curr = heap_head;
    while (curr) {
        if (curr->is_free && curr->size >= req_size) {
            /* Can we split this block? */
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

    /* No suitable block found, allocate a new chunk from kernel via SYS_MMAP */
    size_t chunk_size = req_size + HEADER_SIZE;
    if (chunk_size < MIN_MMAP_CHUNK) {
        chunk_size = MIN_MMAP_CHUNK;
    }
    chunk_size = (chunk_size + 4095) & ~4095; /* Page align */

    void *mem = mmap(NULL, chunk_size, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if (mem == MAP_FAILED) {
        return NULL;
    }

    block_header_t *new_block = (block_header_t *)mem;
    new_block->size = chunk_size - HEADER_SIZE;
    new_block->is_free = 1;
    new_block->next = NULL;
    new_block->prev = NULL;

    /* Append to heap list */
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

    /* Split if necessary */
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

void free(void *ptr) {
    if (!ptr) {
        return;
    }

    block_header_t *block = (block_header_t *)((char *)ptr - HEADER_SIZE);
    block->is_free = 1;

    /* Coalesce with next block if contiguous */
    if (block->next && block->next->is_free) {
        if ((char *)block + HEADER_SIZE + block->size == (char *)block->next) {
            block->size += HEADER_SIZE + block->next->size;
            block->next = block->next->next;
            if (block->next) {
                block->next->prev = block;
            }
        }
    }

    /* Coalesce with previous block if contiguous */
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

void *calloc(size_t nmemb, size_t size) {
    size_t total = nmemb * size;
    if (nmemb != 0 && total / nmemb != size) {
        return NULL; /* Overflow */
    }
    void *ptr = malloc(total);
    if (ptr) {
        memset(ptr, 0, total);
    }
    return ptr;
}

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

void abort(void) {
    const char msg[] = "[ABORT] Program aborted.\n";
    write(STDERR_FILENO, msg, sizeof(msg) - 1);
    exit(134);
}

int abs(int j) {
    return (j < 0) ? -j : j;
}

int atoi(const char *nptr) {
    return (int)atol(nptr);
}

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

    while (low < ptr) {
        char tmp = *low;
        *low++ = *ptr;
        *ptr-- = tmp;
    }

    return rc;
}
