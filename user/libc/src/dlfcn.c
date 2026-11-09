/**
 * @file dlfcn.c
 * @brief NullOS Dynamic Linker & Shared Object Loading Subsystem (dlopen / dlsym)
 *
 * This file implements the POSIX dynamic linking interface (`dlopen`, `dlsym`,
 * `dlclose`, `dlerror`) in the NullOS C library. It communicates directly with the
 * kernel-level dynamic linker (`kernel/src/task/dynlink.rs`) via dedicated system
 * calls:
 * - `SYS_DLOPEN` (300): Loads an ELF shared object (`.so`), parses dynamic headers,
 *   allocates memory mappings, applies relocations, and returns a library handle.
 * - `SYS_DLSYM` (301): Looks up exported symbols within a loaded library handle's
 *   symbol table (or global scope) and returns the virtual address.
 * - `SYS_DLCLOSE` (302): Decrements library reference count and unloads memory mappings.
 * - `dlerror`: Retrieves human-readable diagnostic error messages from the most
 *   recent dynamic linking operation.
 */

#include <dlfcn.h>
#include <stddef.h>
#include <string.h>

/** System call number for opening/loading a shared library. */
#define SYS_DLOPEN  300

/** System call number for resolving a symbol address in a loaded library. */
#define SYS_DLSYM   301

/** System call number for unloading a shared library. */
#define SYS_DLCLOSE 302

/** System call dispatcher prototype. */
extern long syscall(long number, ...);

/** Thread-local/static buffer storing diagnostic error messages for dlerror(). */
static char dlerror_buf[128];

/** Indicator flag set to 1 when an error message is pending in `dlerror_buf`. */
static int has_dlerror = 0;

/**
 * @brief Loads a dynamic shared library into the calling process's address space.
 *
 * Issues the `SYS_DLOPEN` (300) system call. The kernel resolves the file path,
 * reads ELF program headers, loads `PT_LOAD` segments, parses the `.dynamic` section,
 * and performs symbol relocations.
 *
 * @param filename File path of the shared object (e.g. "/lib/libc.so" or "libc.so").
 * @param flags    Binding flags (e.g. RTLD_NOW, RTLD_LAZY, RTLD_GLOBAL).
 * @return Opaque handle pointer to the loaded library on success, or NULL on failure.
 */
void *dlopen(const char *filename, int flags) {
    (void)flags;
    if (!filename) {
        strcpy(dlerror_buf, "dlopen: NULL filename");
        has_dlerror = 1;
        return NULL;
    }
    long ret = syscall(SYS_DLOPEN, (long)filename, (long)flags, 0, 0, 0, 0);
    if (ret <= 0) {
        strcpy(dlerror_buf, "dlopen: failed to load library");
        has_dlerror = 1;
        return NULL;
    }
    has_dlerror = 0;
    return (void *)ret;
}

/**
 * @brief Obtains the virtual address of a symbol exported by a loaded library.
 *
 * Issues the `SYS_DLSYM` (301) system call. The kernel searches the symbol table
 * (`.dynsym` and `.dynstr`) of the specified shared library handle.
 *
 * @param handle Opaque library handle returned by a previous `dlopen` call.
 * @param symbol Null-terminated string containing the symbol name (function or variable).
 * @return Address of the resolved symbol in virtual memory, or NULL on error.
 */
void *dlsym(void *handle, const char *symbol) {
    if (!symbol) {
        strcpy(dlerror_buf, "dlsym: NULL symbol");
        has_dlerror = 1;
        return NULL;
    }
    long ret = syscall(SYS_DLSYM, (long)handle, (long)symbol, 0, 0, 0, 0);
    if (ret <= 0) {
        strcpy(dlerror_buf, "dlsym: undefined symbol");
        has_dlerror = 1;
        return NULL;
    }
    has_dlerror = 0;
    return (void *)ret;
}

/**
 * @brief Closes a previously opened dynamic shared library.
 *
 * Issues the `SYS_DLCLOSE` (302) system call. Decrements the reference count of
 * the library. If the reference count drops to zero, the library mappings are unmapped.
 *
 * @param handle Opaque library handle returned by `dlopen`.
 * @return 0 on success, or -1 on error.
 */
int dlclose(void *handle) {
    long ret = syscall(SYS_DLCLOSE, (long)handle, 0, 0, 0, 0, 0);
    if (ret != 0) {
        strcpy(dlerror_buf, "dlclose: failed to unload library");
        has_dlerror = 1;
        return -1;
    }
    has_dlerror = 0;
    return 0;
}

/**
 * @brief Returns a human-readable string describing the most recent error.
 *
 * Returns a pointer to an internal static error buffer. Subsequent calls to
 * `dlerror` reset the error status and return NULL until another error occurs.
 *
 * @return String description of last error, or NULL if no error occurred since last call.
 */
char *dlerror(void) {
    if (has_dlerror) {
        has_dlerror = 0;
        return dlerror_buf;
    }
    return NULL;
}
