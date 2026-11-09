#include <dlfcn.h>
#include <stddef.h>
#include <string.h>

#define SYS_DLOPEN  300
#define SYS_DLSYM   301
#define SYS_DLCLOSE 302

extern long syscall(long number, ...);

static char dlerror_buf[128];
static int has_dlerror = 0;

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

char *dlerror(void) {
    if (has_dlerror) {
        has_dlerror = 0;
        return dlerror_buf;
    }
    return NULL;
}

