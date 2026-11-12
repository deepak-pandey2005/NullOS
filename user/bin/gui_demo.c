#include <nullos_gui.h>
#include <stdlib.h>
#include <unistd.h>

#define WIN_W 320
#define WIN_H 220

static uint32_t canvas[WIN_W * WIN_H];

static const uint32_t palette[6] = {
    0xFFEF4444, /* Red */
    0xFF22C55E, /* Green */
    0xFF3B82F6, /* Blue */
    0xFFEAB308, /* Yellow */
    0xFF06B6D4, /* Cyan */
    0xFFFFFFFF  /* White */
};

static void clear_canvas(void) {
    gui_fill_rect(canvas, WIN_W, WIN_H, 0, 0, WIN_W, WIN_H, 0xFF0F172A);
    gui_fill_rect(canvas, WIN_W, WIN_H, 0, 0, WIN_W, 28, 0xFF1E293B);
    for (int i = 0; i < 6; i++) {
        gui_fill_rect(canvas, WIN_W, WIN_H, 8 + i * 32, 4, 24, 20, palette[i]);
    }
}

int main(void) {
    int win = gui_create_window("Ring 3 Paint & Bounce", WIN_W, WIN_H);
    if (win < 0) {
        return 1;
    }

    clear_canvas();
    uint32_t active_color = palette[4];
    int drawing = 0;
    int bx = 40, by = 60, vx = 3, vy = 2;

    for (;;) {
        nullos_gui_event_t ev;
        while (gui_poll_event(win, &ev) > 0) {
            if (ev.kind == GUI_EVENT_CLOSE_REQUESTED) {
                gui_close_window(win);
                return 0;
            } else if (ev.kind == GUI_EVENT_KEY_DOWN) {
                if (ev.param1 == 'q' || ev.param1 == 'Q') {
                    gui_close_window(win);
                    return 0;
                } else if (ev.param1 == 'c' || ev.param1 == 'C') {
                    clear_canvas();
                }
            } else if (ev.kind == GUI_EVENT_MOUSE_DOWN) {
                int mx = ev.param1;
                int my = ev.param2;
                if (my < 28) {
                    int idx = (mx - 8) / 32;
                    if (idx >= 0 && idx < 6) {
                        active_color = palette[idx];
                    }
                } else {
                    drawing = 1;
                    gui_fill_rect(canvas, WIN_W, WIN_H, mx - 2, my - 2, 5, 5, active_color);
                }
            } else if (ev.kind == GUI_EVENT_MOUSE_UP) {
                drawing = 0;
            } else if (ev.kind == GUI_EVENT_MOUSE_MOVE && drawing) {
                if (ev.param2 >= 30) {
                    gui_fill_rect(canvas, WIN_W, WIN_H, ev.param1 - 2, ev.param2 - 2, 5, 5, active_color);
                }
            }
        }

        /* Erase old bouncing indicator, step, and draw new indicator */
        gui_fill_rect(canvas, WIN_W, WIN_H, bx, by, 14, 14, 0xFF0F172A);
        bx += vx;
        by += vy;
        if (bx <= 4 || bx + 14 >= WIN_W - 4) vx = -vx;
        if (by <= 32 || by + 14 >= WIN_H - 4) vy = -vy;
        gui_fill_rect(canvas, WIN_W, WIN_H, bx, by, 14, 14, active_color);

        if (gui_present(win, canvas, WIN_W, WIN_H) < 0) {
            return 0;
        }
        usleep(30000);
    }
}

