#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <fcntl.h>

int main(int argc, char **argv) {
    printf("====================================================\n");
    printf("   Hello from Ring 3 C Userspace on NullOS!        \n");
    printf("====================================================\n");

    /* 1. Verify Process ID */
    pid_t pid = getpid();
    printf("[USER HELLO] Running in isolated process with PID: %ld (argc=%d)\n", (long)pid, argc);
    if (argv && argv[0]) {
        printf("[USER HELLO] Binary argv[0]: \"%s\"\n", argv[0]);
    }

    /* 2. Test String Operations */
    const char *greeting = "NullOS Minimal Libc C Runtime";
    size_t len = strlen(greeting);
    printf("[USER HELLO] Testing strlen(\"%s\") = %lu bytes\n", greeting, (unsigned long)len);

    char copy_buf[64];
    strcpy(copy_buf, greeting);
    int cmp = strcmp(copy_buf, greeting);
    printf("[USER HELLO] Testing strcpy & strcmp: %s (result=%d)\n",
           (cmp == 0) ? "MATCH SUCCESS" : "FAILED", cmp);

    /* 3. Test Dynamic Memory Allocation (malloc, free) */
    printf("[USER HELLO] Testing malloc() dynamic heap allocation...\n");
    char *dyn_buf = (char *)malloc(256);
    if (!dyn_buf) {
        printf("[USER HELLO] ERROR: malloc failed!\n");
        return 1;
    }
    snprintf(dyn_buf, 256, "Dynamic heap memory allocated at pointer %p via SYS_MMAP!", (void *)dyn_buf);
    printf("[USER HELLO] Heap contents: %s\n", dyn_buf);
    free(dyn_buf);
    printf("[USER HELLO] Memory freed successfully.\n");

    /* 4. Test Persistent File I/O via VFS */
    printf("[USER HELLO] Testing file open & read on /disk/README.TXT...\n");
    int fd = open("/disk/README.TXT", O_RDONLY);
    if (fd >= 0) {
        char file_buf[128];
        memset(file_buf, 0, sizeof(file_buf));
        ssize_t bytes_read = read(fd, file_buf, sizeof(file_buf) - 1);
        printf("[USER HELLO] Read %ld bytes from /disk/README.TXT:\n", (long)bytes_read);
        printf("  >> %s\n", file_buf);
        close(fd);
    } else {
        /* Fallback test on /hello.txt in initramfs */
        printf("[USER HELLO] /disk/README.TXT not opened (fd=%d), trying /hello.txt...\n", fd);
        fd = open("/hello.txt", O_RDONLY);
        if (fd >= 0) {
            char file_buf[128];
            memset(file_buf, 0, sizeof(file_buf));
            ssize_t bytes_read = read(fd, file_buf, sizeof(file_buf) - 1);
            printf("[USER HELLO] Read %ld bytes from /hello.txt:\n", (long)bytes_read);
            printf("  >> %s\n", file_buf);
            close(fd);
        }
    }

    printf("====================================================\n");
    printf("   Ring 3 C Userspace Program Completed Successfully!\n");
    printf("====================================================\n");
    return 0;
}
