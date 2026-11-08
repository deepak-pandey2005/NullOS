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
use crate::graphics::event::{InputEvent, MouseButton};
use crate::graphics::mouse::MOUSE;
use crate::graphics::window::Window;
use crate::sync::Spinlock;

pub const TASKBAR_HEIGHT: usize = 32;

/// Global desktop window manager instance.
pub struct WindowManager {
    pub screen_width: usize,
    pub screen_height: usize,
    pub hw_address: u64,
    pub hw_stride: usize,
    pub backbuffer: Canvas,
    pub windows: Vec<Window>,
    pub focused_window: Option<u32>,
    pub dragging: Option<(u32, isize, isize)>,
    next_window_id: u32,
}

impl WindowManager {
    pub fn new(width: usize, height: usize, stride: usize, hw_address: u64) -> Self {
        Self {
            screen_width: width,
            screen_height: height,
            hw_address,
            hw_stride: stride,
            backbuffer: Canvas::new(width, height, stride),
            windows: Vec::new(),
            focused_window: None,
            dragging: None,
            next_window_id: 1,
        }
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
        id
    }

    /// Closes and removes a window by ID.
    pub fn close_window(&mut self, id: u32) {
        if let Some(pos) = self.windows.iter().position(|w| w.id == id) {
            self.windows.remove(pos);
            if self.focused_window == Some(id) {
                self.focused_window = self.windows.last().map(|w| w.id);
                if let Some(top) = self.windows.last_mut() {
                    top.is_focused = true;
                }
            }
        }
    }

    /// Brings the specified window to the front of the Z-order.
    pub fn bring_to_front(&mut self, id: u32) {
        if let Some(pos) = self.windows.iter().position(|w| w.id == id) {
            let mut win = self.windows.remove(pos);
            for w in self.windows.iter_mut() {
                w.is_focused = false;
            }
            win.is_focused = true;
            self.windows.push(win);
            self.focused_window = Some(id);
        }
    }

    /// Dispatches an input event to the window manager.
    pub fn handle_event(&mut self, event: InputEvent) {
        match event {
            InputEvent::MouseDown { button: MouseButton::Left, x, y } => {
                // Check windows from top to bottom (reverse order)
                let mut clicked_win = None;
                let mut close_clicked = None;
                let mut drag_started = None;

                for win in self.windows.iter().rev() {
                    if win.contains_point(x, y) {
                        clicked_win = Some(win.id);
                        if win.is_close_button(x, y) {
                            close_clicked = Some(win.id);
                        } else if win.is_titlebar(x, y) && win.draggable {
                            drag_started = Some((win.id, x - win.x, y - win.y));
                        }
                        break;
                    }
                }

                if let Some(close_id) = close_clicked {
                    self.close_window(close_id);
                } else if let Some(win_id) = clicked_win {
                    self.bring_to_front(win_id);
                    if let Some((d_id, ox, oy)) = drag_started {
                        self.dragging = Some((d_id, ox, oy));
                    }
                }
            }

            InputEvent::MouseUp { button: MouseButton::Left, .. } => {
                self.dragging = None;
            }

            InputEvent::MouseMove { x, y, .. } => {
                if let Some((win_id, ox, oy)) = self.dragging {
                    if let Some(win) = self.windows.iter_mut().find(|w| w.id == win_id) {
                        win.x = x - ox;
                        win.y = (y - oy).max(0);
                    }
                }
            }

            _ => {}
        }
    }

    /// Renders the wallpaper, desktop elements, windows, taskbar, and mouse cursor,
    /// then flushes the backbuffer to hardware.
    pub fn render_desktop(&mut self) {
        // 1. Draw Wallpaper: Modern deep slate gradient with grid
        self.render_wallpaper();

        // 2. Draw all windows in Z-order (back to front)
        for win in self.windows.iter() {
            win.render_frame(&mut self.backbuffer);
        }

        // 3. Draw Bottom Taskbar
        self.render_taskbar();

        // 4. Render Mouse Cursor
        {
            let mouse = MOUSE.lock();
            mouse.render(&mut self.backbuffer);
        }

        // 5. Hardware Flush
        self.backbuffer.flush_to(self.hw_address, self.hw_stride, self.screen_height);
    }

