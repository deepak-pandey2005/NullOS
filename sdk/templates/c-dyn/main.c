#include <stdio.h>
#include <dlfcn.h>
#include <nullos/sdk.h>

int main(void) {
    printf("Hello from NullOS Dynamic Libc App (PID=%llu, SDK=%s)\n",
           (unsigned long long)nullos_getpid(), NULLOS_SDK_VERSION_STRING);
    return 0;
}

