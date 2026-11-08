//! Double-Buffered 2D Graphics Canvas & Rendering Engine.
//!
//! Provides pixel plotting, Bresenham line rasterization, filled/outlined
//! rectangles, midpoint circles, font rendering via noto-sans-mono-bitmap,
//! and high-speed offscreen backbuffer flushing.

use alloc::vec;
use alloc::vec::Vec;
use crate::graphics::color::Color;
use noto_sans_mono_bitmap::{get_raster, FontWeight, RasterHeight};

const FONT_HEIGHT: RasterHeight = RasterHeight::Size16;
pub const CHAR_WIDTH: usize = 9;
pub const CHAR_HEIGHT: usize = 16;

/// 2D Drawing Surface backed by a continuous array of 32-bit ARGB pixels.
pub struct Canvas {
    pub width: usize,
    pub height: usize,
    pub stride: usize,
    pub buffer: Vec<u32>,
}

impl Canvas {
    /// Creates a new Canvas initialized with black pixels.
    pub fn new(width: usize, height: usize, stride: usize) -> Self {
        let size = stride * height;
        Self {
            width,
            height,
            stride,
            buffer: vec![0xFF000000; size],
        }
    }

    /// Clears the entire canvas surface with a uniform color.
    pub fn clear(&mut self, color: Color) {
        let val = color.to_u32();
        self.buffer.fill(val);
    }

    /// Sets a pixel at (x, y) with alpha blending support.
    #[inline]
    pub fn set_pixel(&mut self, x: usize, y: usize, color: Color) {
        if x >= self.width || y >= self.height {
            return;
        }
        let idx = match y.checked_mul(self.stride).and_then(|v| v.checked_add(x)) {
            Some(i) => i,
            None => {
                crate::println!("[CANVAS ERROR] overflow in set_pixel: x={}, y={}, width={}, height={}, stride={}", x, y, self.width, self.height, self.stride);
                return;
            }
        };
        if idx >= self.buffer.len() {
            return;
        }
        if color.a == 255 {
            self.buffer[idx] = color.to_u32();
        } else if color.a > 0 {
            let bg = Color::from_u32(self.buffer[idx]);
            self.buffer[idx] = color.blend_over(bg).to_u32();
        }
    }

    /// Retrieves the pixel color at (x, y).
    #[inline]
    pub fn get_pixel(&self, x: usize, y: usize) -> Color {
        if x >= self.width || y >= self.height {
            return Color::TRANSPARENT;
        }
        if let Some(idx) = y.checked_mul(self.stride).and_then(|v| v.checked_add(x)) {
            if idx < self.buffer.len() {
                return Color::from_u32(self.buffer[idx]);
            }
        }
        Color::TRANSPARENT
    }

    /// Draws a straight line between (x0, y0) and (x1, y1) using Bresenham's algorithm.
    pub fn draw_line(&mut self, mut x0: isize, mut y0: isize, x1: isize, y1: isize, color: Color) {
        let dx = (x1 - x0).abs();
        let dy = -(y1 - y0).abs();
        let sx = if x0 < x1 { 1 } else { -1 };
        let sy = if y0 < y1 { 1 } else { -1 };
        let mut err = dx + dy;

        loop {
            if x0 >= 0 && y0 >= 0 && (x0 as usize) < self.width && (y0 as usize) < self.height {
                self.set_pixel(x0 as usize, y0 as usize, color);
            }
            if x0 == x1 && y0 == y1 {
                break;
            }
            let e2 = 2 * err;
            if e2 >= dy {
                err += dy;
                x0 += sx;
            }
            if e2 <= dx {
                err += dx;
                y0 += sy;
            }
        }
    }

    /// Draws an outlined rectangle.
    pub fn draw_rect(&mut self, x: usize, y: usize, w: usize, h: usize, color: Color) {
        if w == 0 || h == 0 {
            return;
        }
        let x2 = (x + w).saturating_sub(1);
        let y2 = (y + h).saturating_sub(1);

        // Top & bottom horizontal lines
        for px in x..=x2 {
            self.set_pixel(px, y, color);
            self.set_pixel(px, y2, color);
        }
        // Left & right vertical lines
        for py in y..=y2 {
            self.set_pixel(x, py, color);
            self.set_pixel(x2, py, color);
        }
    }

    /// Draws a solid filled rectangle.
    pub fn fill_rect(&mut self, x: usize, y: usize, w: usize, h: usize, color: Color) {
        if x >= self.width || y >= self.height || w == 0 || h == 0 {
            return;
        }
        let max_x = (x + w).min(self.width);
        let max_y = (y + h).min(self.height);

        if color.a == 255 {
            let val = color.to_u32();
            for py in y..max_y {
                let row_start = py * self.stride + x;
                let row_end = row_start + (max_x - x);
                if row_end <= self.buffer.len() {
                    self.buffer[row_start..row_end].fill(val);
                }
            }
        } else {
            for py in y..max_y {
                for px in x..max_x {
                    self.set_pixel(px, py, color);
                }
            }
        }
    }

