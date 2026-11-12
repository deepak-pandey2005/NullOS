//! Window Abstraction, Title Bar Decorations, and Surface Compositing.
//!
//! Windows represent individual GUI surfaces with decorated title bars,
//! close buttons, dragging boundaries, and dedicated client canvases.

use alloc::collections::VecDeque;
use alloc::string::String;
use alloc::vec::Vec;
use crate::graphics::canvas::{Canvas, CHAR_HEIGHT, CHAR_WIDTH};
use crate::graphics::color::Color;
use crate::graphics::event::GuiUserEvent;

pub const TITLEBAR_HEIGHT: usize = 26;
pub const BORDER_WIDTH: usize = 2;
pub const CLOSE_BTN_SIZE: usize = 16;

/// Window role for compositor routing and built-in desktop applets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowKind {
    Standard,
    Terminal,
    KernelLog,
    SystemMonitor,
    FileManager,
    UserApp,
}

/// A top-level desktop window.
pub struct Window {
    pub id: u32,
    pub kind: WindowKind,
    pub owner_pid: Option<u64>,
    pub title: String,
    pub x: isize,
    pub y: isize,
    pub width: usize,
    pub height: usize,
    pub surface: Canvas,
    pub is_focused: bool,
    pub closable: bool,
    pub draggable: bool,
    pub minimized: bool,
    pub maximized: bool,
    pub saved_rect: (isize, isize, usize, usize),
    pub term_lines: Vec<String>,
    pub input_line: String,
    pub user_events: VecDeque<GuiUserEvent>,
    pub fm_path: String,
    pub fm_entries: Vec<(String, bool, u64)>,
    pub fm_preview: Vec<String>,
}

impl Window {
    /// Creates a new Window with a dedicated client surface.
    pub fn new(id: u32, title: &str, x: isize, y: isize, width: usize, height: usize) -> Self {
        Self {
            id,
            kind: WindowKind::Standard,
            owner_pid: None,
            title: String::from(title),
            x,
            y,
            width,
            height,
            surface: Canvas::new(width, height, width),
            is_focused: false,
            closable: true,
            draggable: true,
            minimized: false,
            maximized: false,
            saved_rect: (x, y, width, height),
            term_lines: Vec::new(),
            input_line: String::new(),
            user_events: VecDeque::new(),
            fm_path: String::from("/"),
            fm_entries: Vec::new(),
            fm_preview: Vec::new(),
        }
    }

    pub fn push_user_event(&mut self, ev: GuiUserEvent) {
        if self.user_events.len() < 256 {
            self.user_events.push_back(ev);
        }
    }

    pub fn pop_user_event(&mut self) -> Option<GuiUserEvent> {
        self.user_events.pop_front()
    }

    /// Loads directory entries from VFS for the File Manager window.
    pub fn navigate_file_manager(&mut self, path: &str) {
        self.fm_path = String::from(path);
        self.fm_entries.clear();
        self.fm_preview.clear();
        if let Ok(inode) = crate::fs::vfs::VFS.resolve_path(path) {
            if let Ok(entries) = inode.read_dir() {
                for entry in entries {
                    let is_dir = entry.file_type == crate::fs::vfs::FileType::Directory;
                    let size = if is_dir {
                        0
                    } else {
                        inode.lookup(&entry.name).map(|n| n.stat().size as u64).unwrap_or(0)
                    };
                    self.fm_entries.push((entry.name, is_dir, size));
                }
            }
        }
        self.render_file_manager_surface();
    }

