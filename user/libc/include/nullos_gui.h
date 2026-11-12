#ifndef _NULLOS_GUI_H
#define _NULLOS_GUI_H

#include <stdint.h>
#include <stddef.h>
#include <string.h>
#include <sys/syscall.h>

#define GUI_EVENT_NONE            0
#define GUI_EVENT_MOUSE_DOWN      1
#define GUI_EVENT_MOUSE_UP        2
#define GUI_EVENT_MOUSE_MOVE      3
#define GUI_EVENT_KEY_DOWN        4
#define GUI_EVENT_CLOSE_REQUESTED 5

typedef struct {
    uint32_t kind;
    int32_t param1;
    int32_t param2;
    int32_t param3;
} nullos_gui_event_t;

static inline int gui_create_window(const char *title, int width, int height) {
    size_t len = title ? strlen(title) : 0;
    return (int)syscall4(SYS_GUI_CREATE_WINDOW, (long)title, (long)len, (long)width, (long)height);
}

static inline int gui_present(int win_id, const uint32_t *pixels, int width, int height) {
    size_t bytes = (size_t)width * (size_t)height * sizeof(uint32_t);
    return (int)syscall3(SYS_GUI_PRESENT, (long)win_id, (long)pixels, (long)bytes);
}

static inline int gui_poll_event(int win_id, nullos_gui_event_t *out_event) {
    return (int)syscall2(SYS_GUI_POLL_EVENT, (long)win_id, (long)out_event);
}

static inline int gui_close_window(int win_id) {
    return (int)syscall1(SYS_GUI_CLOSE_WINDOW, (long)win_id);
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