    fn render_wallpaper(&mut self) {
        let w = self.screen_width;
        let h = self.screen_height;

        // Gradient top-to-bottom
        for py in 0..h {
            let factor = (py * 30) / h.max(1);
            let r = 15u8.saturating_sub(factor as u8);
            let g = 23u8.saturating_sub(factor as u8);
            let b = 42u8.saturating_sub(factor as u8);
            let color = Color::rgb(r, g, b);

            let row_start = py * self.backbuffer.stride;
            self.backbuffer.buffer[row_start..row_start + w].fill(color.to_u32());
        }

        // Subtle geometric grid
        let grid_color = Color::rgba(255, 255, 255, 8);
        for x in (0..w).step_by(40) {
            self.backbuffer.draw_line(x as isize, 0, x as isize, h as isize, grid_color);
        }
        for y in (0..h).step_by(40) {
            self.backbuffer.draw_line(0, y as isize, w as isize, y as isize, grid_color);
        }

        // Desktop Watermark / Branding
        let brand_str = "NullOS 64-Bit Operating System";
        let brand_x = w.saturating_sub(brand_str.len() * CHAR_WIDTH + 24);
        let brand_y = h.saturating_sub(TASKBAR_HEIGHT + CHAR_HEIGHT + 16);
        self.backbuffer.draw_string(brand_x, brand_y, brand_str, Color::rgba(255, 255, 255, 60), None);
    }

    fn render_taskbar(&mut self) {
        let w = self.screen_width;
        let h = self.screen_height;
        let tb_y = h.saturating_sub(TASKBAR_HEIGHT);

        // Taskbar background & top border
        self.backbuffer.fill_rect(0, tb_y, w, TASKBAR_HEIGHT, Color::TASKBAR_BG);
        self.backbuffer.draw_line(0, tb_y as isize, w as isize, tb_y as isize, Color::BORDER);

        // "Start" / Logo badge
        let logo_w = 80;
        self.backbuffer.fill_rect(8, tb_y + 4, logo_w, TASKBAR_HEIGHT - 8, Color::TITLE_ACTIVE);
        self.backbuffer.draw_string(18, tb_y + 8, "NullOS", Color::WHITE, None);

        // Active window buttons in taskbar
        let mut tab_x = 100;
        for win in self.windows.iter() {
            let tab_w = 140;
            if tab_x + tab_w > w.saturating_sub(180) {
                break;
            }

            let tab_color = if win.is_focused {
                Color::TITLE_BAR
            } else {
                Color::PANEL_BG
            };

            self.backbuffer.fill_rect(tab_x, tb_y + 4, tab_w, TASKBAR_HEIGHT - 8, tab_color);
            self.backbuffer.draw_rect(tab_x, tb_y + 4, tab_w, TASKBAR_HEIGHT - 8, Color::BORDER);

            let max_chars = (tab_w - 12) / CHAR_WIDTH;
            let display_title: alloc::string::String = win.title.chars().take(max_chars).collect();
            self.backbuffer.draw_string(tab_x + 6, tb_y + 8, &display_title, Color::LIGHT_SLATE, None);

            tab_x += tab_w + 6;
        }

        // Right-aligned status stats (clock / uptime ticks)
        let ticks = crate::arch::x86_64::apic::get_ticks();
        let uptime_secs = ticks / 100;
        let clock_str = alloc::format!("Up: {}s", uptime_secs);
        let clock_x = w.saturating_sub(clock_str.len() * CHAR_WIDTH + 16);
        self.backbuffer.draw_string(clock_x, tb_y + 8, &clock_str, Color::LIGHT_SLATE, None);
    }
}

pub static WINDOW_MANAGER: Spinlock<Option<WindowManager>> = Spinlock::new(None);
