/*
 * NullOS Ring 3 Userspace Display Server & Desktop Compositor
 *
 * Linux-style architecture:
 *   - Maps /dev/fb0 directly via SYS_FB_MAP (no WM logic in Ring 0)
 *   - Polls raw hardware input events via SYS_INPUT_POLL (evdev-style)
 *   - Listens on /tmp/compositor.sock via AF_UNIX for GUI clients
 *   - Attaches client pixel surfaces zero-copy via SYS_SHMAT
 *   - Composites windows with cached wallpaper, dirty-rect VRAM flushes,
 *     drop shadows, minimize/maximize/resize, Start Menu, and desktop icons.
 */

#include <nullos_gui.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include "font8x8.h"

#define MAX_WINDOWS     16
#define TASKBAR_H       32
#define TITLEBAR_H      26
#define BORDER_W        2
#define BTN_SIZE        16

#define COL_DARK_SLATE  0xFF0F172A
#define COL_PANEL_BG    0xFF1E293B
#define COL_TITLE_BAR   0xFF334155
#define COL_TITLE_ACT   0xFF2563EB
#define COL_LIGHT_SLATE 0xFFE2E8F0
#define COL_MUTED       0xFF94A3B8
#define COL_BORDER      0xFF475569
#define COL_CLOSE       0xFFDC2626
#define COL_TASKBAR     0xFF090D16
#define COL_CYAN        0xFF06B6D4
#define COL_GREEN       0xFF22C55E
#define COL_YELLOW      0xFFEAB308
#define COL_WHITE       0xFFFFFFFF
#define COL_BLACK       0xFF000000

typedef struct {
    int used;
    uint32_t id;
    int client_fd;
    int is_builtin_info;
    char title[32];
    int x, y;
    int width, height;
    int saved_x, saved_y, saved_w, saved_h;
    int minimized;
    int maximized;
    int focused;
    uint32_t *pixels;
} comp_window_t;

static nullos_fb_info_t g_fb;
static uint32_t *g_vram = NULL;
static uint32_t *g_back = NULL;
static uint32_t *g_wall = NULL;

static comp_window_t g_wins[MAX_WINDOWS];
static int g_zorder[MAX_WINDOWS];
static int g_win_count = 0;
static uint32_t g_next_win_id = 1;

static int g_mouse_x = 400;
static int g_mouse_y = 300;
static int g_start_open = 0;
static int g_running = 1;

static int g_drag_idx = -1;
static int g_drag_ox = 0, g_drag_oy = 0;
static int g_resize_idx = -1;
static int g_resize_mx = 0, g_resize_my = 0, g_resize_w = 0, g_resize_h = 0;

static int g_dirty = 1;
static int g_dmg_x0 = 0, g_dmg_y0 = 0, g_dmg_x1 = 0, g_dmg_y1 = 0;
static uint64_t g_frames = 0;
static uint64_t g_partials = 0;

static const char *CURSOR_BMP[16] = {
    "X...........",
    "XX..........",
    "XWX.........",
    "XWWX........",
    "XWWWX.......",
    "XWWWWX......",
    "XWWWWWX.....",
    "XWWWWWWX....",
    "XWWWWWWWX...",
    "XWWWWWWWWX..",
    "XWWWWWXXXXX.",
    "XWWXWWX.....",
    "XWX.XWWX....",
    "XX..XWWX....",
    "X....XWWX...",
    "......XX....",
};

static void mark_dirty_rect(int x, int y, int w, int h) {
    if (w <= 0 || h <= 0) return;
    int x0 = x < 0 ? 0 : x;
    int y0 = y < 0 ? 0 : y;
    int x1 = x + w;
    int y1 = y + h;
    if (x1 > (int)g_fb.width) x1 = (int)g_fb.width;
    if (y1 > (int)g_fb.height) y1 = (int)g_fb.height;
    if (x0 >= x1 || y0 >= y1) return;

    if (!g_dirty || g_dmg_x0 >= g_dmg_x1) {
        g_dmg_x0 = x0; g_dmg_y0 = y0; g_dmg_x1 = x1; g_dmg_y1 = y1;
    } else {
        if (x0 < g_dmg_x0) g_dmg_x0 = x0;
        if (y0 < g_dmg_y0) g_dmg_y0 = y0;
        if (x1 > g_dmg_x1) g_dmg_x1 = x1;
        if (y1 > g_dmg_y1) g_dmg_y1 = y1;
    }
    g_dirty = 1;
}

