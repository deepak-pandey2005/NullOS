#include <stdio.h>
#include <errno.h>
#include <nullos/sdk.h>

int main(int argc, char **argv) {
    (void)argc;
    (void)argv;
    errno = 0;
    printf("Hello from NullOS SDK CLI template (version %s, pid=%llu)\n",
           NULLOS_SDK_VERSION_STRING, (unsigned long long)nullos_getpid());
    return 0;
}

