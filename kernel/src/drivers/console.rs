//! High-Performance Framebuffer Text Console Driver with ANSI Color Support.
//!
//! Features:
//! - Thread-safe & interrupt-safe synchronization using Spinlock.
//! - Optimized fast burst memory copy for instantaneous screen scrolling.
//! - ANSI escape code parsing for colored kernel diagnostic logs.
//! - Antialiased vector-quality font rasterization via noto-sans-mono-bitmap.
//!
//! // TODO(phase12-m30-gui): Implement double-buffered 2D graphics canvas and window compositor:
//! // 1. Allocate an off-screen secondary backbuffer to prevent tearing and flicker during fast animations.
//! // 2. Add 2D drawing primitives: line (Bresenham algorithm), rect, filled rect, and bitmap sprite blitting.
//! // 3. Implement overlapping Window abstraction with title bars, draggable headers, and dirty region redraws.
//! // 4. Hardware Mouse Cursor: Render transparent mouse arrow sprite updated on PS/2 / USB mouse input events.

use crate::sync::Spinlock;
use bootinfo::{FramebufferInfo, PixelFormat};
use core::fmt;
use noto_sans_mono_bitmap::{get_raster, FontWeight, RasterHeight};

const FONT_HEIGHT: RasterHeight = RasterHeight::Size16;
const CHAR_HEIGHT: usize = 16;
const CHAR_WIDTH: usize = 9;

const DEFAULT_FG: u32 = 0x00E2E8F0; // Clean light slate text
const DEFAULT_BG: u32 = 0x000F172A; // Dark slate blue background

#[derive(Clone, Copy, PartialEq, Eq)]
enum AnsiState {
    Normal,
    Esc,
    Bracket,
    Param(u32),
}

pub struct FramebufferConsole {
    address: u64,
    width: usize,
    height: usize,
    stride: usize,
    _bytes_per_pixel: usize,
    _pixel_format: PixelFormat,
    cursor_x: usize,
    cursor_y: usize,
    fg_color: u32,
    bg_color: u32,
    ansi_state: AnsiState,
}

static CONSOLE: Spinlock<Option<FramebufferConsole>> = Spinlock::new(None);

impl FramebufferConsole {
    pub fn new(fb: &FramebufferInfo) -> Self {
        Self {
            address: fb.address,
            width: fb.width,
            height: fb.height,
            stride: fb.stride,
            _bytes_per_pixel: fb.bytes_per_pixel,
            _pixel_format: fb.pixel_format,
            cursor_x: 0,
            cursor_y: 0,
            fg_color: DEFAULT_FG,
            bg_color: DEFAULT_BG,
            ansi_state: AnsiState::Normal,
        }
    }

    pub fn clear(&mut self) {
        let ptr = self.address as *mut u32;
        let total_pixels = self.stride * self.height;
        let color = self.bg_color;
        for i in 0..total_pixels {
            unsafe {
                *ptr.add(i) = color;
            }
        }
        self.cursor_x = 0;
        self.cursor_y = 0;
    }

    pub fn write_char(&mut self, c: char) {
        // ANSI escape sequence state machine
        match self.ansi_state {
            AnsiState::Normal => {
                if c == '\x1b' {
                    self.ansi_state = AnsiState::Esc;
                    return;
                }
            }
            AnsiState::Esc => {
                if c == '[' {
                    self.ansi_state = AnsiState::Bracket;
                } else {
                    self.ansi_state = AnsiState::Normal;
                }
                return;
            }
            AnsiState::Bracket => {
                if c.is_ascii_digit() {
                    self.ansi_state = AnsiState::Param(c.to_digit(10).unwrap_or(0));
                } else if c == 'm' {
                    self.apply_sgr(0);
                    self.ansi_state = AnsiState::Normal;
                } else {
                    self.ansi_state = AnsiState::Normal;
                }
                return;
            }
            AnsiState::Param(n) => {
                if c.is_ascii_digit() {
                    let next = n.saturating_mul(10).saturating_add(c.to_digit(10).unwrap_or(0));
                    self.ansi_state = AnsiState::Param(next);
                    return;
                } else if c == 'm' {
                    self.apply_sgr(n);
                    self.ansi_state = AnsiState::Normal;
                    return;
                } else {
                    self.ansi_state = AnsiState::Normal;
                    return;
                }
            }
        }

        // Standard character rendering
        match c {
            '\n' => self.newline(),
            '\r' => self.cursor_x = 0,
            '\x08' => {
                if self.cursor_x >= CHAR_WIDTH {
                    self.cursor_x -= CHAR_WIDTH;
                } else if self.cursor_y >= CHAR_HEIGHT {
                    self.cursor_y -= CHAR_HEIGHT;
                    self.cursor_x = (self.width / CHAR_WIDTH) * CHAR_WIDTH - CHAR_WIDTH;
                }
            }
            '\t' => {
                for _ in 0..4 {
                    self.write_char(' ');
                }
            }
            ' ' => {
                if self.cursor_x + CHAR_WIDTH > self.width {
                    self.newline();
                }
                self.clear_char(self.cursor_x, self.cursor_y);
                self.cursor_x += CHAR_WIDTH;
            }
            c if !c.is_control() => {
                if self.cursor_x + CHAR_WIDTH > self.width {
                    self.newline();
                }
                self.draw_char(self.cursor_x, self.cursor_y, c);
                self.cursor_x += CHAR_WIDTH;
            }
            _ => {}
        }
    }