static void mark_full_dirty(void) {
    g_dmg_x0 = 0;
    g_dmg_y0 = 0;
    g_dmg_x1 = (int)g_fb.width;
    g_dmg_y1 = (int)g_fb.height;
    g_dirty = 1;
}

static void fill_rect_buf(uint32_t *buf, int bw, int bh, int x, int y, int w, int h, uint32_t col) {
    int x0 = x < 0 ? 0 : x;
    int y0 = y < 0 ? 0 : y;
    int x1 = x + w; if (x1 > bw) x1 = bw;
    int y1 = y + h; if (y1 > bh) y1 = bh;
    for (int py = y0; py < y1; py++) {
        uint32_t *row = &buf[py * bw];
        for (int px = x0; px < x1; px++) {
            row[px] = col;
        }
    }
}

static void draw_rect_outline(uint32_t *buf, int bw, int bh, int x, int y, int w, int h, uint32_t col) {
    fill_rect_buf(buf, bw, bh, x, y, w, 1, col);
    fill_rect_buf(buf, bw, bh, x, y + h - 1, w, 1, col);
    fill_rect_buf(buf, bw, bh, x, y, 1, h, col);
    fill_rect_buf(buf, bw, bh, x + w - 1, y, 1, h, col);
}

static void draw_char_buf(uint32_t *buf, int bw, int bh, int x, int y, char c, uint32_t fg) {
    if (c < 0x20 || c > 0x7E) c = '?';
    const uint8_t *glyph = font8x8_basic[c - 0x20];
    for (int row = 0; row < 8; row++) {
        int py = y + row;
        if (py < 0 || py >= bh) continue;
        uint8_t bits = glyph[row];
        for (int col = 0; col < 8; col++) {
            if (bits & (0x80 >> col)) {
                int px = x + col;
                if (px >= 0 && px < bw) {
                    buf[py * bw + px] = fg;
                }
            }
        }
    }
}

static void draw_text_buf(uint32_t *buf, int bw, int bh, int x, int y, const char *s, uint32_t fg, int max_w) {
    int cx = x;
    for (int i = 0; s[i]; i++) {
        if (max_w > 0 && (cx - x + 8) > max_w) break;
        draw_char_buf(buf, bw, bh, cx, y, s[i], fg);
        cx += 9;
    }
}

static void build_wallpaper(void) {
    int w = (int)g_fb.width;
    int h = (int)g_fb.height;
    for (int py = 0; py < h; py++) {
        int f = (py * 28) / (h > 0 ? h : 1);
        uint8_t r = (uint8_t)(15 > f ? 15 - f : 0);
        uint8_t g = (uint8_t)(23 > f ? 23 - f : 0);
        uint8_t b = (uint8_t)(42 > f ? 42 - f : 0);
        uint32_t col = 0xFF000000u | ((uint32_t)r << 16) | ((uint32_t)g << 8) | b;
        uint32_t *row = &g_wall[py * w];
        for (int px = 0; px < w; px++) row[px] = col;
    }
    for (int x = 0; x < w; x += 40) fill_rect_buf(g_wall, w, h, x, 0, 1, h, 0xFF1A253D);
    for (int y = 0; y < h; y += 40) fill_rect_buf(g_wall, w, h, 0, y, w, 1, 0xFF1A253D);
    draw_text_buf(g_wall, w, h, w - 300, h - TASKBAR_H - 20, "NullOS Ring 3 Compositor (Linux-style)", COL_MUTED, 290);
}

