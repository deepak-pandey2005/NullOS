//! Desktop Window Manager & Compositor.
//!
//! Features:
//! - Z-ordered window stack management with click-to-focus.
//! - Window title bar dragging and close-button handling.
//! - Desktop wallpaper background and system taskbar.
//! - Double-buffered composite pipeline with hardware flush.

use alloc::vec::Vec;
use crate::graphics::canvas::{Canvas, CHAR_HEIGHT, CHAR_WIDTH};
use crate::graphics::color::Color;
use crate::graphics::event::{
    GuiUserEvent, InputEvent, MouseButton, GUI_EVENT_CLOSE_REQUESTED, GUI_EVENT_MOUSE_DOWN,
    GUI_EVENT_MOUSE_MOVE, GUI_EVENT_MOUSE_UP,
};
use crate::graphics::mouse::MOUSE;
use crate::graphics::window::{Window, WindowKind, BORDER_WIDTH, TITLEBAR_HEIGHT};
use crate::sync::Spinlock;

pub const TASKBAR_HEIGHT: usize = 32;

/// Actions queued by UI clicks (Start Menu / Desktop Icons) and executed outside the WM lock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DesktopAction {
    OpenTerminal,
    OpenFileManager,
    OpenSystemMonitor,
    OpenKernelLog,
    LaunchUserGuiDemo,
}

const START_MENU_ITEMS: [(&str, DesktopAction); 5] = [
    ("New Terminal", DesktopAction::OpenTerminal),
    ("File Manager", DesktopAction::OpenFileManager),
    ("System Monitor", DesktopAction::OpenSystemMonitor),
    ("Kernel Log (dmesg)", DesktopAction::OpenKernelLog),
    ("GUI Demo (Ring 3)", DesktopAction::LaunchUserGuiDemo),
];

const DESKTOP_ICONS: [(&str, Color, DesktopAction); 4] = [
    ("Terminal", Color::TITLE_ACTIVE, DesktopAction::OpenTerminal),
    ("Files", Color::YELLOW, DesktopAction::OpenFileManager),
    ("Monitor", Color::GREEN, DesktopAction::OpenSystemMonitor),
    ("GUI Demo", Color::CYAN, DesktopAction::LaunchUserGuiDemo),
];

/// Global desktop window manager instance.
pub struct WindowManager {
    pub screen_width: usize,
    pub screen_height: usize,
    pub hw_address: u64,
    pub hw_stride: usize,
    pub backbuffer: Canvas,
    pub wallpaper_cache: Vec<u32>,
    pub windows: Vec<Window>,
    pub focused_window: Option<u32>,
    pub dragging: Option<(u32, isize, isize)>,
    pub start_menu_open: bool,
    pub pending_actions: Vec<DesktopAction>,
    pub is_running: bool,
    pub is_dirty: bool,
    next_window_id: u32,
}

impl WindowManager {
    pub fn new(width: usize, height: usize, stride: usize, hw_address: u64) -> Self {
        let mut backbuffer = Canvas::new(width, height, stride);
        Self::build_wallpaper_into(&mut backbuffer, width, height);
        let wallpaper_cache = backbuffer.buffer.clone();
        Self {
            screen_width: width,
            screen_height: height,
            hw_address,
            hw_stride: stride,
            backbuffer,
            wallpaper_cache,
            windows: Vec::new(),
            focused_window: None,
            dragging: None,
            start_menu_open: false,
            pending_actions: Vec::new(),
            is_running: true,
            is_dirty: true,
            next_window_id: 1,
        }
    }