    /// Handles a mouse click in client-local coordinates inside the File Manager.
    pub fn handle_file_manager_click(&mut self, _local_x: isize, local_y: isize) {
        let list_start_y = 38isize;
        let row_h = (CHAR_HEIGHT + 2) as isize;
        if local_y < list_start_y {
            return;
        }
        let row_idx = ((local_y - list_start_y) / row_h) as usize;
        if row_idx == 0 && self.fm_path != "/" {
            // Navigate to parent directory
            let parent = if let Some(pos) = self.fm_path.trim_end_matches('/').rfind('/') {
                if pos == 0 { String::from("/") } else { String::from(&self.fm_path[..pos]) }
            } else {
                String::from("/")
            };
            self.navigate_file_manager(&parent);
            return;
        }
        let entry_idx = if self.fm_path != "/" { row_idx.saturating_sub(1) } else { row_idx };
        if let Some((name, is_dir, _)) = self.fm_entries.get(entry_idx).cloned() {
            let full = if self.fm_path == "/" {
                alloc::format!("/{}", name)
            } else {
                alloc::format!("{}/{}", self.fm_path, name)
            };
            if is_dir {
                self.navigate_file_manager(&full);
            } else {
                self.fm_preview.clear();
                self.fm_preview.push(alloc::format!("File: {}", full));
                if let Ok(inode) = crate::fs::vfs::VFS.resolve_path(&full) {
                    let mut buf = [0u8; 256];
                    if let Ok(n) = inode.read_at(0, &mut buf) {
                        if let Ok(text) = core::str::from_utf8(&buf[..n]) {
                            for line in text.lines().take(4) {
                                self.fm_preview.push(String::from(line));
                            }
                        } else {
                            self.fm_preview.push(alloc::format!("<binary data: {} bytes>", n));
                        }
                    }
                }
                self.render_file_manager_surface();
            }
        }
    }

    /// Renders the File Manager directory listing and preview pane.
    pub fn render_file_manager_surface(&mut self) {
        self.surface.clear(Color::PANEL_BG);
        self.surface.fill_rect(0, 0, self.width, 30, Color::DARK_SLATE);
        self.surface.draw_string_bounded(
            10,
            7,
            &alloc::format!("Path: {}", self.fm_path),
            self.width.saturating_sub(20),
            Color::CYAN,
            None,
        );
        self.surface.draw_line(0, 30, self.width as isize, 30, Color::BORDER);

        let mut y = 38usize;
        let row_h = CHAR_HEIGHT + 2;
        let preview_top = self.height.saturating_sub(88);

        if self.fm_path != "/" && y + row_h < preview_top {
            self.surface.draw_string(12, y, "[DIR]  ..", Color::YELLOW, None);
            y += row_h;
        }

        for (name, is_dir, size) in &self.fm_entries {
            if y + row_h >= preview_top {
                break;
            }
            let (label, color) = if *is_dir {
                (alloc::format!("[DIR]  {}/", name), Color::YELLOW)
            } else {
                (alloc::format!("[FILE] {} ({} B)", name, size), Color::LIGHT_SLATE)
            };
            self.surface.draw_string_bounded(12, y, &label, self.width.saturating_sub(24), color, None);
            y += row_h;
        }

        self.surface.draw_line(0, preview_top as isize, self.width as isize, preview_top as isize, Color::BORDER);
        self.surface.fill_rect(0, preview_top + 1, self.width, self.height.saturating_sub(preview_top + 1), Color::BLACK);
        let mut py = preview_top + 6;
        if self.fm_preview.is_empty() {
            self.surface.draw_string(10, py, "Click a directory to open or file to preview", Color::MUTED_TEXT, None);
        } else {
            for line in &self.fm_preview {
                if py + CHAR_HEIGHT > self.height {
                    break;
                }
                self.surface.draw_string_bounded(10, py, line, self.width.saturating_sub(20), Color::GREEN, None);
                py += CHAR_HEIGHT;
            }
        }
    }

    /// Redraws terminal lines and current input line onto client surface.
    pub fn render_terminal_surface(&mut self) {
        self.surface.clear(Color::BLACK);
        let max_lines = self.height / CHAR_HEIGHT;
        let mut y = 6;
        let lines_to_show = max_lines.saturating_sub(2);
        let start_idx = self.term_lines.len().saturating_sub(lines_to_show);

        for line in self.term_lines.iter().skip(start_idx) {
            let max_chars = (self.width.saturating_sub(16)) / CHAR_WIDTH;
            let display: String = line.chars().take(max_chars).collect();
            let color = if display.starts_with("nullos$") {
                Color::YELLOW
            } else if display.starts_with('[') {
                Color::CYAN
            } else {
                Color::LIGHT_SLATE
            };
            self.surface.draw_string(8, y, &display, color, None);
            y += CHAR_HEIGHT;
        }

        let prompt = alloc::format!("nullos$ {}{}", self.input_line, "_");
        let max_chars = (self.width.saturating_sub(16)) / CHAR_WIDTH;
        let display_prompt: String = prompt.chars().take(max_chars).collect();
        self.surface.draw_string(8, y, &display_prompt, Color::GREEN, None);
    }