static void render_builtin_info(comp_window_t *win) {
    if (!win->pixels) return;
    fill_rect_buf(win->pixels, win->width, win->height, 0, 0, win->width, win->height, COL_PANEL_BG);
    draw_text_buf(win->pixels, win->width, win->height, 12, 12, "NullOS Ring 3 Display Server", COL_CYAN, win->width - 24);
    fill_rect_buf(win->pixels, win->width, win->height, 12, 26, win->width - 24, 1, COL_BORDER);
    draw_text_buf(win->pixels, win->width, win->height, 12, 38, "Architecture : Ring 3 Compositor", COL_LIGHT_SLATE, win->width - 24);
    draw_text_buf(win->pixels, win->width, win->height, 12, 54, "Kernel Layer : fbdev + evdev only", COL_LIGHT_SLATE, win->width - 24);
    draw_text_buf(win->pixels, win->width, win->height, 12, 70, "Client IPC   : AF_UNIX + SysV SHM", COL_GREEN, win->width - 24);
    draw_text_buf(win->pixels, win->width, win->height, 12, 94, "Shortcuts:", COL_YELLOW, win->width - 24);
    draw_text_buf(win->pixels, win->width, win->height, 12, 110, "  Click [NullOS] -> Start Menu", COL_MUTED, win->width - 24);
    draw_text_buf(win->pixels, win->width, win->height, 12, 126, "  Ctrl+N: Cycle | Ctrl+W: Close", COL_MUTED, win->width - 24);
    draw_text_buf(win->pixels, win->width, win->height, 12, 142, "  ESC: Return to CLI Shell", COL_MUTED, win->width - 24);
}

static void bring_to_front(int idx) {
    int pos = -1;
    for (int i = 0; i < g_win_count; i++) {
        g_wins[g_zorder[i]].focused = 0;
        if (g_zorder[i] == idx) pos = i;
    }
    if (pos >= 0) {
        for (int i = pos; i < g_win_count - 1; i++) g_zorder[i] = g_zorder[i + 1];
        g_zorder[g_win_count - 1] = idx;
    }
    g_wins[idx].minimized = 0;
    g_wins[idx].focused = 1;
    mark_full_dirty();
}

static int create_window_slot(const char *title, int x, int y, int w, int h, int client_fd, uint32_t *pixels, int builtin_info) {
    if (g_win_count >= MAX_WINDOWS) return -1;
    int slot = -1;
    for (int i = 0; i < MAX_WINDOWS; i++) {
        if (!g_wins[i].used) { slot = i; break; }
    }
    if (slot < 0) return -1;

    comp_window_t *win = &g_wins[slot];
    memset(win, 0, sizeof(*win));
    win->used = 1;
    win->id = g_next_win_id++;
    win->client_fd = client_fd;
    win->is_builtin_info = builtin_info;
    size_t tl = strlen(title); if (tl > 31) tl = 31;
    memcpy(win->title, title, tl);
    win->x = x; win->y = y;
    win->width = w; win->height = h;
    win->pixels = pixels;
    g_zorder[g_win_count++] = slot;
    bring_to_front(slot);
    return slot;
}

static void close_window_slot(int idx) {
    if (idx < 0 || idx >= MAX_WINDOWS || !g_wins[idx].used) return;
    if (g_wins[idx].client_fd >= 0) {
        nullos_gui_msg_t msg;
        memset(&msg, 0, sizeof(msg));
        msg.magic = GUI_MSG_MAGIC;
        msg.kind = GUI_MSG_CLOSE;
        msg.win_id = g_wins[idx].id;
        (void)write(g_wins[idx].client_fd, &msg, sizeof(msg));
        close(g_wins[idx].client_fd);
    } else if (g_wins[idx].pixels) {
        free(g_wins[idx].pixels);
    }
    g_wins[idx].used = 0;
    int pos = -1;
    for (int i = 0; i < g_win_count; i++) {
        if (g_zorder[i] == idx) { pos = i; break; }
    }
    if (pos >= 0) {
        for (int i = pos; i < g_win_count - 1; i++) g_zorder[i] = g_zorder[i + 1];
        g_win_count--;
    }
    for (int i = g_win_count - 1; i >= 0; i--) {
        if (!g_wins[g_zorder[i]].minimized) {
            g_wins[g_zorder[i]].focused = 1;
            break;
        }
    }
    mark_full_dirty();
}

