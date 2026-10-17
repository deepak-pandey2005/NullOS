use bootinfo::{FramebufferInfo, PixelFormat};
use core::fmt;
use noto_sans_mono_bitmap::{get_raster, FontWeight, RasterHeight};

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
}

static mut CONSOLE: Option<FramebufferConsole> = None;

const FONT_HEIGHT: RasterHeight = RasterHeight::Size16;
const CHAR_HEIGHT: usize = 16;
const CHAR_WIDTH: usize = 9; // Size16 font width is ~9 px

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
            fg_color: 0x00E2E8F0, // Clean light slate text
            bg_color: 0x000F172A, // Dark slate blue background
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
        match c {
            '\n' => self.newline(),
            '\r' => self.cursor_x = 0,
            '\t' => {
                for _ in 0..4 {
                    self.write_char(' ');
                }
            }
            c => {
                if self.cursor_x + CHAR_WIDTH > self.width {
                    self.newline();
                }
                self.draw_char(self.cursor_x, self.cursor_y, c);
                self.cursor_x += CHAR_WIDTH;
            }
        }
    }

    fn newline(&mut self) {
        self.cursor_x = 0;
        self.cursor_y += CHAR_HEIGHT;
        if self.cursor_y + CHAR_HEIGHT > self.height {
            self.scroll_up();
        }
    }

    fn scroll_up(&mut self) {
        let ptr = self.address as *mut u32;
        let lines_to_move = self.height - CHAR_HEIGHT;

        // Move rows up
        for y in 0..lines_to_move {
            let src = (y + CHAR_HEIGHT) * self.stride;
            let dst = y * self.stride;
            for x in 0..self.width {
                unsafe {
                    *ptr.add(dst + x) = *ptr.add(src + x);
                }
            }
        }

        // Clear bottom row
        for y in lines_to_move..self.height {
            let dst = y * self.stride;
            for x in 0..self.width {
                unsafe {
                    *ptr.add(dst + x) = self.bg_color;
                }
            }
        }

        self.cursor_y = lines_to_move;
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
                    // Mild antialiasing
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
    unsafe {
        CONSOLE = Some(console);
    }
}

pub fn write_str(s: &str) {
    unsafe {
        if let Some(ref mut console) = CONSOLE {
            let _ = <FramebufferConsole as fmt::Write>::write_str(console, s);
        }
    }
}
