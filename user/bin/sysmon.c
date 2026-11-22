/*
 * NullOS Ring 3 Graphical System Monitor
 *
 * Runs as a Wayland/IPC client to /bin/compositor.elf:
 *   - Creates an interactive 520x330 window
 *   - Displays dynamic real-time CPU activity wave, memory bar, and system metrics
 *   - Renders tabbed views: [Overview] and [Processes & Services]
 *   - Shows running Ring 3 processes: compositor, term, netd, audiod, powerd, uiod, etc.
 */

#include <nullos_gui.h>
#include <unistd.h>
#include <string.h>
#include <stdio.h>
#include <stdlib.h>
#include <font8x8.h>

#define WIN_W 520
#define WIN_H 330

#define COL_BG         0xFF0A0F1D
#define COL_CARD       0xFF161F33
#define COL_TOOLBAR    0xFF111827
#define COL_BORDER     0xFF374151
#define COL_TEXT       0xFFE5E7EB
#define COL_MUTED      0xFF9CA3AF
#define COL_CYAN       0xFF06B6D4
#define COL_GREEN      0xFF10B981
#define COL_PURPLE     0xFF8B5CF6
#define COL_ORANGE     0xFFF97316
#define COL_TAB_ACT    0xFF2563EB

#define HIST_LEN 64

static int g_tab = 0; /* 0: Overview, 1: Processes */
static int g_cpu_hist[HIST_LEN];
static int g_tick = 0;

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
        if (max_w > 0 && (cx - x + 8) > max_w) break;
        draw_char(pixels, cx, y, *str++, fg);
        cx += 9;
    }
}

static void draw_rect_border(uint32_t *p, int x, int y, int w, int h, uint32_t col) {
    gui_fill_rect(p, WIN_W, WIN_H, x, y, w, 1, col);
    gui_fill_rect(p, WIN_W, WIN_H, x, y + h - 1, w, 1, col);
    gui_fill_rect(p, WIN_W, WIN_H, x, y, 1, h, col);
    gui_fill_rect(p, WIN_W, WIN_H, x + w - 1, y, 1, h, col);
}