static void send_client_event(comp_window_t *win, uint32_t kind, int32_t p1, int32_t p2, int32_t p3) {
    if (!win || win->client_fd < 0) return;
    nullos_gui_msg_t msg;
    memset(&msg, 0, sizeof(msg));
    msg.magic = GUI_MSG_MAGIC;
    msg.kind = GUI_MSG_EVENT;
    msg.win_id = win->id;
    msg.event.kind = kind;
    msg.event.param1 = p1;
    msg.event.param2 = p2;
    msg.event.param3 = p3;
    (void)write(win->client_fd, &msg, sizeof(msg));
}

static void open_builtin_info_window(void) {
    int w = 360, h = 190;
    uint32_t *pix = (uint32_t *)malloc((size_t)w * (size_t)h * sizeof(uint32_t));
    if (!pix) return;
    int idx = create_window_slot("System Info (Ring 3)", 120, 48, w, h, -1, pix, 1);
    if (idx >= 0) render_builtin_info(&g_wins[idx]);
}

static void render_desktop(void) {
    int sw = (int)g_fb.width;
    int sh = (int)g_fb.height;
    g_dirty = 0;
    g_frames++;

    memcpy(g_back, g_wall, (size_t)sw * (size_t)sh * sizeof(uint32_t));

    /* Desktop icons */
    const char *icon_labels[3] = { "Terminal", "GUI Demo", "Sys Info" };
    uint32_t icon_cols[3] = { COL_CYAN, COL_YELLOW, COL_GREEN };
    for (int i = 0; i < 3; i++) {
        int ix = 20, iy = 24 + i * 76;
        int hov = (g_mouse_x >= ix && g_mouse_x < ix + 68 && g_mouse_y >= iy && g_mouse_y < iy + 64);
        if (hov) fill_rect_buf(g_back, sw, sh, ix, iy, 68, 64, 0xFF1F2F4D);
        fill_rect_buf(g_back, sw, sh, ix + 15, iy + 6, 38, 32, COL_PANEL_BG);
        draw_rect_outline(g_back, sw, sh, ix + 15, iy + 6, 38, 32, icon_cols[i]);
        fill_rect_buf(g_back, sw, sh, ix + 19, iy + 10, 30, 8, icon_cols[i]);
        draw_text_buf(g_back, sw, sh, ix + 2, iy + 46, icon_labels[i], COL_WHITE, 68);
    }

    /* Windows in Z-order */
    for (int zi = 0; zi < g_win_count; zi++) {
        comp_window_t *w = &g_wins[g_zorder[zi]];
        if (!w->used || w->minimized) continue;
        int tw = w->width + BORDER_W * 2;
        int th = w->height + TITLEBAR_H + BORDER_W * 2;
        uint32_t bcol = w->focused ? COL_TITLE_ACT : COL_BORDER;
        uint32_t tcol = w->focused ? COL_TITLE_ACT : COL_TITLE_BAR;

        /* Drop shadow + frame + titlebar */
        fill_rect_buf(g_back, sw, sh, w->x + 4, w->y + 4, tw, th, 0xFF05080F);
        fill_rect_buf(g_back, sw, sh, w->x, w->y, tw, th, bcol);
        fill_rect_buf(g_back, sw, sh, w->x + BORDER_W, w->y + BORDER_W, w->width, TITLEBAR_H, tcol);
        draw_text_buf(g_back, sw, sh, w->x + BORDER_W + 8, w->y + BORDER_W + 9, w->title, COL_WHITE, w->width - 70);

        /* Controls: [_] [+] [x] */
        int by = w->y + BORDER_W + (TITLEBAR_H - BTN_SIZE) / 2;
        int bx_min = w->x + tw - BORDER_W - BTN_SIZE * 3 - 12;
        int bx_max = w->x + tw - BORDER_W - BTN_SIZE * 2 - 8;
        int bx_cls = w->x + tw - BORDER_W - BTN_SIZE - 4;
        fill_rect_buf(g_back, sw, sh, bx_min, by, BTN_SIZE, BTN_SIZE, COL_PANEL_BG);
        draw_char_buf(g_back, sw, sh, bx_min + 4, by + 4, '_', COL_WHITE);
        fill_rect_buf(g_back, sw, sh, bx_max, by, BTN_SIZE, BTN_SIZE, COL_PANEL_BG);
        draw_char_buf(g_back, sw, sh, bx_max + 4, by + 4, w->maximized ? '-' : '+', COL_WHITE);
        fill_rect_buf(g_back, sw, sh, bx_cls, by, BTN_SIZE, BTN_SIZE, COL_CLOSE);
        draw_char_buf(g_back, sw, sh, bx_cls + 4, by + 4, 'x', COL_WHITE);

        /* Client surface blit (clipped to screen) */
        if (w->pixels) {
            int cx0 = w->x + BORDER_W;
            int cy0 = w->y + BORDER_W + TITLEBAR_H;
            int dx0 = cx0 < 0 ? 0 : cx0;
            int dy0 = cy0 < 0 ? 0 : cy0;
            int dx1 = cx0 + w->width; if (dx1 > sw) dx1 = sw;
            int dy1 = cy0 + w->height; if (dy1 > sh) dy1 = sh;
            if (dx0 < dx1 && dy0 < dy1) {
                int cw = dx1 - dx0;
                int sx0 = dx0 - cx0;
                int sy0 = dy0 - cy0;
                for (int r = 0; r < dy1 - dy0; r++) {
                    memcpy(&g_back[(dy0 + r) * sw + dx0],
                           &w->pixels[(sy0 + r) * w->width + sx0],
                           (size_t)cw * sizeof(uint32_t));
                }
            }
        }
        /* Resize grip */
        if (!w->maximized) {
            fill_rect_buf(g_back, sw, sh, w->x + tw - 6, w->y + th - 6, 3, 3, COL_MUTED);
        }
    }

    /* Taskbar */
    int tb_y = sh - TASKBAR_H;
    fill_rect_buf(g_back, sw, sh, 0, tb_y, sw, TASKBAR_H, COL_TASKBAR);
    fill_rect_buf(g_back, sw, sh, 0, tb_y, sw, 1, COL_BORDER);
    fill_rect_buf(g_back, sw, sh, 8, tb_y + 4, 80, TASKBAR_H - 8, g_start_open ? COL_CYAN : COL_TITLE_ACT);
    draw_text_buf(g_back, sw, sh, 20, tb_y + 12, "NullOS", COL_WHITE, 64);

    int tab_x = 100;
    int exit_x = sw - 102;
    for (int i = 0; i < MAX_WINDOWS; i++) {
        if (!g_wins[i].used || tab_x + 140 > exit_x - 12) continue;
        uint32_t tc = (g_wins[i].focused && !g_wins[i].minimized) ? COL_TITLE_BAR : COL_PANEL_BG;
        fill_rect_buf(g_back, sw, sh, tab_x, tb_y + 4, 136, TASKBAR_H - 8, tc);
        draw_rect_outline(g_back, sw, sh, tab_x, tb_y + 4, 136, TASKBAR_H - 8, COL_BORDER);
        draw_text_buf(g_back, sw, sh, tab_x + 6, tb_y + 12, g_wins[i].title, COL_LIGHT_SLATE, 124);
        tab_x += 142;
    }

    fill_rect_buf(g_back, sw, sh, exit_x, tb_y + 4, 90, TASKBAR_H - 8, COL_CLOSE);
    draw_text_buf(g_back, sw, sh, exit_x + 10, tb_y + 12, "Exit GUI", COL_WHITE, 72);

    /* Start Menu */
    if (g_start_open) {
        int mw = 210, ih = 28, mh = 4 * ih + 12;
        int mx = 8, my = tb_y - mh;
        fill_rect_buf(g_back, sw, sh, mx, my, mw, mh, COL_DARK_SLATE);
        draw_rect_outline(g_back, sw, sh, mx, my, mw, mh, COL_TITLE_ACT);
        const char *items[4] = { "Terminal (Ring 3)", "GUI Demo", "System Info", "Exit Desktop -> CLI" };
        for (int i = 0; i < 4; i++) {
            int iy = my + 6 + i * ih;
            int hov = (g_mouse_x >= mx + 6 && g_mouse_x < mx + mw - 6 && g_mouse_y >= iy && g_mouse_y < iy + ih - 4);
            uint32_t bg = (i == 3) ? COL_CLOSE : (hov ? COL_TITLE_BAR : COL_PANEL_BG);
            fill_rect_buf(g_back, sw, sh, mx + 6, iy, mw - 12, ih - 4, bg);
            draw_text_buf(g_back, sw, sh, mx + 14, iy + 8, items[i], COL_WHITE, mw - 24);
        }
    }

    /* Mouse Cursor */
    for (int r = 0; r < 16; r++) {
        int py = g_mouse_y + r;
        if (py < 0 || py >= sh) continue;
        for (int c = 0; c < 12; c++) {
            int px = g_mouse_x + c;
            if (px < 0 || px >= sw) continue;
            if (CURSOR_BMP[r][c] == 'X') g_back[py * sw + px] = COL_BLACK;
            else if (CURSOR_BMP[r][c] == 'W') g_back[py * sw + px] = COL_WHITE;
        }
    }

    /* Dirty-rect flush to mapped hardware VRAM */
    int rx0 = g_dmg_x0, ry0 = g_dmg_y0, rx1 = g_dmg_x1, ry1 = g_dmg_y1;
    if (rx0 >= rx1 || ry0 >= ry1) { rx0 = 0; ry0 = 0; rx1 = sw; ry1 = sh; }
    else if (rx1 - rx0 < sw || ry1 - ry0 < sh) g_partials++;

    int cw = rx1 - rx0;
    for (int py = ry0; py < ry1; py++) {
        memcpy(&g_vram[py * (int)g_fb.stride + rx0],
               &g_back[py * sw + rx0],
               (size_t)cw * sizeof(uint32_t));
    }
    g_dmg_x0 = g_dmg_y0 = g_dmg_x1 = g_dmg_y1 = 0;
}

