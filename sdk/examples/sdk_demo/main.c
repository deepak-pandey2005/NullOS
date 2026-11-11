/**
 * @file sdk_demo/main.c
 * @brief NullOS SDK End-to-End Verification Application (Phase 18 - M39)
 *
 * Built via `sdk/bin/nullos-cc`, `sdk/sdk.mk`, `sdk/crt/crt0.s`, and `sdk/ld/nullos-app.ld`,
 * and packaged into `sdk_demo.npkg` for verification by the NullOS kernel.
 */

#include <stdio.h>
#include <stdlib.h>
#include <errno.h>
#include <nullos/sdk.h>
#include <nullos/pkg.h>

static int ctor_ran = 0;

__attribute__((constructor))
static void sdk_init_hook(void) {
    ctor_ran = 1;
}

int main(int argc, char **argv) {
    (void)argc;
    (void)argv;
    uint64_t t0 = nullos_rdtsc();
    errno = EINVAL;

    void *buf = malloc(128);
    uint64_t pid = nullos_getpid();
    uint64_t t1 = nullos_rdtsc();

    printf("====================================================\n");
    printf("      NullOS SDK Demo Application (M39)             \n");
    printf("====================================================\n");
    printf("[SDK_DEMO] Version      : %s\n", NULLOS_SDK_VERSION_STRING);
    printf("[SDK_DEMO] PID (fast)   : %llu | init_array ctor=%d | errno=%d\n",
           (unsigned long long)pid, ctor_ran, errno);
    printf("[SDK_DEMO] NPKG Magic   : 0x%08x (hdr=%uB)\n",
           NPKG_MAGIC, (unsigned)sizeof(NpkgHeader));
    printf("[SDK_DEMO] malloc(128)  : %p | rdtsc delta=%llu cyc\n",
           buf, (unsigned long long)(t1 - t0));

    free(buf);
    if (!ctor_ran || buf == NULL) {
        printf("[SDK_DEMO] FAILED checks!\n");
        return 1;
    }
    printf("[SDK_DEMO] All M39 SDK runtime & ABI checks PASSED!\n");
    return 0;
}