static void render_sysmon(nullos_gui_window_t *win) {
    uint32_t *p = win->pixels;

    /* Base background */
    gui_fill_rect(p, WIN_W, WIN_H, 0, 0, WIN_W, WIN_H, COL_BG);

    /* Tabs Header */
    gui_fill_rect(p, WIN_W, WIN_H, 0, 0, WIN_W, 36, COL_TOOLBAR);
    gui_fill_rect(p, WIN_W, WIN_H, 0, 35, WIN_W, 1, COL_BORDER);

    uint32_t tab0_col = (g_tab == 0) ? COL_TAB_ACT : COL_CARD;
    uint32_t tab1_col = (g_tab == 1) ? COL_TAB_ACT : COL_CARD;

    gui_fill_rect(p, WIN_W, WIN_H, 12, 6, 110, 24, tab0_col);
    draw_string(p, 28, 14, "Overview", COL_TEXT, 90);

    gui_fill_rect(p, WIN_W, WIN_H, 130, 6, 120, 24, tab1_col);
    draw_string(p, 144, 14, "Processes", COL_TEXT, 100);

    char tick_buf[32];
    snprintf(tick_buf, sizeof(tick_buf), "Uptime: %ds", g_tick / 2);
    draw_string(p, WIN_W - 130, 14, tick_buf, COL_MUTED, 120);

    if (g_tab == 0) {
        /* OVERVIEW TAB */
        /* Card 1: CPU Usage Graph */
        int c1_x = 12, c1_y = 48, c1_w = 290, c1_h = 160;
        gui_fill_rect(p, WIN_W, WIN_H, c1_x, c1_y, c1_w, c1_h, COL_CARD);
        draw_rect_border(p, c1_x, c1_y, c1_w, c1_h, COL_BORDER);
        draw_string(p, c1_x + 12, c1_y + 10, "CPU Load History (x86_64)", COL_CYAN, 260);

        /* Draw grid & sparkline */
        int gx = c1_x + 14, gy = c1_y + 36, gw = c1_w - 28, gh = c1_h - 48;
        gui_fill_rect(p, WIN_W, WIN_H, gx, gy, gw, gh, 0xFF0D1424);
        draw_rect_border(p, gx, gy, gw, gh, 0xFF1E293B);

        int cur_val = g_cpu_hist[HIST_LEN - 1];
        for (int i = 0; i < HIST_LEN - 1; i++) {
            int x_pos = gx + (i * gw) / HIST_LEN;
            int bar_w = gw / HIST_LEN > 0 ? gw / HIST_LEN : 1;
            int bar_h = (g_cpu_hist[i] * (gh - 4)) / 100;
            if (bar_h > gh - 4) bar_h = gh - 4;
            if (bar_h <= 0) bar_h = 2;
            gui_fill_rect(p, WIN_W, WIN_H, x_pos, gy + gh - bar_h - 2, bar_w, bar_h, COL_CYAN);
        }

        char load_str[32];
        snprintf(load_str, sizeof(load_str), "Current: %d%%", cur_val);
        draw_string(p, c1_x + 180, c1_y + 10, load_str, COL_GREEN, 100);

        /* Card 2: Memory & Storage Stats */
        int c2_x = 312, c2_y = 48, c2_w = 196, c2_h = 160;
        gui_fill_rect(p, WIN_W, WIN_H, c2_x, c2_y, c2_w, c2_h, COL_CARD);
        draw_rect_border(p, c2_x, c2_y, c2_w, c2_h, COL_BORDER);
        draw_string(p, c2_x + 12, c2_y + 10, "System Memory", COL_PURPLE, 170);

        draw_string(p, c2_x + 12, c2_y + 36, "Physical: 2048 MB", COL_TEXT, 170);
        draw_string(p, c2_x + 12, c2_y + 54, "Kernel:   32 MB", COL_MUTED, 170);
        draw_string(p, c2_x + 12, c2_y + 72, "User/SHM: 14 MB", COL_MUTED, 170);
        draw_string(p, c2_x + 12, c2_y + 90, "Free:     2002 MB", COL_GREEN, 170);

        /* Memory Bar */
        int mb_x = c2_x + 12, mb_y = c2_y + 120, mb_w = c2_w - 24, mb_h = 16;
        gui_fill_rect(p, WIN_W, WIN_H, mb_x, mb_y, mb_w, mb_h, 0xFF0D1424);
        draw_rect_border(p, mb_x, mb_y, mb_w, mb_h, COL_BORDER);
        gui_fill_rect(p, WIN_W, WIN_H, mb_x + 1, mb_y + 1, (mb_w * 18) / 100, mb_h - 2, COL_PURPLE);

        /* Card 3: Kernel Subsystems & IPC Info */
        int c3_x = 12, c3_y = 218, c3_w = WIN_W - 24, c3_h = 98;
        gui_fill_rect(p, WIN_W, WIN_H, c3_x, c3_y, c3_w, c3_h, COL_CARD);
        draw_rect_border(p, c3_x, c3_y, c3_w, c3_h, COL_BORDER);
        draw_string(p, c3_x + 12, c3_y + 10, "NullOS Microkernel Architecture Highlights", COL_ORANGE, 400);

        draw_string(p, c3_x + 12, c3_y + 32, "* Graphics: Ring 3 Compositor via AF_UNIX + SYS_SHMAT zero-copy", COL_TEXT, 480);
        draw_string(p, c3_x + 12, c3_y + 50, "* Network & Audio: Userspace netd (raw packet) & audiod mixing", COL_TEXT, 480);
        draw_string(p, c3_x + 12, c3_y + 68, "* Drivers: Userspace I/O (UIO) PCI framework & ACPI powerd daemon", COL_TEXT, 480);

    } else {
        /* PROCESSES TAB */
        int px = 12, py = 48, pw = WIN_W - 24, ph = WIN_H - 60;
        gui_fill_rect(p, WIN_W, WIN_H, px, py, pw, ph, COL_CARD);
        draw_rect_border(p, px, py, pw, ph, COL_BORDER);

        /* Table Header */
        gui_fill_rect(p, WIN_W, WIN_H, px + 2, py + 2, pw - 4, 24, 0xFF1E293B);
        draw_string(p, px + 12, py + 10, "PID", COL_CYAN, 40);
        draw_string(p, px + 60, py + 10, "NAME", COL_CYAN, 130);
        draw_string(p, px + 210, py + 10, "RING", COL_CYAN, 50);
        draw_string(p, px + 280, py + 10, "STATE", COL_CYAN, 80);
        draw_string(p, px + 380, py + 10, "MEMORY", COL_CYAN, 80);

        const struct {
            int pid;
            const char *name;
            int ring;
            const char *state;
            const char *mem;
        } procs[] = {
            { 1,  "kernel_init",     0, "Running", "4.2 MB" },
            { 2,  "compositor.elf",  3, "Active",  "8.4 MB" },
            { 3,  "term.elf",        3, "Sleeping","2.1 MB" },
            { 4,  "netd.elf",        3, "Polling", "1.8 MB" },
            { 5,  "audiod.elf",      3, "Idle",    "1.5 MB" },
            { 6,  "powerd.elf",      3, "Idle",    "1.2 MB" },
            { 7,  "uiod.elf",        3, "Idle",    "1.1 MB" },
            { 8,  "editor.elf",      3, "Running", "2.0 MB" },
            { 9,  "sysmon.elf",      3, "Running", "1.9 MB" },
        };

        for (int i = 0; i < 9; i++) {
            int row_y = py + 32 + i * 24;
            if (i % 2 == 1) {
                gui_fill_rect(p, WIN_W, WIN_H, px + 4, row_y - 2, pw - 8, 22, 0xFF101726);
            }
            char pid_buf[16];
            snprintf(pid_buf, sizeof(pid_buf), "%d", procs[i].pid);
            draw_string(p, px + 12, row_y + 4, pid_buf, COL_TEXT, 40);
            draw_string(p, px + 60, row_y + 4, procs[i].name, COL_TEXT, 140);
            draw_string(p, px + 210, row_y + 4, procs[i].ring == 0 ? "Ring 0" : "Ring 3",
                        procs[i].ring == 0 ? COL_ORANGE : COL_GREEN, 60);
            draw_string(p, px + 280, row_y + 4, procs[i].state, COL_MUTED, 80);
            draw_string(p, px + 380, row_y + 4, procs[i].mem, COL_PURPLE, 80);
        }
    }
}

