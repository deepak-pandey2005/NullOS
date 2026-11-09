/**
 * @file async_test.c
 * @brief NullOS Asynchronous & Event Infrastructure Validation Test Suite (Milestone 29)
 *
 * This userspace integration test validates the high-performance asynchronous event
 * primitives and I/O mechanisms provided by the NullOS kernel:
 *
 * 1. Scatter-Gather Vectored I/O (`writev`):
 *    - Validates that multiple discrete non-contiguous memory buffers (`struct iovec`)
 *      can be atomically written to a file descriptor in a single system call (`SYS_WRITEV`).
 *
 * 2. Event File Descriptors (`eventfd`) and Multiplexed Polling (`poll`):
 *    - Validates 64-bit integer counter event notifications.
 *    - Tests that an initial eventfd with value 0 reports not-ready in `poll()`.
 *    - Tests writing a value (`eventfd_write`) transitions the descriptor to readable (`POLLIN`).
 *    - Tests reading the 64-bit value (`eventfd_read`) resets the counter.
 *
 * 3. Scalable Event Notification Multiplexing (`epoll`):
 *    - Tests creating an epoll instance (`epoll_create1`).
 *    - Tests registering interest in an eventfd via `epoll_ctl` with `EPOLL_CTL_ADD`.
 *    - Tests waiting for events via `epoll_wait`, verifying that custom user data (`u64`)
 *      and ready event flags are preserved across the kernel boundary.
 *
 * 4. High-Resolution Timers (`timerfd`):
 *    - Tests timer file descriptor creation (`timerfd_create`) using `CLOCK_MONOTONIC`.
 *    - Sets a 20 ms timer interval using `timerfd_settime`.
 *    - Waits for expiration notification via `poll()` on `POLLIN`.
 *    - Reads the 64-bit expiration tick counter via `read()` and cleans up resources.
 */

#include <stdio.h>
#include <stdlib.h>
#include <unistd.h>
#include <sys/poll.h>
#include <sys/epoll.h>
#include <sys/eventfd.h>
#include <sys/timerfd.h>
#include <sys/uio.h>

/**
 * @brief Main entry point executing asynchronous subsystem test phases.
 *
 * @param argc Argument count.
 * @param argv Argument vector.
 * @return 0 on test success, 1 on test failure.
 */