    fn build_wallpaper_into(canvas: &mut Canvas, w: usize, h: usize) {
        for py in 0..h {
            let factor = (py * 30) / h.max(1);
            let r = 15u8.saturating_sub(factor as u8);
            let g = 23u8.saturating_sub(factor as u8);
            let b = 42u8.saturating_sub(factor as u8);
            let color = Color::rgb(r, g, b);

            let row_start = py * canvas.stride;
            if row_start + w <= canvas.buffer.len() {
                canvas.buffer[row_start..row_start + w].fill(color.to_u32());
            }
        }

        let grid_color = Color::rgba(255, 255, 255, 8);
        for x in (0..w).step_by(40) {
            canvas.draw_line(x as isize, 0, x as isize, h as isize, grid_color);
        }
        for y in (0..h).step_by(40) {
            canvas.draw_line(0, y as isize, w as isize, y as isize, grid_color);
        }

        let brand_str = "NullOS 64-Bit Operating System";
        let brand_x = w.saturating_sub(brand_str.len() * CHAR_WIDTH + 24);
        let brand_y = h.saturating_sub(TASKBAR_HEIGHT + CHAR_HEIGHT + 16);
        canvas.draw_string(brand_x, brand_y, brand_str, Color::rgba(255, 255, 255, 60), None);
    }

    /// Signals the window manager event loop to terminate and return to text console.
    pub fn request_exit(&mut self) {
        self.is_running = false;
        self.is_dirty = true;
    }

    /// Allocates and creates a new managed Window.
    pub fn create_window(&mut self, title: &str, x: isize, y: isize, width: usize, height: usize) -> u32 {
        let id = self.next_window_id;
        self.next_window_id += 1;

        let mut win = Window::new(id, title, x, y, width, height);
        win.is_focused = true;

        for w in self.windows.iter_mut() {
            w.is_focused = false;
        }

        self.windows.push(win);
        self.focused_window = Some(id);
        self.is_dirty = true;
        id
    }

    /// Closes and removes a window by ID.
    pub fn close_window(&mut self, id: u32) {
        if let Some(pos) = self.windows.iter().position(|w| w.id == id) {
            self.windows.remove(pos);
            if self.focused_window == Some(id) {
                self.focused_window = self.windows.iter().rev().find(|w| !w.minimized).map(|w| w.id);
                if let Some(fid) = self.focused_window {
                    if let Some(top) = self.windows.iter_mut().find(|w| w.id == fid) {
                        top.is_focused = true;
                    }
                }
            }
            self.is_dirty = true;
        }
    }

    /// Minimizes a window by ID and shifts focus to the topmost visible window.
    pub fn minimize_window(&mut self, id: u32) {
        if let Some(win) = self.windows.iter_mut().find(|w| w.id == id) {
            win.minimized = true;
            win.is_focused = false;
        }
        if self.focused_window == Some(id) {
            self.focused_window = self.windows.iter().rev().find(|w| !w.minimized).map(|w| w.id);
            if let Some(fid) = self.focused_window {
                if let Some(top) = self.windows.iter_mut().find(|w| w.id == fid) {
                    top.is_focused = true;
                }
            }
        }
        self.is_dirty = true;
    }

    /// Toggles maximize/restore for a window (resizes non-UserApp surfaces cleanly).
    pub fn toggle_maximize_window(&mut self, id: u32) {
        let max_w = self.screen_width.saturating_sub(BORDER_WIDTH * 2);
        let max_h = self
            .screen_height
            .saturating_sub(TASKBAR_HEIGHT + TITLEBAR_HEIGHT + BORDER_WIDTH * 2);
        if let Some(win) = self.windows.iter_mut().find(|w| w.id == id) {
            if win.maximized {
                let (sx, sy, sw, sh) = win.saved_rect;
                win.x = sx;
                win.y = sy;
                win.width = sw;
                win.height = sh;
                win.maximized = false;
            } else {
                win.saved_rect = (win.x, win.y, win.width, win.height);
                win.x = 0;
                win.y = 0;
                win.width = max_w;
                win.height = max_h;
                win.maximized = true;
            }
            if win.kind != WindowKind::UserApp {
                win.surface = Canvas::new(win.width, win.height, win.width);
                match win.kind {
                    WindowKind::Terminal => win.render_terminal_surface(),
                    WindowKind::FileManager => win.render_file_manager_surface(),
                    _ => {}
                }
            }
        }
        self.bring_to_front(id);
        self.is_dirty = true;
    }

