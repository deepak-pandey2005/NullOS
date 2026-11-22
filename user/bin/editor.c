/*
 * NullOS Ring 3 Graphical Text Editor & File Viewer
 *
 * Runs as a Wayland/IPC client to /bin/compositor.elf:
 *   - Creates an interactive 580x360 window
 *   - Supports viewing and editing files (/disk/README.TXT, /disk/NOTES.TXT, /hello.txt, /tmp/test.txt)
 *   - Toolbar with quick file buttons (README, NOTES, HELLO, NEW) and Save button
 *   - Keyboard input with cursor navigation, backspace, enter, and text editing
 *   - Status bar showing file path, cursor position (line/col), and save confirmation
 */

#include <nullos_gui.h>
#include <unistd.h>
#include <fcntl.h>
#include <string.h>
#include <stdio.h>
#include <stdlib.h>
#include <font8x8.h>

#define WIN_W 580
#define WIN_H 360

#define TOOLBAR_H 32
#define STATUSBAR_H 22
#define EDITOR_Y (TOOLBAR_H)
#define EDITOR_H (WIN_H - TOOLBAR_H - STATUSBAR_H)

#define FONT_W 8
#define FONT_H 8
#define PAD_X 10
#define PAD_Y 6

#define MAX_TEXT_LEN 4096
#define MAX_PATH_LEN 64

#define COL_BG         0xFF0F172A
#define COL_TOOLBAR    0xFF1E293B
#define COL_STATUS     0xFF090D16
#define COL_BTN        0xFF334155
#define COL_BTN_ACT    0xFF2563EB
#define COL_SAVE_BTN   0xFF16A34A
#define COL_TEXT       0xFFE2E8F0
#define COL_MUTED      0xFF94A3B8
#define COL_BORDER     0xFF475569
#define COL_CURSOR     0xFF38BDF8
#define COL_ACCENT     0xFFF59E0B

static char g_text[MAX_TEXT_LEN];
static int g_text_len = 0;
static int g_cursor = 0;
static char g_cur_path[MAX_PATH_LEN] = "/disk/README.TXT";
static char g_status_msg[64] = "Ready";

static void draw_char(uint32_t *pixels, int x, int y, char c, uint32_t fg) {
    if (c < 0x20 || c > 0x7E) c = '?';
    const uint8_t *glyph = font8x8_basic[c - 0x20];
    for (int r = 0; r < 8; r++) {
        int py = y + r;
        if (py < 0 || py >= WIN_H) continue;
        uint8_t byte = glyph[r];
        for (int b = 0; b < 8; b++) {
            if (byte & (0x80 >> b)) {
                int px = x + b;
                if (px >= 0 && px < WIN_W) {
                    pixels[py * WIN_W + px] = fg;
                }
            }
        }
    }
}

static void draw_string(uint32_t *pixels, int x, int y, const char *str, uint32_t fg, int max_w) {
    int cx = x;
    while (*str) {
        if (max_w > 0 && (cx - x + FONT_W) > max_w) break;
        draw_char(pixels, cx, y, *str++, fg);
        cx += FONT_W + 1;
    }
}

static void load_file(const char *path) {
    strncpy(g_cur_path, path, MAX_PATH_LEN - 1);
    g_cur_path[MAX_PATH_LEN - 1] = '\0';
    g_text_len = 0;
    g_cursor = 0;

    int fd = open(path, O_RDONLY);
    if (fd < 0) {
        snprintf(g_status_msg, sizeof(g_status_msg), "New file: %s", path);
        g_text[0] = '\0';
        return;
    }

    ssize_t n = read(fd, g_text, MAX_TEXT_LEN - 1);
    if (n < 0) n = 0;
    g_text[n] = '\0';
    g_text_len = (int)n;
    close(fd);

    snprintf(g_status_msg, sizeof(g_status_msg), "Loaded: %s (%d bytes)", path, g_text_len);
}

static void save_file(void) {
    int fd = open(g_cur_path, O_WRONLY | O_CREAT | O_TRUNC);
    if (fd < 0) {
        snprintf(g_status_msg, sizeof(g_status_msg), "Save failed: cannot open %s", g_cur_path);
        return;
    }
    ssize_t written = write(fd, g_text, g_text_len);
    close(fd);
    if (written == g_text_len) {
        snprintf(g_status_msg, sizeof(g_status_msg), "Saved %d bytes to %s", g_text_len, g_cur_path);
    } else {
        snprintf(g_status_msg, sizeof(g_status_msg), "Partial save (%ld/%d bytes)", (long)written, g_text_len);
    }
}

