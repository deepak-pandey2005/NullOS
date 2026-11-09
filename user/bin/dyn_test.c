#include <stdio.h>
#include <dlfcn.h>
#include <string.h>

int main(int argc, char **argv) {
    (void)argc;
    (void)argv;
    printf("[DYN_TEST] Starting Dynamic Linker & Shared Object user test...\n");

    // 1. Test dlopen with nonexistent file
    printf("[DYN_TEST] 1. Testing dlopen with non-existent library...\n");
    void *bad_handle = dlopen("/nonexistent.so", RTLD_NOW);
    if (bad_handle != NULL) {
        printf("[DYN_TEST] FAIL: Expected NULL for non-existent library\n");
        return 1;
    }
    printf("[DYN_TEST] PASS: Correctly failed: %s\n", dlerror());

    // 2. Test dlopen on libc.so (try /disk/LIBC.SO first, fallback to /lib/libc.so or libc.so)
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

    // 3. Test dlsym for puts
    printf("[DYN_TEST] 3. Testing dlsym for 'puts'...\n");
    typedef int (*puts_fn)(const char *);
    puts_fn dyn_puts = (puts_fn)dlsym(handle, "puts");
    if (!dyn_puts) {
        printf("[DYN_TEST] FAIL: dlsym failed for puts: %s\n", dlerror());
        dlclose(handle);
        return 1;
    }
    printf("[DYN_TEST] PASS: dlsym resolved 'puts' -> %p\n", (void *)dyn_puts);

    // 4. Test dlsym for memory allocation functions
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

    // 5. Test dlsym for nonexistent symbol
    printf("[DYN_TEST] 5. Testing dlsym for nonexistent symbol...\n");
    void *bad_sym = dlsym(handle, "undefined_symbol_xyz");
    if (bad_sym != NULL) {
        printf("[DYN_TEST] FAIL: Expected NULL for nonexistent symbol\n");
        dlclose(handle);
        return 1;
    }
    printf("[DYN_TEST] PASS: Correctly failed: %s\n", dlerror());

    // 6. Test dlclose
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