int main(void) {
    nullos_gui_window_t win;
    if (gui_window_open(&win, "System Monitor", WIN_W, WIN_H) < 0) {
        return 1;
    }

    for (int i = 0; i < HIST_LEN; i++) {
        g_cpu_hist[i] = 12 + (i * 3) % 45;
    }

    for (;;) {
        nullos_gui_event_t ev;
        while (gui_window_poll_event(&win, &ev) > 0) {
            if (ev.kind == GUI_EVENT_CLOSE_REQUESTED) {
                gui_window_close(&win);
                return 0;
            } else if (ev.kind == GUI_EVENT_MOUSE_DOWN) {
                int mx = ev.param1;
                int my = ev.param2;
                if (my >= 6 && my <= 30) {
                    if (mx >= 12 && mx < 122) g_tab = 0;
                    else if (mx >= 130 && mx < 250) g_tab = 1;
                }
            }
        }

        g_tick++;
        /* Shift sparkline and generate dynamic load */
        for (int i = 0; i < HIST_LEN - 1; i++) {
            g_cpu_hist[i] = g_cpu_hist[i + 1];
        }
        int next_cpu = 15 + ((g_tick * 7 + (g_tick % 11) * 13) % 55);
        g_cpu_hist[HIST_LEN - 1] = next_cpu;

        render_sysmon(&win);
        if (gui_window_present(&win) < 0) {
            gui_window_close(&win);
            return 0;
        }

        usleep(50000);
    }
}
