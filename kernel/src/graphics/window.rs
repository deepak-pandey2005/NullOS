//! Window Abstraction, Title Bar Decorations, and Surface Compositing.
//!
//! Windows represent individual GUI surfaces with decorated title bars,
//! close buttons, dragging boundaries, and dedicated client canvases.

use alloc::string::String;
use crate::graphics::canvas::{Canvas, CHAR_HEIGHT, CHAR_WIDTH};
use crate::graphics::color::Color;

pub const TITLEBAR_HEIGHT: usize = 26;
pub const BORDER_WIDTH: usize = 2;
pub const CLOSE_BTN_SIZE: usize = 16;

/// A top-level desktop window.
pub struct Window {
    pub id: u32,
    pub title: String,
    pub x: isize,
    pub y: isize,
    pub width: usize,
    pub height: usize,
    pub surface: Canvas,
    pub is_focused: bool,
    pub closable: bool,
    pub draggable: bool,
}

impl Window {
    /// Creates a new Window with a dedicated client surface.
    pub fn new(id: u32, title: &str, x: isize, y: isize, width: usize, height: usize) -> Self {
        Self {
            id,
            title: String::from(title),
            x,
            y,
            width,
            height,
            surface: Canvas::new(width, height, width),
            is_focused: false,
            closable: true,
            draggable: true,
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

    /// Renders the complete decorated window (frame, title bar, close button, client surface) onto target canvas.
    pub fn render_frame(&self, target: &mut Canvas) {
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

        // 3. Draw title text
        let title_text_x = tb_x + 8;
        let title_text_y = tb_y + ((TITLEBAR_HEIGHT.saturating_sub(CHAR_HEIGHT)) / 2);
        target.draw_string(title_text_x, title_text_y, &self.title, Color::WHITE, None);

        // 4. Draw Close Button [X]
        if self.closable {
            let btn_x = (origin_x + (tw as isize) - (BORDER_WIDTH as isize) - (CLOSE_BTN_SIZE as isize) - 4).max(0) as usize;
            let btn_y = (origin_y + (BORDER_WIDTH as isize) + ((TITLEBAR_HEIGHT - CLOSE_BTN_SIZE) / 2) as isize).max(0) as usize;

            target.fill_rect(btn_x, btn_y, CLOSE_BTN_SIZE, CLOSE_BTN_SIZE, Color::CLOSE_BUTTON);
            target.draw_char(
                btn_x + ((CLOSE_BTN_SIZE.saturating_sub(CHAR_WIDTH)) / 2),
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