static void handle_mouse_down(int button, int x, int y) {
    int tb_y = (int)g_fb.height - TASKBAR_H;
    mark_full_dirty();

    if (g_start_open && button == 0) {
        int mw = 210, ih = 28, mh = 4 * ih + 12;
        int mx = 8, my = tb_y - mh;
        if (x >= mx && x < mx + mw && y >= my && y < tb_y) {
            int idx = (y - my - 6) / ih;
            g_start_open = 0;
            if (idx == 0) (void)sys_spawn("/bin/term.elf");
            else if (idx == 1) (void)sys_spawn("/bin/gui_demo.elf");
            else if (idx == 2) open_builtin_info_window();
            else if (idx == 3) g_running = 0;
            return;
        }
        if (!(y >= tb_y && x >= 8 && x < 88)) g_start_open = 0;
    }

    if (y >= tb_y && button == 0) {
        if (x >= 8 && x < 88) { g_start_open = !g_start_open; return; }
        int exit_x = (int)g_fb.width - 102;
        if (x >= exit_x && x < exit_x + 90) { g_running = 0; return; }
        int tab_x = 100;
        for (int i = 0; i < MAX_WINDOWS; i++) {
            if (!g_wins[i].used || tab_x + 140 > exit_x - 12) continue;
            if (x >= tab_x && x < tab_x + 136) {
                if (g_wins[i].focused && !g_wins[i].minimized) g_wins[i].minimized = 1;
                else bring_to_front(i);
                return;
            }
            tab_x += 142;
        }
        return;
    }

    for (int zi = g_win_count - 1; zi >= 0; zi--) {
        int idx = g_zorder[zi];
        comp_window_t *w = &g_wins[idx];
        if (!w->used || w->minimized) continue;
        int tw = w->width + BORDER_W * 2;
        int th = w->height + TITLEBAR_H + BORDER_W * 2;
        if (x >= w->x && x < w->x + tw && y >= w->y && y < w->y + th) {
            int by = w->y + BORDER_W + (TITLEBAR_H - BTN_SIZE) / 2;
            int bx_min = w->x + tw - BORDER_W - BTN_SIZE * 3 - 12;
            int bx_max = w->x + tw - BORDER_W - BTN_SIZE * 2 - 8;
            int bx_cls = w->x + tw - BORDER_W - BTN_SIZE - 4;
            if (button == 0 && y >= by && y < by + BTN_SIZE) {
                if (x >= bx_cls && x < bx_cls + BTN_SIZE) { close_window_slot(idx); return; }
                if (x >= bx_min && x < bx_min + BTN_SIZE) { w->minimized = 1; w->focused = 0; return; }
                if (x >= bx_max && x < bx_max + BTN_SIZE) {
                    if (w->maximized) { w->x = w->saved_x; w->y = w->saved_y; w->maximized = 0; }
                    else { w->saved_x = w->x; w->saved_y = w->y; w->x = 0; w->y = 0; w->maximized = 1; }
                    bring_to_front(idx);
                    return;
                }
            }
            bring_to_front(idx);
            if (button == 0 && !w->maximized && x >= w->x + tw - 14 && y >= w->y + th - 14) {
                g_resize_idx = idx; g_resize_mx = x; g_resize_my = y; g_resize_w = w->width; g_resize_h = w->height;
            } else if (button == 0 && y < w->y + BORDER_W + TITLEBAR_H) {
                g_drag_idx = idx; g_drag_ox = x - w->x; g_drag_oy = y - w->y;
            } else {
                send_client_event(w, GUI_EVENT_MOUSE_DOWN, x - (w->x + BORDER_W), y - (w->y + BORDER_W + TITLEBAR_H), button);
            }
            return;
        }
    }

    if (button == 0) {
        for (int i = 0; i < 3; i++) {
            int ix = 20, iy = 24 + i * 76;
            if (x >= ix && x < ix + 68 && y >= iy && y < iy + 64) {
                if (i == 0) (void)sys_spawn("/bin/term.elf");
                else if (i == 1) (void)sys_spawn("/bin/gui_demo.elf");
                else open_builtin_info_window();
                return;
            }
        }
    }
}