    /// Brings the specified window to the front of the Z-order (and un-minimizes it).
    pub fn bring_to_front(&mut self, id: u32) {
        if let Some(pos) = self.windows.iter().position(|w| w.id == id) {
            let mut win = self.windows.remove(pos);
            for w in self.windows.iter_mut() {
                w.is_focused = false;
            }
            win.minimized = false;
            win.is_focused = true;
            self.windows.push(win);
            self.focused_window = Some(id);
            self.is_dirty = true;
        }
    }

    fn mouse_button_code(button: MouseButton) -> i32 {
        match button {
            MouseButton::Left => 0,
            MouseButton::Right => 1,
            MouseButton::Middle => 2,
        }
    }

    /// Dispatches an input event to the window manager.
    pub fn handle_event(&mut self, event: InputEvent) {
        self.is_dirty = true;
        match event {
            InputEvent::MouseDown { button, x, y } => {
                let tb_y = (self.screen_height.saturating_sub(TASKBAR_HEIGHT)) as isize;

                // 0. If Start Menu is open, check clicks inside it first (Left click)
                if self.start_menu_open && button == MouseButton::Left {
                    let menu_w = 210isize;
                    let item_h = 28isize;
                    let total_items = (START_MENU_ITEMS.len() + 1) as isize; // +1 for Exit
                    let menu_h = total_items * item_h + 12;
                    let menu_x = 8isize;
                    let menu_y = tb_y - menu_h;

                    if x >= menu_x && x < menu_x + menu_w && y >= menu_y && y < tb_y {
                        let rel_y = y - menu_y - 6;
                        if rel_y >= 0 {
                            let idx = (rel_y / item_h) as usize;
                            if idx < START_MENU_ITEMS.len() {
                                self.pending_actions.push(START_MENU_ITEMS[idx].1);
                                self.start_menu_open = false;
                                return;
                            } else if idx == START_MENU_ITEMS.len() {
                                self.start_menu_open = false;
                                self.is_running = false;
                                return;
                            }
                        }
                    }
                    // Clicking outside Start Menu closes it (unless clicking the Start button itself below)
                    if !(y >= tb_y && x >= 8 && x < 88) {
                        self.start_menu_open = false;
                    }
                }

                // 1. Check clicks on Taskbar
                if y >= tb_y && button == MouseButton::Left {
                    // Start / "NullOS" button
                    if x >= 8 && x < 88 {
                        self.start_menu_open = !self.start_menu_open;
                        return;
                    }

                    let exit_btn_w = 90isize;
                    let exit_btn_x = (self.screen_width.saturating_sub(exit_btn_w as usize + 12)) as isize;

                    if x >= exit_btn_x && x < exit_btn_x + exit_btn_w {
                        self.is_running = false;
                        return;
                    }

                    let mut tab_x = 100isize;
                    let max_tabs_x = exit_btn_x.saturating_sub(100);
                    let mut target_id = None;
                    let mut was_focused_visible = false;
                    for win in self.windows.iter() {
                        let tab_w = 140isize;
                        if tab_x + tab_w > max_tabs_x {
                            break;
                        }
                        if x >= tab_x && x < tab_x + tab_w {
                            target_id = Some(win.id);
                            was_focused_visible = win.is_focused && !win.minimized;
                            break;
                        }
                        tab_x += tab_w + 6;
                    }
                    if let Some(win_id) = target_id {
                        if was_focused_visible {
                            self.minimize_window(win_id);
                        } else {
                            self.bring_to_front(win_id);
                        }
                        return;
                    }
                    return;
                }

                // 2. Check visible windows from top to bottom (reverse Z-order)
                let mut clicked_win = None;
                let mut close_clicked = None;
                let mut min_clicked = None;
                let mut max_clicked = None;
                let mut drag_started = None;
                let mut client_click: Option<(u32, WindowKind, isize, isize)> = None;

                for win in self.windows.iter().rev() {
                    if win.minimized {
                        continue;
                    }
                    if win.contains_point(x, y) {
                        clicked_win = Some(win.id);
                        if button == MouseButton::Left && win.is_close_button(x, y) {
                            close_clicked = Some(win.id);
                        } else if button == MouseButton::Left && win.is_minimize_button(x, y) {
                            min_clicked = Some(win.id);
                        } else if button == MouseButton::Left && win.is_maximize_button(x, y) {
                            max_clicked = Some(win.id);
                        } else if button == MouseButton::Left && win.is_titlebar(x, y) && win.draggable {
                            drag_started = Some((win.id, x - win.x, y - win.y));
                        } else {
                            let cx = x - (win.x + BORDER_WIDTH as isize);
                            let cy = y - (win.y + (BORDER_WIDTH + TITLEBAR_HEIGHT) as isize);
                            if cx >= 0 && cy >= 0 && (cx as usize) < win.width && (cy as usize) < win.height {
                                client_click = Some((win.id, win.kind, cx, cy));
                            }
                        }
                        break;
                    }
                }

                if let Some(close_id) = close_clicked {
                    if let Some(win) = self.windows.iter_mut().find(|w| w.id == close_id) {
                        if win.kind == WindowKind::UserApp {
                            win.push_user_event(GuiUserEvent {
                                kind: GUI_EVENT_CLOSE_REQUESTED,
                                param1: 0,
                                param2: 0,
                                param3: 0,
                            });
                        }
                    }
                    self.close_window(close_id);
                } else if let Some(min_id) = min_clicked {
                    self.minimize_window(min_id);
                } else if let Some(max_id) = max_clicked {
                    self.toggle_maximize_window(max_id);
                } else if let Some(win_id) = clicked_win {
                    self.bring_to_front(win_id);
                    if let Some((d_id, ox, oy)) = drag_started {
                        self.dragging = Some((d_id, ox, oy));
                    } else if let Some((c_id, kind, cx, cy)) = client_click {
                        if let Some(win) = self.windows.iter_mut().find(|w| w.id == c_id) {
                            match kind {
                                WindowKind::FileManager if button == MouseButton::Left => {
                                    win.handle_file_manager_click(cx, cy);
                                }
                                WindowKind::UserApp => {
                                    win.push_user_event(GuiUserEvent {
                                        kind: GUI_EVENT_MOUSE_DOWN,
                                        param1: cx as i32,
                                        param2: cy as i32,
                                        param3: Self::mouse_button_code(button),
                                    });
                                }
                                _ => {}
                            }
                        }
                    }
                } else if button == MouseButton::Left {
                    // 3. Clicked on desktop background: check desktop shortcut icons
                    for (idx, (_, _, action)) in DESKTOP_ICONS.iter().enumerate() {
                        let ix = 20isize;
                        let iy = 24isize + (idx as isize) * 76;
                        if x >= ix && x < ix + 68 && y >= iy && y < iy + 64 {
                            self.pending_actions.push(*action);
                            return;
                        }
                    }
                }
            }

            InputEvent::MouseUp { button, x, y } => {
                if button == MouseButton::Left {
                    self.dragging = None;
                }
                if let Some(fid) = self.focused_window {
                    if let Some(win) = self.windows.iter_mut().find(|w| w.id == fid) {
                        if win.kind == WindowKind::UserApp && !win.minimized {
                            let cx = x - (win.x + BORDER_WIDTH as isize);
                            let cy = y - (win.y + (BORDER_WIDTH + TITLEBAR_HEIGHT) as isize);
                            win.push_user_event(GuiUserEvent {
                                kind: GUI_EVENT_MOUSE_UP,
                                param1: cx as i32,
                                param2: cy as i32,
                                param3: Self::mouse_button_code(button),
                            });
                        }
                    }
                }
            }

            InputEvent::MouseMove { x, y, .. } => {
                if let Some((win_id, ox, oy)) = self.dragging {
                    if let Some(win) = self.windows.iter_mut().find(|w| w.id == win_id) {
                        win.x = x - ox;
                        win.y = (y - oy).max(0);
                    }
                } else if let Some(fid) = self.focused_window {
                    if let Some(win) = self.windows.iter_mut().find(|w| w.id == fid) {
                        if win.kind == WindowKind::UserApp && !win.minimized {
                            let cx = x - (win.x + BORDER_WIDTH as isize);
                            let cy = y - (win.y + (BORDER_WIDTH + TITLEBAR_HEIGHT) as isize);
                            if cx >= 0 && cy >= 0 && (cx as usize) < win.width && (cy as usize) < win.height {
                                win.push_user_event(GuiUserEvent {
                                    kind: GUI_EVENT_MOUSE_MOVE,
                                    param1: cx as i32,
                                    param2: cy as i32,
                                    param3: 0,
                                });
                            }
                        }
                    }
                }
            }

            _ => {}
        }
    }

