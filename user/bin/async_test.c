/*
 * NullOS Async & Event Infrastructure Test Program (Milestone 29).
 *
 * This userspace program tests and validates:
 * 1. Scatter-gather vectored I/O via writev(2) across discrete user memory buffers.
 * 2. 64-bit event notification descriptors via eventfd(2) and multiplexed poll(2).
 * 3. Scalable event notification multiplexing via epoll_create1(2), epoll_ctl(2), and epoll_wait(2).
 * 4. High-resolution interval and single-shot timers via timerfd_create(2), timerfd_settime(2), and read(2).
 */

#include <stdio.h>
#include <stdlib.h>
#include <unistd.h>
#include <sys/poll.h>
#include <sys/epoll.h>
#include <sys/eventfd.h>
#include <sys/timerfd.h>
#include <sys/uio.h>

int main(int argc, char **argv) {
    (void)argc; (void)argv;
    printf("====================================================\n");
    printf("   NullOS Async & Event Infrastructure Demo (M29)   \n");
    printf("====================================================\n");

    /* 1. Test Scatter-Gather Vectored I/O (writev) */
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

    /* 2. Test eventfd and poll */
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

    int p_res = poll(&pfd, 1, 0);
    if (p_res != 0) {
        printf("[ASYNC ERROR] poll unexpectedly found ready on empty eventfd: %d\n", p_res);
        return 1;
    }

    eventfd_t val = 42;
    if (eventfd_write(efd, val) != 0) {
        printf("[ASYNC ERROR] eventfd_write failed!\n");
        return 1;
    }

    p_res = poll(&pfd, 1, 100);
    if (p_res <= 0 || !(pfd.revents & POLLIN)) {
        printf("[ASYNC ERROR] poll failed to report POLLIN on written eventfd!\n");
        return 1;
    }

    eventfd_t read_val = 0;
    if (eventfd_read(efd, &read_val) != 0 || read_val != 42) {
        printf("[ASYNC ERROR] eventfd_read returned unexpected value: %llu\n", (unsigned long long)read_val);
        return 1;
    }
    printf("[ASYNC] eventfd written and read back value: %llu\n", (unsigned long long)read_val);

    /* 3. Test Epoll Multiplexing */
    printf("[ASYNC] 3. Testing epoll_create1(2), epoll_ctl(2), and epoll_wait(2)...\n");
    int epfd = epoll_create1(0);
    if (epfd < 0) {
        printf("[ASYNC ERROR] epoll_create1 failed: %d\n", epfd);
        return 1;
    }

    struct epoll_event ev;
    ev.events = EPOLLIN;
    ev.data.u64 = 0xCAFE;

    if (epoll_ctl(epfd, EPOLL_CTL_ADD, efd, &ev) != 0) {
        printf("[ASYNC ERROR] epoll_ctl ADD failed!\n");
        return 1;
    }

    eventfd_write(efd, 100);

    struct epoll_event ev_out[2];
    int n_ready = epoll_wait(epfd, ev_out, 2, 100);
    if (n_ready != 1 || ev_out[0].data.u64 != 0xCAFE || !(ev_out[0].events & EPOLLIN)) {
        printf("[ASYNC ERROR] epoll_wait failed or invalid event: n_ready=%d\n", n_ready);
        return 1;
    }
    eventfd_read(efd, &read_val);
    printf("[ASYNC] epoll_wait successfully received event for data: 0x%llx\n", (unsigned long long)ev_out[0].data.u64);

    /* 4. Test timerfd */
    printf("[ASYNC] 4. Testing timerfd_create(2) and timerfd_settime(2)...\n");
    int tfd = timerfd_create(CLOCK_MONOTONIC, 0);
    if (tfd < 0) {
        printf("[ASYNC ERROR] timerfd_create failed: %d\n", tfd);
        return 1;
    }

    struct itimerspec ts;
    ts.it_interval.tv_sec = 0;
    ts.it_interval.tv_nsec = 0;
    ts.it_value.tv_sec = 0;
    ts.it_value.tv_nsec = 20000000;

    if (timerfd_settime(tfd, 0, &ts, NULL) != 0) {
        printf("[ASYNC ERROR] timerfd_settime failed!\n");
        return 1;
    }

    struct pollfd timer_pfd;
    timer_pfd.fd = tfd;
    timer_pfd.events = POLLIN;
    timer_pfd.revents = 0;

    int timer_res = poll(&timer_pfd, 1, 500);
    if (timer_res <= 0 || !(timer_pfd.revents & POLLIN)) {
        printf("[ASYNC ERROR] poll timed out or failed on timerfd!\n");
        return 1;
    }

    uint64_t timer_exp = 0;
    if (read(tfd, &timer_exp, sizeof(timer_exp)) != sizeof(timer_exp) || timer_exp == 0) {
        printf("[ASYNC ERROR] failed to read timerfd expirations: %llu\n", (unsigned long long)timer_exp);
        return 1;
    }
    printf("[ASYNC] timerfd successfully expired with %llu expiration tick(s).\n", (unsigned long long)timer_exp);

    close(efd);
    close(epfd);
    close(tfd);

    printf("[ASYNC] All Async & Event Subsystem tests passed successfully!\n");
    return 0;
}