static void poll_clients(void) {
    int new_fd = unix_accept(COMPOSITOR_SOCK_PATH);
    if (new_fd >= 0) {
        nullos_gui_msg_t msg;
        for (int tries = 0; tries < 50; tries++) {
            ssize_t n = read(new_fd, &msg, sizeof(msg));
            if (n == (ssize_t)sizeof(msg) && msg.magic == GUI_MSG_MAGIC && msg.kind == GUI_MSG_CREATE_WINDOW) {
                size_t sz = 0;
                uint32_t *pix = (uint32_t *)sys_shmat(msg.shm_id, &sz);
                if (pix) {
                    int off = (g_win_count % 5) * 28;
                    int idx = create_window_slot(msg.title, 180 + off, 80 + off, msg.a, msg.b, new_fd, pix, 0);
                    if (idx >= 0) {
                        nullos_gui_msg_t rep;
                        memset(&rep, 0, sizeof(rep));
                        rep.magic = GUI_MSG_MAGIC;
                        rep.kind = GUI_MSG_CREATED;
                        rep.win_id = g_wins[idx].id;
                        (void)write(new_fd, &rep, sizeof(rep));
                        break;
                    }
                }
                close(new_fd);
                break;
            }
            usleep(2000);
        }
    }

    for (int i = 0; i < MAX_WINDOWS; i++) {
        if (!g_wins[i].used || g_wins[i].client_fd < 0) continue;
        nullos_gui_msg_t msg;
        while (read(g_wins[i].client_fd, &msg, sizeof(msg)) == (ssize_t)sizeof(msg)) {
            if (msg.magic != GUI_MSG_MAGIC) continue;
            if (msg.kind == GUI_MSG_COMMIT) {
                if (!g_wins[i].minimized) {
                    mark_dirty_rect(g_wins[i].x, g_wins[i].y, g_wins[i].width + 8, g_wins[i].height + TITLEBAR_H + 8);
                }
            } else if (msg.kind == GUI_MSG_CLOSE) {
                close_window_slot(i);
                break;
            }
        }
    }
}