    /// Redraws log lines onto client surface (for Kernel Log viewer window).
    pub fn render_log_surface(&mut self, logs: &[String]) {
        self.surface.clear(Color::DARK_SLATE);
        let max_lines = self.height / CHAR_HEIGHT;
        let mut y = 6;
        let lines_to_show = max_lines.saturating_sub(1);
        let start_idx = logs.len().saturating_sub(lines_to_show);

        let max_width = self.width.saturating_sub(16);
        for line in logs.iter().skip(start_idx) {
            let color = if line.contains("ERROR") || line.contains("PANIC") {
                Color::RED
            } else if line.contains("verified") || line.contains("passed") || line.contains("success") {
                Color::GREEN
            } else if line.starts_with('[') {
                Color::CYAN
            } else {
                Color::LIGHT_SLATE
            };
            self.surface.draw_string_bounded(8, y, line, max_width, color, None);
            y += CHAR_HEIGHT;
        }
    }

    /// Total outer width of the window including borders.
    #[inline]
    pub fn total_width(&self) -> usize {
        self.width + BORDER_WIDTH * 2
    }

    /// Total outer height of the window including title bar and borders.
    #[inline]
    pub fn total_height(&self) -> usize {
        self.height + TITLEBAR_HEIGHT + BORDER_WIDTH * 2
    }

    /// Checks if a global desktop coordinate (px, py) falls inside this window.
    pub fn contains_point(&self, px: isize, py: isize) -> bool {
        let tw = self.total_width() as isize;
        let th = self.total_height() as isize;
        px >= self.x && px < self.x + tw && py >= self.y && py < self.y + th
    }

    /// Checks if (px, py) falls within the window's draggable title bar.
    pub fn is_titlebar(&self, px: isize, py: isize) -> bool {
        let tw = self.total_width() as isize;
        let tb_h = (TITLEBAR_HEIGHT + BORDER_WIDTH) as isize;
        px >= self.x && px < self.x + tw && py >= self.y && py < self.y + tb_h
    }

    /// Checks if (px, py) hits the minimize button (`_`) to the left of maximize.
    pub fn is_minimize_button(&self, px: isize, py: isize) -> bool {
        let btn_x = self.x + (self.total_width() as isize) - (BORDER_WIDTH as isize) - (CLOSE_BTN_SIZE as isize) * 3 - 12;
        let btn_y = self.y + (BORDER_WIDTH as isize) + ((TITLEBAR_HEIGHT - CLOSE_BTN_SIZE) / 2) as isize;
        let s = CLOSE_BTN_SIZE as isize;
        px >= btn_x && px < btn_x + s && py >= btn_y && py < btn_y + s
    }

    /// Checks if (px, py) hits the maximize/restore button (`+`) to the left of close.
    pub fn is_maximize_button(&self, px: isize, py: isize) -> bool {
        let btn_x = self.x + (self.total_width() as isize) - (BORDER_WIDTH as isize) - (CLOSE_BTN_SIZE as isize) * 2 - 8;
        let btn_y = self.y + (BORDER_WIDTH as isize) + ((TITLEBAR_HEIGHT - CLOSE_BTN_SIZE) / 2) as isize;
        let s = CLOSE_BTN_SIZE as isize;
        px >= btn_x && px < btn_x + s && py >= btn_y && py < btn_y + s
    }

    /// Checks if (px, py) hits the close button at the top-right of the title bar.
    pub fn is_close_button(&self, px: isize, py: isize) -> bool {
        if !self.closable {
            return false;
        }
        let btn_x = self.x + (self.total_width() as isize) - (BORDER_WIDTH as isize) - (CLOSE_BTN_SIZE as isize) - 4;
        let btn_y = self.y + (BORDER_WIDTH as isize) + ((TITLEBAR_HEIGHT - CLOSE_BTN_SIZE) / 2) as isize;
        let btn_w = CLOSE_BTN_SIZE as isize;
        let btn_h = CLOSE_BTN_SIZE as isize;

        px >= btn_x && px < btn_x + btn_w && py >= btn_y && py < btn_y + btn_h
    }