static void render_editor(nullos_gui_window_t *win) {
    uint32_t *p = win->pixels;

    /* 1. Background */
    gui_fill_rect(p, WIN_W, WIN_H, 0, 0, WIN_W, WIN_H, COL_BG);

    /* 2. Toolbar */
    gui_fill_rect(p, WIN_W, WIN_H, 0, 0, WIN_W, TOOLBAR_H, COL_TOOLBAR);
    gui_fill_rect(p, WIN_W, WIN_H, 0, TOOLBAR_H - 1, WIN_W, 1, COL_BORDER);

    /* Quick Preset Buttons */
    const char *btns[4] = { "README", "NOTES", "HELLO", "SAVE" };
    int bx = 8;
    for (int i = 0; i < 4; i++) {
        int bw = 60;
        uint32_t col = (i == 3) ? COL_SAVE_BTN : COL_BTN;
        gui_fill_rect(p, WIN_W, WIN_H, bx, 4, bw, 24, col);
        draw_string(p, bx + 8, 12, btns[i], 0xFFFFFFFF, bw - 8);
        bx += bw + 8;
    }

    /* Path display on toolbar */
    draw_string(p, bx + 10, 12, "File: ", COL_MUTED, 50);
    draw_string(p, bx + 55, 12, g_cur_path, COL_ACCENT, WIN_W - bx - 60);

    /* 3. Text Area */
    int cx = PAD_X;
    int cy = EDITOR_Y + PAD_Y;
    int max_cx = WIN_W - PAD_X;
    int cur_screen_x = cx;
    int cur_screen_y = cy;

    for (int i = 0; i < g_text_len; i++) {
        if (i == g_cursor) {
            cur_screen_x = cx;
            cur_screen_y = cy;
        }

        char c = g_text[i];
        if (c == '\n') {
            cx = PAD_X;
            cy += FONT_H + 4;
            continue;
        }

        if (cx + FONT_W > max_cx) {
            cx = PAD_X;
            cy += FONT_H + 4;
        }

        if (cy + FONT_H <= EDITOR_Y + EDITOR_H) {
            draw_char(p, cx, cy, c, COL_TEXT);
        }
        cx += FONT_W + 1;
    }

    if (g_cursor == g_text_len) {
        cur_screen_x = cx;
        cur_screen_y = cy;
    }

    /* Render Cursor */
    if (cur_screen_y + FONT_H <= EDITOR_Y + EDITOR_H) {
        gui_fill_rect(p, WIN_W, WIN_H, cur_screen_x, cur_screen_y, 2, FONT_H + 2, COL_CURSOR);
    }

    /* 4. Status Bar */
    int sby = WIN_H - STATUSBAR_H;
    gui_fill_rect(p, WIN_W, WIN_H, 0, sby, WIN_W, STATUSBAR_H, COL_STATUS);
    gui_fill_rect(p, WIN_W, WIN_H, 0, sby, WIN_W, 1, COL_BORDER);

    draw_string(p, 10, sby + 7, g_status_msg, COL_MUTED, 360);

    char pos_str[32];
    snprintf(pos_str, sizeof(pos_str), "Pos: %d/%d", g_cursor, g_text_len);
    draw_string(p, WIN_W - 120, sby + 7, pos_str, COL_TEXT, 110);
}

int main(void) {
    nullos_gui_window_t win;
    if (gui_window_open(&win, "NullOS Text Editor", WIN_W, WIN_H) < 0) {
        return 1;
    }

    load_file("/disk/README.TXT");
    render_editor(&win);
    gui_window_present(&win);

    for (;;) {
        nullos_gui_event_t ev;
        int needs_render = 0;

        while (gui_window_poll_event(&win, &ev) > 0) {
            if (ev.kind == GUI_EVENT_CLOSE_REQUESTED) {
                gui_window_close(&win);
                return 0;
            } else if (ev.kind == GUI_EVENT_MOUSE_DOWN) {
                int mx = ev.param1;
                int my = ev.param2;

                /* Check Toolbar clicks */
                if (my >= 4 && my <= 28) {
                    if (mx >= 8 && mx < 68) {
                        load_file("/disk/README.TXT");
                        needs_render = 1;
                    } else if (mx >= 76 && mx < 136) {
                        load_file("/disk/NOTES.TXT");
                        needs_render = 1;
                    } else if (mx >= 144 && mx < 204) {
                        load_file("/hello.txt");
                        needs_render = 1;
                    } else if (mx >= 212 && mx < 272) {
                        save_file();
                        needs_render = 1;
                    }
                }
            } else if (ev.kind == GUI_EVENT_KEY_DOWN) {
                uint32_t key = (uint32_t)ev.param1;

                if (key == '\b' || key == 127) {
                    /* Backspace */
                    if (g_cursor > 0) {
                        memmove(&g_text[g_cursor - 1], &g_text[g_cursor], g_text_len - g_cursor);
                        g_cursor--;
                        g_text_len--;
                        g_text[g_text_len] = '\0';
                        needs_render = 1;
                    }
                } else if (key == '\n' || key == '\r') {
                    /* Enter */
                    if (g_text_len < MAX_TEXT_LEN - 2) {
                        memmove(&g_text[g_cursor + 1], &g_text[g_cursor], g_text_len - g_cursor);
                        g_text[g_cursor] = '\n';
                        g_cursor++;
                        g_text_len++;
                        g_text[g_text_len] = '\0';
                        needs_render = 1;
                    }
                } else if (key >= 0x20 && key <= 0x7E) {
                    /* Printable character */
                    if (g_text_len < MAX_TEXT_LEN - 2) {
                        memmove(&g_text[g_cursor + 1], &g_text[g_cursor], g_text_len - g_cursor);
                        g_text[g_cursor] = (char)key;
                        g_cursor++;
                        g_text_len++;
                        g_text[g_text_len] = '\0';
                        needs_render = 1;
                    }
                }
            }
        }

        if (needs_render) {
            render_editor(&win);
            if (gui_window_present(&win) < 0) {
                gui_window_close(&win);
                return 0;
            }
        }

        usleep(15000);
    }
}
