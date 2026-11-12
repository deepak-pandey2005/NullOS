#ifndef _NULLOS_GUI_H
#define _NULLOS_GUI_H

#include <stdint.h>
#include <stddef.h>
#include <string.h>
#include <unistd.h>
#include <sys/syscall.h>

#define COMPOSITOR_SOCK_PATH "/tmp/compositor.sock"
#define GUI_MSG_MAGIC        0x4E475549u /* 'NGUI' */

/* Kernel fbdev / evdev / IPC wrappers */
typedef struct {
    uint32_t width;
    uint32_t height;
    uint32_t stride;
    uint32_t bpp;
} nullos_fb_info_t;

#define RAW_INPUT_MOUSE_MOVE 1
#define RAW_INPUT_MOUSE_DOWN 2
#define RAW_INPUT_MOUSE_UP   3
#define RAW_INPUT_KEY        4

typedef struct {
    uint32_t kind;
    int32_t x;
    int32_t y;
    int32_t dx;
    int32_t dy;
    uint32_t button;
    uint32_t key;
    uint32_t pressed;
} nullos_raw_input_t;

static inline uint32_t *fb_map(nullos_fb_info_t *out_info) {
    long addr = syscall1(SYS_FB_MAP, (long)out_info);
    return addr > 0 ? (uint32_t *)(uintptr_t)addr : NULL;
}

static inline void fb_release(void) {
    syscall0(SYS_FB_RELEASE);
}

static inline int input_poll(nullos_raw_input_t *out_ev) {
    return (int)syscall1(SYS_INPUT_POLL, (long)out_ev);
}

static inline long sys_shmget(uint64_t key, size_t size, int create) {
    return syscall3(SYS_SHMGET, (long)key, (long)size, (long)create);
}

static inline void *sys_shmat(uint64_t shm_id, size_t *out_size) {
    long addr = syscall2(SYS_SHMAT, (long)shm_id, (long)out_size);
    return addr > 0 ? (void *)(uintptr_t)addr : NULL;
}

static inline int unix_listen(const char *path) {
    return (int)syscall2(SYS_UNIX_LISTEN, (long)path, (long)strlen(path));
}

static inline int unix_accept(const char *path) {
    return (int)syscall2(SYS_UNIX_ACCEPT, (long)path, (long)strlen(path));
}

static inline int unix_connect(const char *path) {
    return (int)syscall2(SYS_UNIX_CONNECT, (long)path, (long)strlen(path));
}

static inline int sys_spawn(const char *path) {
    return (int)syscall2(SYS_SPAWN, (long)path, (long)strlen(path));
}

/* Wayland-style Ring 3 Client <-> Compositor Protocol over AF_UNIX + SHM */
#define GUI_MSG_CREATE_WINDOW 1
#define GUI_MSG_CREATED       2
#define GUI_MSG_COMMIT        3
#define GUI_MSG_CLOSE         4
#define GUI_MSG_EVENT         5

#define GUI_EVENT_NONE            0
#define GUI_EVENT_MOUSE_DOWN      1
#define GUI_EVENT_MOUSE_UP        2
#define GUI_EVENT_MOUSE_MOVE      3
#define GUI_EVENT_KEY_DOWN        4
#define GUI_EVENT_CLOSE_REQUESTED 5
#define GUI_EVENT_RESIZED         6

typedef struct {
    uint32_t kind;
    int32_t param1;
    int32_t param2;
    int32_t param3;
} nullos_gui_event_t;

typedef struct {
    uint32_t magic;
    uint32_t kind;
    uint32_t win_id;
    int32_t a;
    int32_t b;
    uint64_t shm_id;
    char title[32];
    nullos_gui_event_t event;
} nullos_gui_msg_t;

typedef struct {
    int sock_fd;
    uint32_t win_id;
    uint32_t *pixels;
    int width;
    int height;
} nullos_gui_window_t;