    /// Draws an outlined circle using the Midpoint Circle Algorithm.
    pub fn draw_circle(&mut self, cx: isize, cy: isize, radius: isize, color: Color) {
        let mut x = radius;
        let mut y = 0;
        let mut err = 0;

        while x >= y {
            let points = [
                (cx + x, cy + y), (cx + y, cy + x),
                (cx - y, cy + x), (cx - x, cy + y),
                (cx - x, cy - y), (cx - y, cy - x),
                (cx + y, cy - x), (cx + x, cy - y),
            ];
            for (px, py) in points {
                if px >= 0 && py >= 0 && (px as usize) < self.width && (py as usize) < self.height {
                    self.set_pixel(px as usize, py as usize, color);
                }
            }

            if err <= 0 {
                y += 1;
                err += 2 * y + 1;
            }
            if err > 0 {
                x -= 1;
                err -= 2 * x + 1;
            }
        }
    }

    /// Draws a solid filled circle.
    pub fn fill_circle(&mut self, cx: isize, cy: isize, radius: isize, color: Color) {
        let r2 = radius * radius;
        for dy in -radius..=radius {
            let py = cy + dy;
            if py < 0 || (py as usize) >= self.height {
                continue;
            }
            for dx in -radius..=radius {
                let px = cx + dx;
                if px < 0 || (px as usize) >= self.width {
                    continue;
                }
                if dx * dx + dy * dy <= r2 {
                    self.set_pixel(px as usize, py as usize, color);
                }
            }
        }
    }

    /// Draws a single glyph at (x, y) with alpha antialiasing.
    pub fn draw_char(&mut self, x: usize, y: usize, c: char, fg: Color, bg: Option<Color>) {
        if let Some(bg_color) = bg {
            self.fill_rect(x, y, CHAR_WIDTH, CHAR_HEIGHT, bg_color);
        }

        let raster = get_raster(c, FontWeight::Regular, FONT_HEIGHT)
            .unwrap_or_else(|| get_raster('?', FontWeight::Regular, FONT_HEIGHT).unwrap());

        for (row_idx, row) in raster.raster().iter().enumerate() {
            let py = y + row_idx;
            if py >= self.height {
                break;
            }

            for (col_idx, &intensity) in row.iter().enumerate() {
                let px = x + col_idx;
                if px >= self.width {
                    break;
                }

                if intensity > 0 {
                    let mut glyph_color = fg;
                    glyph_color.a = intensity;
                    self.set_pixel(px, py, glyph_color);
                }
            }
        }
    }

    /// Draws a string of text at (x, y).
    pub fn draw_string(&mut self, mut x: usize, y: usize, s: &str, fg: Color, bg: Option<Color>) {
        for c in s.chars() {
            if x + CHAR_WIDTH > self.width {
                break;
            }
            self.draw_char(x, y, c, fg, bg);
            x += CHAR_WIDTH;
        }
    }

    /// Blits a source canvas into this canvas at (dest_x, dest_y).
    pub fn blit(&mut self, dest_x: usize, dest_y: usize, src: &Canvas) {
        if dest_x >= self.width || dest_y >= self.height {
            return;
        }
        let copy_w = src.width.min(self.width - dest_x);
        let copy_h = src.height.min(self.height - dest_y);

        for row in 0..copy_h {
            let src_start = row * src.stride;
            let dest_start = (dest_y + row) * self.stride + dest_x;

            for col in 0..copy_w {
                if src_start + col < src.buffer.len() && dest_start + col < self.buffer.len() {
                    let pixel = Color::from_u32(src.buffer[src_start + col]);
                    if pixel.a == 255 {
                        self.buffer[dest_start + col] = pixel.to_u32();
                    } else if pixel.a > 0 {
                        let bg = Color::from_u32(self.buffer[dest_start + col]);
                        self.buffer[dest_start + col] = pixel.blend_over(bg).to_u32();
                    }
                }
            }
        }
    }

    /// Flushes this backbuffer to the physical hardware framebuffer memory.
    ///
    /// Uses high-speed memory block copies for tear-free rendering.
    pub fn flush_to(&self, hw_address: u64, hw_stride: usize, hw_height: usize) {
        if hw_address == 0 {
            return;
        }
        let hw_ptr = hw_address as *mut u32;
        let copy_height = self.height.min(hw_height);
        let copy_stride = self.stride.min(hw_stride);

        if self.stride == hw_stride {
            // Screen resolution and stride align perfectly: perform single continuous block copy
            let total_pixels = copy_stride * copy_height;
            unsafe {
                core::ptr::copy_nonoverlapping(self.buffer.as_ptr(), hw_ptr, total_pixels);
            }
        } else {
            // Copy row-by-row
            for row in 0..copy_height {
                let src_offset = row * self.stride;
                let dst_offset = row * hw_stride;
                unsafe {
                    core::ptr::copy_nonoverlapping(
                        self.buffer.as_ptr().add(src_offset),
                        hw_ptr.add(dst_offset),
                        copy_stride,
                    );
                }
            }
        }
    }
}