    /// Renders the wallpaper, desktop icons, windows, taskbar, Start Menu, and mouse cursor,
    /// then flushes the backbuffer to hardware.
    pub fn render_desktop(&mut self) {
        self.is_dirty = false;

        // 1. Copy cached wallpaper
        self.render_wallpaper();

        // 2. Draw desktop shortcut icons
        self.render_desktop_icons();

        // 3. Draw all non-minimized windows in Z-order (back to front)
        for win in self.windows.iter() {
            win.render_frame(&mut self.backbuffer);
        }

        // 4. Draw Bottom Taskbar & optional Start Menu popup
        self.render_taskbar();
        if self.start_menu_open {
            self.render_start_menu();
        }

        // 5. Render Mouse Cursor
        {
            let mouse = MOUSE.lock();
            mouse.render(&mut self.backbuffer);
        }

        // 6. Hardware Flush
        self.backbuffer.flush_to(self.hw_address, self.hw_stride, self.screen_height);
    }

    fn render_wallpaper(&mut self) {
        if self.wallpaper_cache.len() == self.backbuffer.buffer.len() {
            self.backbuffer.buffer.copy_from_slice(&self.wallpaper_cache);
        } else {
            Self::build_wallpaper_into(&mut self.backbuffer, self.screen_width, self.screen_height);
        }
    }

