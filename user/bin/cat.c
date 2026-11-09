/**
 * @file cat.c
 * @brief NullOS Standard Concatenate & Print Utility
 *
 * Implements the standard UNIX `cat` command line utility for NullOS:
 * - Validates argument count and displays usage instructions if missing.
 * - Opens the target file path in read-only mode (`O_RDONLY`).
 * - Reads chunks of data into a 512-byte buffer in a loop using `read()`.
 * - Writes the data chunks directly to standard output (`STDOUT_FILENO`) via `write()`.
 * - Closes the file descriptor upon reaching EOF and exits cleanly.
 */

#include <stdio.h>
#include <stdlib.h>
#include <unistd.h>
#include <fcntl.h>

/**
 * @brief Main entry point for the `cat` file concatenation tool.
 *
 * @param argc Argument count.
 * @param argv Argument vector (argv[1] is the target file path).
 * @return 0 on success, 1 on error.
 */
int main(int argc, char **argv) {
    if (argc < 2) {
        printf("Usage: cat <filename>\n");
        return 1;
    }

    const char *path = argv[1];
    int fd = open(path, O_RDONLY);
    if (fd < 0) {
        printf("cat: cannot open '%s'\n", path);
        return 1;
    }

    char buf[512];
    ssize_t n;
    /* Read chunks from the file descriptor and stream them to standard output */
    while ((n = read(fd, buf, sizeof(buf))) > 0) {
        write(STDOUT_FILENO, buf, n);
    }

    close(fd);
    return 0;
}