int main(int argc, char **argv) {
    (void)argc; (void)argv;
    printf("====================================================\n");
    printf("   NullOS Async & Event Infrastructure Demo (M29)   \n");
    printf("====================================================\n");

    /* ========================================================================
     * Phase 1: Test Scatter-Gather Vectored I/O (writev)
     * ======================================================================== */
    printf("[ASYNC] 1. Testing writev(2) scatter-gather output...\n");
    char part1[] = "Hello ";
    char part2[] = "from ";
    char part3[] = "writev(2) vectored I/O!\n";
    struct iovec iov[3];
    iov[0].iov_base = part1;
    iov[0].iov_len = sizeof(part1) - 1;
    iov[1].iov_base = part2;
    iov[1].iov_len = sizeof(part2) - 1;
    iov[2].iov_base = part3;
    iov[2].iov_len = sizeof(part3) - 1;
    long written = writev(1, iov, 3);
    if (written < 0) {
        printf("[ASYNC ERROR] writev failed: %ld\n", written);
        return 1;
    }
    printf("[ASYNC] writev successfully output %ld bytes.\n", written);

    /* ========================================================================
     * Phase 2: Test eventfd and poll
     * ======================================================================== */
    printf("[ASYNC] 2. Testing eventfd(2) and poll(2)...\n");
    int efd = eventfd(0, 0);
    if (efd < 0) {
        printf("[ASYNC ERROR] eventfd failed: %d\n", efd);
        return 1;
    }

    struct pollfd pfd;
    pfd.fd = efd;
    pfd.events = POLLIN;
    pfd.revents = 0;

    /* Verify that an empty eventfd does not trigger POLLIN */
    int p_res = poll(&pfd, 1, 0);
    if (p_res != 0) {
        printf("[ASYNC ERROR] poll unexpectedly found ready on empty eventfd: %d\n", p_res);
        return 1;
    }

    /* Write a 64-bit counter value to the eventfd */
    eventfd_t val = 42;
    if (eventfd_write(efd, val) != 0) {
        printf("[ASYNC ERROR] eventfd_write failed!\n");
        return 1;
    }

    /* Verify that poll now reports POLLIN */
    p_res = poll(&pfd, 1, 100);
    if (p_res <= 0 || !(pfd.revents & POLLIN)) {
        printf("[ASYNC ERROR] poll failed to report POLLIN on written eventfd!\n");
        return 1;
    }

    /* Read back the 64-bit value and verify counter correctness */
    eventfd_t read_val = 0;
    if (eventfd_read(efd, &read_val) != 0 || read_val != 42) {
        printf("[ASYNC ERROR] eventfd_read returned unexpected value: %llu\n", (unsigned long long)read_val);
        return 1;
    }
    printf("[ASYNC] eventfd written and read back value: %llu\n", (unsigned long long)read_val);

    /* ========================================================================
     * Phase 3: Test Epoll Multiplexing
     * ======================================================================== */
    printf("[ASYNC] 3. Testing epoll_create1(2), epoll_ctl(2), and epoll_wait(2)...\n");
    int epfd = epoll_create1(0);
    if (epfd < 0) {
        printf("[ASYNC ERROR] epoll_create1 failed: %d\n", epfd);
        return 1;
    }

    /* Register eventfd for reading with custom 64-bit user data tag 0xCAFE */
    struct epoll_event ev;
    ev.events = EPOLLIN;
    ev.data.u64 = 0xCAFE;

    if (epoll_ctl(epfd, EPOLL_CTL_ADD, efd, &ev) != 0) {
        printf("[ASYNC ERROR] epoll_ctl ADD failed!\n");
        return 1;
    }

    /* Signal eventfd with counter increment */
    eventfd_write(efd, 100);

    /* Wait for ready event on epoll descriptor */
    struct epoll_event ev_out[2];
    int n_ready = epoll_wait(epfd, ev_out, 2, 100);
    if (n_ready != 1 || ev_out[0].data.u64 != 0xCAFE || !(ev_out[0].events & EPOLLIN)) {
        printf("[ASYNC ERROR] epoll_wait failed or invalid event: n_ready=%d\n", n_ready);
        return 1;
    }
    eventfd_read(efd, &read_val);
    printf("[ASYNC] epoll_wait successfully received event for data: 0x%llx\n", (unsigned long long)ev_out[0].data.u64);

    /* ========================================================================
     * Phase 4: Test timerfd Interval Timing
     * ======================================================================== */
    printf("[ASYNC] 4. Testing timerfd_create(2) and timerfd_settime(2)...\n");
    int tfd = timerfd_create(CLOCK_MONOTONIC, 0);
    if (tfd < 0) {
        printf("[ASYNC ERROR] timerfd_create failed: %d\n", tfd);
        return 1;
    }

    /* Configure 20 ms one-shot timer */
    struct itimerspec ts;
    ts.it_interval.tv_sec = 0;
    ts.it_interval.tv_nsec = 0;
    ts.it_value.tv_sec = 0;
    ts.it_value.tv_nsec = 20000000; /* 20,000,000 ns = 20 ms */

    if (timerfd_settime(tfd, 0, &ts, NULL) != 0) {
        printf("[ASYNC ERROR] timerfd_settime failed!\n");
        return 1;
    }

    /* Wait for timer expiration via poll */
    struct pollfd timer_pfd;
    timer_pfd.fd = tfd;
    timer_pfd.events = POLLIN;
    timer_pfd.revents = 0;

    int timer_res = poll(&timer_pfd, 1, 500);
    if (timer_res <= 0 || !(timer_pfd.revents & POLLIN)) {
        printf("[ASYNC ERROR] poll timed out or failed on timerfd!\n");
        return 1;
    }

    /* Read the 64-bit expiration count from the timer descriptor */
    uint64_t timer_exp = 0;
    if (read(tfd, &timer_exp, sizeof(timer_exp)) != sizeof(timer_exp) || timer_exp == 0) {
        printf("[ASYNC ERROR] failed to read timerfd expirations: %llu\n", (unsigned long long)timer_exp);
        return 1;
    }
    printf("[ASYNC] timerfd successfully expired with %llu expiration tick(s).\n", (unsigned long long)timer_exp);

    /* Clean up all open file descriptors */
    close(efd);
    close(epfd);
    close(tfd);

    printf("[ASYNC] All Async & Event Subsystem tests passed successfully!\n");
    return 0;
}