    fn render_desktop_icons(&mut self) {
        for (idx, (label, accent, _)) in DESKTOP_ICONS.iter().enumerate() {
            let ix = 20usize;
            let iy = 24usize + idx * 76;
            if iy + 64 >= self.screen_height.saturating_sub(TASKBAR_HEIGHT) {
                break;
            }
            self.backbuffer.fill_rect(ix + 14, iy + 4, 38, 34, Color::PANEL_BG);
            self.backbuffer.draw_rect(ix + 14, iy + 4, 38, 34, *accent);
            self.backbuffer.fill_rect(ix + 18, iy + 8, 30, 8, *accent);
            let text_w = label.len() * CHAR_WIDTH;
            let tx = ix + 33usize.saturating_sub(text_w / 2);
            self.backbuffer.draw_string(tx, iy + 44, label, Color::WHITE, None);
        }
    }

    fn render_start_menu(&mut self) {
        let tb_y = self.screen_height.saturating_sub(TASKBAR_HEIGHT);
        let menu_w = 210usize;
        let item_h = 28usize;
        let total_items = START_MENU_ITEMS.len() + 1;
        let menu_h = total_items * item_h + 12;
        let menu_x = 8usize;
        let menu_y = tb_y.saturating_sub(menu_h);

        self.backbuffer.fill_rect(menu_x, menu_y, menu_w, menu_h, Color::DARK_SLATE);
        self.backbuffer.draw_rect(menu_x, menu_y, menu_w, menu_h, Color::TITLE_ACTIVE);

        for (i, (label, _)) in START_MENU_ITEMS.iter().enumerate() {
            let iy = menu_y + 6 + i * item_h;
            self.backbuffer.fill_rect(menu_x + 6, iy, menu_w - 12, item_h - 4, Color::PANEL_BG);
            self.backbuffer.draw_string(menu_x + 14, iy + 4, label, Color::LIGHT_SLATE, None);
        }

        let exit_y = menu_y + 6 + START_MENU_ITEMS.len() * item_h;
        self.backbuffer.fill_rect(menu_x + 6, exit_y, menu_w - 12, item_h - 4, Color::CLOSE_BUTTON);
        self.backbuffer.draw_string(menu_x + 14, exit_y + 4, "Exit Desktop -> CLI", Color::WHITE, None);
    }