static inline int gui_window_open(nullos_gui_window_t *win, const char *title, int width, int height) {
    if (!win || width <= 0 || height <= 0) return -1;
    int fd = unix_connect(COMPOSITOR_SOCK_PATH);
    if (fd < 0) return -1;

    size_t buf_bytes = (size_t)width * (size_t)height * sizeof(uint32_t);
    uint64_t key = ((uint64_t)getpid() << 32) ^ (uint64_t)(uintptr_t)win ^ 0x574C5F31u;
    long shm_id = sys_shmget(key, buf_bytes, 1);
    if (shm_id < 0) {
        close(fd);
        return -1;
    }

    size_t mapped_sz = 0;
    uint32_t *pixels = (uint32_t *)sys_shmat((uint64_t)shm_id, &mapped_sz);
    if (!pixels) {
        close(fd);
        return -1;
    }

    nullos_gui_msg_t msg;
    memset(&msg, 0, sizeof(msg));
    msg.magic = GUI_MSG_MAGIC;
    msg.kind = GUI_MSG_CREATE_WINDOW;
    msg.a = width;
    msg.b = height;
    msg.shm_id = (uint64_t)shm_id;
    if (title) {
        size_t tlen = strlen(title);
        if (tlen > 31) tlen = 31;
        memcpy(msg.title, title, tlen);
    }

    if (write(fd, &msg, sizeof(msg)) != (ssize_t)sizeof(msg)) {
        close(fd);
        return -1;
    }

    /* Wait briefly for CREATED reply with assigned win_id */
    nullos_gui_msg_t reply;
    for (int tries = 0; tries < 200; tries++) {
        ssize_t n = read(fd, &reply, sizeof(reply));
        if (n == (ssize_t)sizeof(reply) && reply.magic == GUI_MSG_MAGIC && reply.kind == GUI_MSG_CREATED) {
            win->sock_fd = fd;
            win->win_id = reply.win_id;
            win->pixels = pixels;
            win->width = width;
            win->height = height;
            return 0;
        }
        usleep(5000);
    }

    close(fd);
    return -1;
}

static inline int gui_window_present(nullos_gui_window_t *win) {
    if (!win || win->sock_fd < 0) return -1;
    nullos_gui_msg_t msg;
    memset(&msg, 0, sizeof(msg));
    msg.magic = GUI_MSG_MAGIC;
    msg.kind = GUI_MSG_COMMIT;
    msg.win_id = win->win_id;
    msg.a = win->width;
    msg.b = win->height;
    return write(win->sock_fd, &msg, sizeof(msg)) == (ssize_t)sizeof(msg) ? 0 : -1;
}

static inline int gui_window_poll_event(nullos_gui_window_t *win, nullos_gui_event_t *out_ev) {
    if (!win || win->sock_fd < 0 || !out_ev) return -1;
    nullos_gui_msg_t msg;
    ssize_t n = read(win->sock_fd, &msg, sizeof(msg));
    if (n == (ssize_t)sizeof(msg) && msg.magic == GUI_MSG_MAGIC) {
        if (msg.kind == GUI_MSG_EVENT) {
            *out_ev = msg.event;
            return 1;
        } else if (msg.kind == GUI_MSG_CLOSE) {
            out_ev->kind = GUI_EVENT_CLOSE_REQUESTED;
            return 1;
        }
    }
    return 0;
}

static inline void gui_window_close(nullos_gui_window_t *win) {
    if (!win || win->sock_fd < 0) return;
    nullos_gui_msg_t msg;
    memset(&msg, 0, sizeof(msg));
    msg.magic = GUI_MSG_MAGIC;
    msg.kind = GUI_MSG_CLOSE;
    msg.win_id = win->win_id;
    (void)write(win->sock_fd, &msg, sizeof(msg));
    close(win->sock_fd);
    win->sock_fd = -1;
}

static inline void gui_fill_rect(uint32_t *pixels, int buf_w, int buf_h, int x, int y, int w, int h, uint32_t argb) {
    for (int py = y; py < y + h; py++) {
        if (py < 0 || py >= buf_h) continue;
        for (int px = x; px < x + w; px++) {
            if (px < 0 || px >= buf_w) continue;
            pixels[py * buf_w + px] = argb;
        }
    }
}

#endif /* _NULLOS_GUI_H */