    /// Renders the complete decorated window (frame, title bar, window controls, client surface) onto target canvas.
    pub fn render_frame(&self, target: &mut Canvas) {
        if self.minimized {
            return;
        }
        let origin_x = self.x;
        let origin_y = self.y;

        if origin_x >= target.width as isize || origin_y >= target.height as isize {
            return;
        }

        let tw = self.total_width();
        let th = self.total_height();

        // 1. Draw outer border
        let border_color = if self.is_focused {
            Color::TITLE_ACTIVE
        } else {
            Color::BORDER
        };

        let u_x = origin_x.max(0) as usize;
        let u_y = origin_y.max(0) as usize;
        target.fill_rect(u_x, u_y, tw, th, border_color);

        // 2. Draw title bar
        let tb_color = if self.is_focused {
            Color::TITLE_ACTIVE
        } else {
            Color::TITLE_BAR
        };
        let tb_x = (origin_x + BORDER_WIDTH as isize).max(0) as usize;
        let tb_y = (origin_y + BORDER_WIDTH as isize).max(0) as usize;
        target.fill_rect(tb_x, tb_y, self.width, TITLEBAR_HEIGHT, tb_color);

        // 3. Draw title text (leave room for 3 window buttons)
        let title_text_x = tb_x + 8;
        let title_text_y = tb_y + ((TITLEBAR_HEIGHT.saturating_sub(CHAR_HEIGHT)) / 2);
        let max_title_w = self.width.saturating_sub(CLOSE_BTN_SIZE * 3 + 28);
        target.draw_string_bounded(title_text_x, title_text_y, &self.title, max_title_w, Color::WHITE, None);

        // 4. Draw Window Control Buttons: [_] [+] [x]
        let btn_y = (origin_y + (BORDER_WIDTH as isize) + ((TITLEBAR_HEIGHT - CLOSE_BTN_SIZE) / 2) as isize).max(0) as usize;
        let min_x = (origin_x + (tw as isize) - (BORDER_WIDTH as isize) - (CLOSE_BTN_SIZE as isize) * 3 - 12).max(0) as usize;
        let max_x = (origin_x + (tw as isize) - (BORDER_WIDTH as isize) - (CLOSE_BTN_SIZE as isize) * 2 - 8).max(0) as usize;
        let close_x = (origin_x + (tw as isize) - (BORDER_WIDTH as isize) - (CLOSE_BTN_SIZE as isize) - 4).max(0) as usize;

        target.fill_rect(min_x, btn_y, CLOSE_BTN_SIZE, CLOSE_BTN_SIZE, Color::PANEL_BG);
        target.draw_char(
            min_x + ((CLOSE_BTN_SIZE.saturating_sub(CHAR_WIDTH)) / 2),
            btn_y + ((CLOSE_BTN_SIZE.saturating_sub(CHAR_HEIGHT)) / 2),
            '_',
            Color::WHITE,
            None,
        );

        target.fill_rect(max_x, btn_y, CLOSE_BTN_SIZE, CLOSE_BTN_SIZE, Color::PANEL_BG);
        target.draw_char(
            max_x + ((CLOSE_BTN_SIZE.saturating_sub(CHAR_WIDTH)) / 2),
            btn_y + ((CLOSE_BTN_SIZE.saturating_sub(CHAR_HEIGHT)) / 2),
            if self.maximized { '-' } else { '+' },
            Color::WHITE,
            None,
        );

        if self.closable {
            target.fill_rect(close_x, btn_y, CLOSE_BTN_SIZE, CLOSE_BTN_SIZE, Color::CLOSE_BUTTON);
            target.draw_char(
                close_x + ((CLOSE_BTN_SIZE.saturating_sub(CHAR_WIDTH)) / 2),
                btn_y + ((CLOSE_BTN_SIZE.saturating_sub(CHAR_HEIGHT)) / 2),
                'x',
                Color::WHITE,
                None,
            );
        }

        // 5. Blit client surface onto window interior
        let client_x = (origin_x + BORDER_WIDTH as isize).max(0) as usize;
        let client_y = (origin_y + (BORDER_WIDTH + TITLEBAR_HEIGHT) as isize).max(0) as usize;
        target.blit(client_x, client_y, &self.surface);
    }
}