int main(void) {
    g_vram = fb_map(&g_fb);
    if (!g_vram || g_fb.width == 0 || g_fb.height == 0) {
        printf("[compositor] Failed to map framebuffer.\n");
        return 1;
    }
    if (unix_listen(COMPOSITOR_SOCK_PATH) < 0) {
        fb_release();
        return 1;
    }

    size_t fb_pixels = (size_t)g_fb.width * (size_t)g_fb.height;
    g_back = (uint32_t *)malloc(fb_pixels * sizeof(uint32_t));
    g_wall = (uint32_t *)malloc(fb_pixels * sizeof(uint32_t));
    if (!g_back || !g_wall) {
        fb_release();
        return 1;
    }

    g_mouse_x = (int)g_fb.width / 2;
    g_mouse_y = (int)g_fb.height / 2;
    build_wallpaper();
    open_builtin_info_window();
    mark_full_dirty();
    render_desktop();

    while (g_running) {
        nullos_raw_input_t ev;
        int had_input = 0;
        while (input_poll(&ev) > 0) {
            had_input = 1;
            if (ev.kind == RAW_INPUT_MOUSE_MOVE) {
                int ox = g_mouse_x, oy = g_mouse_y;
                g_mouse_x = ev.x; g_mouse_y = ev.y;
                if (g_drag_idx >= 0 && g_wins[g_drag_idx].used) {
                    comp_window_t *w = &g_wins[g_drag_idx];
                    int tw = w->width + 8, th = w->height + TITLEBAR_H + 8;
                    mark_dirty_rect(w->x, w->y, tw, th);
                    w->x = ev.x - g_drag_ox;
                    w->y = ev.y - g_drag_oy;
                    if (w->y < 0) w->y = 0;
                    mark_dirty_rect(w->x, w->y, tw, th);
                } else {
                    mark_dirty_rect(ox, oy, 14, 18);
                    mark_dirty_rect(ev.x, ev.y, 14, 18);
                    if (g_win_count > 0) {
                        comp_window_t *top = &g_wins[g_zorder[g_win_count - 1]];
                        if (top->client_fd >= 0 && !top->minimized) {
                            int lx = ev.x - (top->x + BORDER_W);
                            int ly = ev.y - (top->y + BORDER_W + TITLEBAR_H);
                            if (lx >= 0 && ly >= 0 && lx < top->width && ly < top->height) {
                                send_client_event(top, GUI_EVENT_MOUSE_MOVE, lx, ly, 0);
                            }
                        }
                    }
                }
            } else if (ev.kind == RAW_INPUT_MOUSE_DOWN) {
                g_mouse_x = ev.x; g_mouse_y = ev.y;
                handle_mouse_down((int)ev.button, ev.x, ev.y);
            } else if (ev.kind == RAW_INPUT_MOUSE_UP) {
                g_drag_idx = -1;
                g_resize_idx = -1;
                if (g_win_count > 0) {
                    comp_window_t *top = &g_wins[g_zorder[g_win_count - 1]];
                    if (top->client_fd >= 0) {
                        send_client_event(top, GUI_EVENT_MOUSE_UP, ev.x - (top->x + BORDER_W), ev.y - (top->y + BORDER_W + TITLEBAR_H), (int32_t)ev.button);
                    }
                }
            } else if (ev.kind == RAW_INPUT_KEY) {
                if (ev.key == 0x1B) { g_running = 0; break; }
                if (ev.key == 0x0E && g_win_count > 1) { bring_to_front(g_zorder[0]); }
                else if (ev.key == 0x17 && g_win_count > 0) { close_window_slot(g_zorder[g_win_count - 1]); }
                else if (g_win_count > 0) {
                    comp_window_t *top = &g_wins[g_zorder[g_win_count - 1]];
                    if (top->client_fd >= 0) send_client_event(top, GUI_EVENT_KEY_DOWN, (int32_t)ev.key, 0, 0);
                }
            }
        }

        poll_clients();
        if (g_dirty) render_desktop();
        if (!had_input) usleep(10000);
    }

    for (int i = MAX_WINDOWS - 1; i >= 0; i--) {
        if (g_wins[i].used) close_window_slot(i);
    }
    fb_release();
    return 0;
}