    fn render_taskbar(&mut self) {
        let w = self.screen_width;
        let h = self.screen_height;
        let tb_y = h.saturating_sub(TASKBAR_HEIGHT);

        self.backbuffer.fill_rect(0, tb_y, w, TASKBAR_HEIGHT, Color::TASKBAR_BG);
        self.backbuffer.draw_line(0, tb_y as isize, w as isize, tb_y as isize, Color::BORDER);

        // Start button ("NullOS")
        let logo_w = 80;
        let start_bg = if self.start_menu_open { Color::CYAN } else { Color::TITLE_ACTIVE };
        self.backbuffer.fill_rect(8, tb_y + 4, logo_w, TASKBAR_HEIGHT - 8, start_bg);
        self.backbuffer.draw_string(18, tb_y + 8, "NullOS", Color::WHITE, None);

        // Active window buttons in taskbar
        let mut tab_x = 100;
        let exit_btn_w = 90;
        let exit_btn_x = w.saturating_sub(exit_btn_w + 12);
        let max_tab_x = exit_btn_x.saturating_sub(110);

        for win in self.windows.iter() {
            let tab_w = 140;
            if tab_x + tab_w > max_tab_x {
                break;
            }

            let tab_color = if win.is_focused && !win.minimized {
                Color::TITLE_BAR
            } else {
                Color::PANEL_BG
            };

            self.backbuffer.fill_rect(tab_x, tb_y + 4, tab_w, TASKBAR_HEIGHT - 8, tab_color);
            self.backbuffer.draw_rect(tab_x, tb_y + 4, tab_w, TASKBAR_HEIGHT - 8, Color::BORDER);

            self.backbuffer.draw_string_bounded(tab_x + 6, tb_y + 8, &win.title, tab_w.saturating_sub(12), Color::LIGHT_SLATE, None);

            tab_x += tab_w + 6;
        }

        // Right-aligned "[Exit GUI]" button
        self.backbuffer.fill_rect(exit_btn_x, tb_y + 4, exit_btn_w, TASKBAR_HEIGHT - 8, Color::CLOSE_BUTTON);
        self.backbuffer.draw_string(exit_btn_x + 10, tb_y + 8, "Exit GUI", Color::WHITE, None);

        // Right-aligned uptime
        let ticks = crate::arch::x86_64::apic::get_ticks();
        let uptime_secs = ticks / 100;
        let mut buf = [0u8; 32];
        buf[0..4].copy_from_slice(b"Up: ");
        let mut pos = 4;
        let mut n = uptime_secs;
        if n == 0 {
            buf[pos] = b'0';
            pos += 1;
        } else {
            let start = pos;
            while n > 0 {
                buf[pos] = b'0' + (n % 10) as u8;
                pos += 1;
                n /= 10;
            }
            buf[start..pos].reverse();
        }
        buf[pos] = b's';
        pos += 1;

        if let Ok(clock_str) = core::str::from_utf8(&buf[..pos]) {
            let clock_x = exit_btn_x.saturating_sub(clock_str.len() * CHAR_WIDTH + 16);
            self.backbuffer.draw_string(clock_x, tb_y + 8, clock_str, Color::LIGHT_SLATE, None);
        }
    }
}

pub static WINDOW_MANAGER: Spinlock<Option<WindowManager>> = Spinlock::new(None);
