/**
 * @file dlfcn.h
 * @brief NullOS Dynamic Linking Interface Definitions (dlopen / dlsym / dlclose / dlerror)
 *
 * This header defines constants and prototypes for dynamic shared library loading
 * and runtime symbol resolution in NullOS userspace, compatible with the POSIX
 * dlfcn.h standard.
 */

#ifndef _DLFCN_H
#define _DLFCN_H

/* ============================================================================
 * Dynamic Linker Binding Modes & Scope Flags
 * ============================================================================ */

/** Perform lazy symbol resolution as code is executed. */
#define RTLD_LAZY   0x00001

/** Resolve all undefined symbols immediately before dlopen returns. */
#define RTLD_NOW    0x00002

/** Make exported symbols available for subsequently loaded shared objects. */
#define RTLD_GLOBAL 0x00100

/** Symbols are not made available to resolve references in other libraries. */
#define RTLD_LOCAL  0x00000

#ifdef __cplusplus
extern "C" {
#endif

/**
 * @brief Opens and loads a dynamic shared library.
 *
 * @param filename File path to the shared object (or NULL to search main executable).
 * @param flags    Binding flags (RTLD_NOW, RTLD_LAZY, RTLD_GLOBAL, etc.).
 * @return Opaque handle pointer to the library, or NULL on error.
 */
void *dlopen(const char *filename, int flags);

/**
 * @brief Resolves a symbol address from a loaded shared library.
 *
 * @param handle Opaque handle returned by dlopen.
 * @param symbol Null-terminated symbol identifier name.
 * @return Virtual address of the resolved function or data symbol, or NULL on error.
 */
void *dlsym(void *handle, const char *symbol);

/**
 * @brief Closes a previously loaded dynamic shared library.
 *
 * @param handle Opaque handle returned by dlopen.
 * @return 0 on success, or non-zero on error.
 */
int dlclose(void *handle);

/**
 * @brief Retrieves a human-readable diagnostic error string from the last dynamic linking call.
 *
 * @return Error message string, or NULL if no error has occurred since last call.
 */
char *dlerror(void);

#ifdef __cplusplus
}
#endif

#endif /* _DLFCN_H */
