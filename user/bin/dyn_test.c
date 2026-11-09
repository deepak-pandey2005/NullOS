/**
 * @file dyn_test.c
 * @brief NullOS Dynamic Linker (dlopen/dlsym/dlclose) Userspace Validation Suite
 *
 * This test binary validates Milestone 34 dynamic linking and shared object runtime
 * loading facilities in NullOS:
 * 1. Error Handling on Non-Existent Files: Tests that `dlopen` returns `NULL` and sets
 *    an informative error message in `dlerror()` when attempting to load a missing file.
 * 2. Shared Library Loading: Tests loading `libc.so` dynamically from disk or rootfs
 *    and confirms receipt of a valid non-NULL library handle.
 * 3. Dynamic Symbol Resolution: Resolves function pointers for standard symbols
 *    (`puts`, `malloc`, `free`) via `dlsym()`, verifying symbol lookup in the ELF
 *    `.dynsym` and `.dynstr` tables.
 * 4. Error Handling on Undefined Symbols: Verifies that searching for a non-existent
 *    symbol returns `NULL` and reports an error via `dlerror()`.
 * 5. Library Unloading: Invokes `dlclose()` to release the library handle, decrement
 *    its reference count, and unmap memory mappings.
 */

#include <stdio.h>
#include <dlfcn.h>
#include <string.h>

/**
 * @brief Main entry point executing dynamic linker validation phases.
 *
 * @param argc Argument count.
 * @param argv Argument vector.
 * @return 0 on test success, 1 on test failure.
 */
int main(int argc, char **argv) {
    (void)argc;
    (void)argv;
    printf("[DYN_TEST] Starting Dynamic Linker & Shared Object user test...\n");

    /* Phase 1: Test dlopen with nonexistent file to verify error reporting */
    printf("[DYN_TEST] 1. Testing dlopen with non-existent library...\n");
    void *bad_handle = dlopen("/nonexistent.so", RTLD_NOW);
    if (bad_handle != NULL) {
        printf("[DYN_TEST] FAIL: Expected NULL for non-existent library\n");
        return 1;
    }
    printf("[DYN_TEST] PASS: Correctly failed: %s\n", dlerror());

    /* Phase 2: Test dlopen on libc.so (try /disk/LIBC.SO first, fallback to /lib/libc.so or libc.so) */
    printf("[DYN_TEST] 2. Testing dlopen on libc.so...\n");
    void *handle = dlopen("/disk/LIBC.SO", RTLD_NOW);
    if (!handle) {
        handle = dlopen("/lib/libc.so", RTLD_NOW);
    }
    if (!handle) {
        handle = dlopen("libc.so", RTLD_NOW);
    }
    if (!handle) {
        printf("[DYN_TEST] FAIL: dlopen failed: %s\n", dlerror());
        return 1;
    }
    printf("[DYN_TEST] PASS: dlopen successfully loaded library at handle %p\n", handle);

    /* Phase 3: Test dlsym resolution for 'puts' function pointer */
    printf("[DYN_TEST] 3. Testing dlsym for 'puts'...\n");
    typedef int (*puts_fn)(const char *);
    puts_fn dyn_puts = (puts_fn)dlsym(handle, "puts");
    if (!dyn_puts) {
        printf("[DYN_TEST] FAIL: dlsym failed for puts: %s\n", dlerror());
        dlclose(handle);
        return 1;
    }
    printf("[DYN_TEST] PASS: dlsym resolved 'puts' -> %p\n", (void *)dyn_puts);

    /* Phase 4: Test dlsym resolution for memory management functions ('malloc' and 'free') */
    printf("[DYN_TEST] 4. Testing dlsym for 'malloc' and 'free'...\n");
    typedef void *(*malloc_fn)(size_t);
    typedef void (*free_fn)(void *);
    malloc_fn dyn_malloc = (malloc_fn)dlsym(handle, "malloc");
    free_fn dyn_free = (free_fn)dlsym(handle, "free");
    if (!dyn_malloc || !dyn_free) {
        printf("[DYN_TEST] FAIL: Failed to resolve malloc or free: %s\n", dlerror());
        dlclose(handle);
        return 1;
    }
    printf("[DYN_TEST] PASS: dlsym resolved 'malloc' (%p) and 'free' (%p)\n", (void *)dyn_malloc, (void *)dyn_free);

    /* Phase 5: Test dlsym resolution for nonexistent symbol */
    printf("[DYN_TEST] 5. Testing dlsym for nonexistent symbol...\n");
    void *bad_sym = dlsym(handle, "undefined_symbol_xyz");
    if (bad_sym != NULL) {
        printf("[DYN_TEST] FAIL: Expected NULL for nonexistent symbol\n");
        dlclose(handle);
        return 1;
    }
    printf("[DYN_TEST] PASS: Correctly failed: %s\n", dlerror());

    /* Phase 6: Test dlclose library handle deallocation */
    printf("[DYN_TEST] 6. Testing dlclose...\n");
    int close_ret = dlclose(handle);
    if (close_ret != 0) {
        printf("[DYN_TEST] FAIL: dlclose failed: %s\n", dlerror());
        return 1;
    }
    printf("[DYN_TEST] PASS: dlclose unloaded library successfully\n");

    printf("[DYN_TEST] All Milestone 34 Dynamic Linker user tests passed!\n");
    return 0;
}