    /// Handles ANSI SGR (Select Graphic Rendition) color codes.
    fn apply_sgr(&mut self, code: u32) {
        match code {
            0 => self.fg_color = DEFAULT_FG,          // Reset
            30 => self.fg_color = 0x000F172A,         // Black
            31 => self.fg_color = 0x00EF4444,         // Red
            32 => self.fg_color = 0x0022C55E,         // Green
            33 => self.fg_color = 0x00EAB308,         // Yellow
            34 => self.fg_color = 0x003B82F6,         // Blue
            35 => self.fg_color = 0x00A855F7,         // Magenta
            36 => self.fg_color = 0x0006B6D4,         // Cyan
            37 => self.fg_color = 0x00F8FAFC,         // White
            90 => self.fg_color = 0x0064748B,         // Bright Gray
            91 => self.fg_color = 0x00F87171,         // Bright Red
            92 => self.fg_color = 0x004ADE80,         // Bright Green
            93 => self.fg_color = 0x00FDE047,         // Bright Yellow
            94 => self.fg_color = 0x0060A5FA,         // Bright Blue
            95 => self.fg_color = 0x00C084FC,         // Bright Magenta
            96 => self.fg_color = 0x0022D3EE,         // Bright Cyan
            _ => {}
        }
    }

    fn newline(&mut self) {
        self.cursor_x = 0;
        self.cursor_y += CHAR_HEIGHT;
        if self.cursor_y + CHAR_HEIGHT > self.height {
            self.scroll_up();
        }
    }

    /// High-performance screen scrolling via block memory movement.
    fn scroll_up(&mut self) {
        let ptr = self.address as *mut u32;
        let lines_to_move = self.height - CHAR_HEIGHT;
        let total_pixels_to_move = lines_to_move * self.stride;

        unsafe {
            // Fast sequential block copy
            core::ptr::copy(ptr.add(CHAR_HEIGHT * self.stride), ptr, total_pixels_to_move);

            // Fast clear of the newly exposed bottom row
            let clear_ptr = ptr.add(total_pixels_to_move);
            let clear_count = CHAR_HEIGHT * self.stride;
            for i in 0..clear_count {
                *clear_ptr.add(i) = self.bg_color;
            }
        }

        self.cursor_y = lines_to_move;
    }

    fn clear_char(&mut self, x_pos: usize, y_pos: usize) {
        let ptr = self.address as *mut u32;
        for row_idx in 0..CHAR_HEIGHT {
            if y_pos + row_idx >= self.height {
                break;
            }
            let row_offset = (y_pos + row_idx) * self.stride;
            for col_idx in 0..CHAR_WIDTH {
                if x_pos + col_idx >= self.width {
                    break;
                }
                unsafe {
                    *ptr.add(row_offset + x_pos + col_idx) = self.bg_color;
                }
            }
        }
    }

    fn draw_char(&mut self, x_pos: usize, y_pos: usize, c: char) {
        let raster = get_raster(c, FontWeight::Regular, FONT_HEIGHT)
            .unwrap_or_else(|| get_raster('?', FontWeight::Regular, FONT_HEIGHT).unwrap());

        let ptr = self.address as *mut u32;

        for (row_idx, row) in raster.raster().iter().enumerate() {
            if y_pos + row_idx >= self.height {
                break;
            }
            let row_offset = (y_pos + row_idx) * self.stride;

            for (col_idx, &intensity) in row.iter().enumerate() {
                if x_pos + col_idx >= self.width {
                    break;
                }

                let pixel_color = if intensity > 128 {
                    self.fg_color
                } else if intensity > 30 {
                    self.blend_color(self.fg_color, self.bg_color, intensity)
                } else {
                    self.bg_color
                };

                unsafe {
                    *ptr.add(row_offset + x_pos + col_idx) = pixel_color;
                }
            }
        }
    }

    fn blend_color(&self, fg: u32, bg: u32, alpha: u8) -> u32 {
        let a = alpha as u32;
        let inv_a = 255 - a;

        let fg_r = (fg >> 16) & 0xFF;
        let fg_g = (fg >> 8) & 0xFF;
        let fg_b = fg & 0xFF;

        let bg_r = (bg >> 16) & 0xFF;
        let bg_g = (bg >> 8) & 0xFF;
        let bg_b = bg & 0xFF;

        let r = (fg_r * a + bg_r * inv_a) / 255;
        let g = (fg_g * a + bg_g * inv_a) / 255;
        let b = (fg_b * a + bg_b * inv_a) / 255;

        (r << 16) | (g << 8) | b
    }
}

impl fmt::Write for FramebufferConsole {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for c in s.chars() {
            self.write_char(c);
        }
        Ok(())
    }
}

pub fn init(fb: &FramebufferInfo) {
    if fb.address == 0 || fb.width == 0 || fb.height == 0 {
        return;
    }
    let mut console = FramebufferConsole::new(fb);
    console.clear();
    *CONSOLE.lock() = Some(console);
}

pub fn write_str(s: &str) {
    if let Some(ref mut console) = *CONSOLE.lock() {
        let _ = <FramebufferConsole as fmt::Write>::write_str(console, s);
    }
}
