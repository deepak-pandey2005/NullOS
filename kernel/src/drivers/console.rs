//! # High-Performance Framebuffer Text Console Driver with ANSI Color Support
//!
//! ## Overview
//! This module provides a fast graphical text console rendered directly into the UEFI GOP
//! linear framebuffer memory aperture.
//!
//! ### Subsystem Capabilities:
//! 1. **Antialiased Vector-Quality Font Rasterization**:
//!    - Uses the `noto-sans-mono-bitmap` crate to render clean 16x9 monospaced glyphs.
//!    - Performs linear alpha-channel color blending (`blend_color`) between foreground and background pixels.
//! 2. **ANSI Color Escape Sequences**:
//!    - Built-in finite state machine (`AnsiState`) parsing standard ECMA-48 / ANSI SGR color sequences
//!      (e.g. `\x1b[31m` red, `\x1b[32m` green, `\x1b[34m` blue, `\x1b[0m` reset).
//! 3. **High-Performance Hardware Scrolling**:
//!    - Implements rapid screen scrolling using block memory moves (`core::ptr::copy`), shifting
//!      all display scanlines up by one character row (`CHAR_HEIGHT = 16` pixels) in a single burst.
//! 4. **Circular `dmesg` Ring Buffer**:
//!    - Maintains an un-allocated 64 KiB circular byte buffer (`DMESG_RING`) capturing all kernel log output.
//!    - Safe across early boot, interrupts, and memory allocator locks.
//! 5. **Compositor Suspension Protocol**:
//!    - Provides [`set_suspended`] to pause direct text rendering when the desktop Window Manager is active,
//!      preventing log text from stomping over GUI window canvases.
//!
//! // TODO(phase12-m30-gui): Implement double-buffered 2D graphics canvas and window compositor:
//! // 1. Allocate an off-screen secondary backbuffer to prevent tearing and flicker during fast animations.
//! // 2. Add 2D drawing primitives: line (Bresenham algorithm), rect, filled rect, and bitmap sprite blitting.
//! // 3. Implement overlapping Window abstraction with title bars, draggable headers, and dirty region redraws.
//! // 4. Hardware Mouse Cursor: Render transparent mouse arrow sprite updated on PS/2 / USB mouse input events.

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};
use crate::sync::Spinlock;
use bootinfo::{FramebufferInfo, PixelFormat};
use core::fmt;
use noto_sans_mono_bitmap::{get_raster, FontWeight, RasterHeight};

/// Raster font height (16 pixels).
const FONT_HEIGHT: RasterHeight = RasterHeight::Size16;
/// Pixel height of each rendered character cell.
const CHAR_HEIGHT: usize = 16;
/// Pixel width of each rendered character cell.
const CHAR_WIDTH: usize = 9;

/// Default foreground color: clean light slate text (0x00E2E8F0).
const DEFAULT_FG: u32 = 0x00E2E8F0;
/// Default background color: dark slate blue (0x000F172A).
const DEFAULT_BG: u32 = 0x000F172A;

#[derive(Clone, Copy, PartialEq, Eq)]
enum AnsiState {
    Normal,
    Esc,
    Bracket,
    Param(u32),
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct Cell {
    ch: char,
    fg: u32,
    bg: u32,
}

impl Cell {
    const fn blank() -> Self {
        Self {
            ch: ' ',
            fg: DEFAULT_FG,
            bg: DEFAULT_BG,
        }
    }
}

struct CellBuffer {
    cols: usize,
    logical: Vec<Cell>,
    displayed: Vec<Cell>,
}

impl CellBuffer {
    fn new(cols: usize, rows: usize) -> Self {
        let total = cols * rows;
        Self {
            cols,
            logical: alloc::vec![Cell::blank(); total],
            displayed: alloc::vec![Cell::blank(); total],
        }
    }
}

static CELL_BUFFER: Spinlock<Option<CellBuffer>> = Spinlock::new(None);

/// Linear framebuffer text console renderer with RAM text-cell ring buffer & dirty-cell redraw.
pub struct FramebufferConsole {
    address: u64,
    width: usize,
    height: usize,
    stride: usize,
    _bytes_per_pixel: usize,
    _pixel_format: PixelFormat,
    cols: usize,
    rows: usize,
    cursor_col: usize,
    cursor_row: usize,
    top_row: usize,
    fg_color: u32,
    bg_color: u32,
    ansi_state: AnsiState,
}

static CONSOLE: Spinlock<Option<FramebufferConsole>> = Spinlock::new(None);
static CONSOLE_SUSPENDED: AtomicBool = AtomicBool::new(false);

const DMESG_CAPACITY: usize = 65536; // 64 KiB circular byte buffer

struct DmesgRing {
    buf: [u8; DMESG_CAPACITY],
    head: usize,
    count: usize,
}

impl DmesgRing {
    const fn new() -> Self {
        Self {
            buf: [0; DMESG_CAPACITY],
            head: 0,
            count: 0,
        }
    }

    #[inline]
    fn push_byte(&mut self, b: u8) {
        let idx = (self.head + self.count) % DMESG_CAPACITY;
        self.buf[idx] = b;
        if self.count < DMESG_CAPACITY {
            self.count += 1;
        } else {
            self.head = (self.head + 1) % DMESG_CAPACITY;
        }
    }

    fn push_str(&mut self, s: &str) {
        for &b in s.as_bytes() {
            self.push_byte(b);
        }
    }
}

static DMESG_RING: Spinlock<DmesgRing> = Spinlock::new(DmesgRing::new());

impl FramebufferConsole {
    pub fn new(fb: &FramebufferInfo) -> Self {
        let cols = (fb.width / CHAR_WIDTH).max(1);
        let rows = (fb.height / CHAR_HEIGHT).max(1);
        Self {
            address: fb.address,
            width: fb.width,
            height: fb.height,
            stride: fb.stride,
            _bytes_per_pixel: fb.bytes_per_pixel,
            _pixel_format: fb.pixel_format,
            cols,
            rows,
            cursor_col: 0,
            cursor_row: 0,
            top_row: 0,
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

        let blank = Cell {
            ch: ' ',
            fg: self.fg_color,
            bg: self.bg_color,
        };
        if let Some(ref mut buf) = *CELL_BUFFER.lock() {
            buf.logical.fill(blank);
            buf.displayed.fill(blank);
        }

        self.cursor_col = 0;
        self.cursor_row = 0;
        self.top_row = 0;
    }

    #[allow(dead_code)]
    pub fn write_char(&mut self, c: char) {
        self.write_char_buffered(c);
        self.flush();
    }

    fn write_char_buffered(&mut self, c: char) {
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

        // Standard character rendering into RAM cell ring buffer (or direct before heap init)
        match c {
            '\n' => self.newline(),
            '\r' => self.cursor_col = 0,
            '\x08' => {
                if self.cursor_col > 0 {
                    self.cursor_col -= 1;
                } else if self.cursor_row > 0 {
                    self.cursor_row -= 1;
                    self.cursor_col = self.cols - 1;
                }
            }
            '\t' => {
                for _ in 0..4 {
                    self.write_char_buffered(' ');
                }
            }
            ' ' => {
                if self.cursor_col >= self.cols {
                    self.newline();
                }
                self.emit_cell(Cell {
                    ch: ' ',
                    fg: self.fg_color,
                    bg: self.bg_color,
                });
                self.cursor_col += 1;
            }
            c if !c.is_control() => {
                if self.cursor_col >= self.cols {
                    self.newline();
                }
                self.emit_cell(Cell {
                    ch: c,
                    fg: self.fg_color,
                    bg: self.bg_color,
                });
                self.cursor_col += 1;
            }
            _ => {}
        }
    }

    #[inline]
    fn emit_cell(&self, cell: Cell) {
        let mut guard = CELL_BUFFER.lock();
        if let Some(ref mut buf) = *guard {
            let log_row = (self.top_row + self.cursor_row) % self.rows;
            buf.logical[log_row * buf.cols + self.cursor_col] = cell;
        } else {
            let x_pos = self.cursor_col * CHAR_WIDTH;
            let y_pos = self.cursor_row * CHAR_HEIGHT;
            if cell.ch == ' ' {
                self.clear_char_at(x_pos, y_pos, cell.bg);
            } else {
                self.draw_char_at(x_pos, y_pos, cell.ch, cell.fg, cell.bg);
            }
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
        self.cursor_col = 0;
        if self.cursor_row + 1 < self.rows {
            self.cursor_row += 1;
        } else {
            self.scroll_up();
        }
    }

    /// O(1) ring-buffer scroll in RAM — never reads back from hardware VRAM once backbuffer is active.
    fn scroll_up(&mut self) {
        let mut guard = CELL_BUFFER.lock();
        if let Some(ref mut buf) = *guard {
            self.top_row = (self.top_row + 1) % self.rows;
            let bottom_log_row = (self.top_row + self.rows - 1) % self.rows;
            let row_base = bottom_log_row * buf.cols;
            let blank = Cell {
                ch: ' ',
                fg: self.fg_color,
                bg: self.bg_color,
            };
            buf.logical[row_base..row_base + buf.cols].fill(blank);
            self.cursor_row = self.rows - 1;
        } else {
            // Early-boot fallback before heap is initialized
            let ptr = self.address as *mut u32;
            let lines_to_move = self.height - CHAR_HEIGHT;
            let total_pixels_to_move = lines_to_move * self.stride;
            unsafe {
                core::ptr::copy(ptr.add(CHAR_HEIGHT * self.stride), ptr, total_pixels_to_move);
                let clear_ptr = ptr.add(total_pixels_to_move);
                let clear_count = CHAR_HEIGHT * self.stride;
                for i in 0..clear_count {
                    *clear_ptr.add(i) = self.bg_color;
                }
            }
            self.cursor_row = self.rows - 1;
        }
    }

    /// Flushes only dirty character cells from the RAM ring buffer to the GOP framebuffer (write-only).
    pub fn flush(&self) {
        let mut guard = CELL_BUFFER.lock();
        let buf = match *guard {
            Some(ref mut b) => b,
            None => return,
        };

        for screen_row in 0..self.rows {
            let log_row = (self.top_row + screen_row) % self.rows;
            let log_base = log_row * buf.cols;
            let disp_base = screen_row * buf.cols;
            let y_pos = screen_row * CHAR_HEIGHT;

            for col in 0..self.cols {
                let cell = buf.logical[log_base + col];
                if cell != buf.displayed[disp_base + col] {
                    buf.displayed[disp_base + col] = cell;
                    let x_pos = col * CHAR_WIDTH;
                    if cell.ch == ' ' {
                        self.clear_char_at(x_pos, y_pos, cell.bg);
                    } else {
                        self.draw_char_at(x_pos, y_pos, cell.ch, cell.fg, cell.bg);
                    }
                }
            }
        }
    }

    fn clear_char_at(&self, x_pos: usize, y_pos: usize, bg: u32) {
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
                    *ptr.add(row_offset + x_pos + col_idx) = bg;
                }
            }
        }
    }

    fn draw_char_at(&self, x_pos: usize, y_pos: usize, c: char, fg: u32, bg: u32) {
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
                    fg
                } else if intensity > 30 {
                    self.blend_color(fg, bg, intensity)
                } else {
                    bg
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
            self.write_char_buffered(c);
        }
        self.flush();
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

/// Activates the heap-backed RAM text-cell ring buffer once the kernel heap is ready,
/// syncing existing screen content from `DMESG_RING` without touching hardware VRAM.
pub fn init_backbuffer() {
    let (cols, rows) = match *CONSOLE.lock() {
        Some(ref c) => (c.cols, c.rows),
        None => return,
    };

    let buf = CellBuffer::new(cols, rows);
    let raw_bytes: Vec<u8> = {
        let ring = DMESG_RING.lock();
        let mut v = Vec::with_capacity(ring.count);
        for i in 0..ring.count {
            v.push(ring.buf[(ring.head + i) % DMESG_CAPACITY]);
        }
        v
    };

    if let Some(ref mut console) = *CONSOLE.lock() {
        console.cursor_col = 0;
        console.cursor_row = 0;
        console.top_row = 0;
        console.fg_color = DEFAULT_FG;
        console.bg_color = DEFAULT_BG;
        console.ansi_state = AnsiState::Normal;

        *CELL_BUFFER.lock() = Some(buf);

        if let Ok(s) = core::str::from_utf8(&raw_bytes) {
            for ch in s.chars() {
                console.write_char_buffered(ch);
            }
        }

        if let Some(ref mut cb) = *CELL_BUFFER.lock() {
            for screen_row in 0..console.rows {
                let log_row = (console.top_row + screen_row) % console.rows;
                let log_base = log_row * cb.cols;
                let disp_base = screen_row * cb.cols;
                cb.displayed[disp_base..disp_base + cb.cols]
                    .copy_from_slice(&cb.logical[log_base..log_base + cb.cols]);
            }
        }
    }
}

/// Suspends or resumes direct rendering to the GOP framebuffer console.
/// When suspended (e.g. while Desktop Compositor is active), logs are recorded in dmesg
/// but not written directly to video RAM.
pub fn set_suspended(suspended: bool) {
    CONSOLE_SUSPENDED.store(suspended, Ordering::SeqCst);
}

/// Checks if console direct video output is currently suspended.
pub fn is_suspended() -> bool {
    CONSOLE_SUSPENDED.load(Ordering::SeqCst)
}

/// Clears the physical text console display.
pub fn clear_screen() {
    if let Some(ref mut console) = *CONSOLE.lock() {
        console.clear();
    }
}

/// Helper to remove ANSI escape sequences like \x1b[32m for readable dmesg logs.
fn strip_ansi_codes(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut in_escape = false;
    for c in input.chars() {
        if c == '\x1b' {
            in_escape = true;
        } else if in_escape {
            if c.is_ascii_alphabetic() {
                in_escape = false;
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Stores a raw string into the zero-allocation in-memory dmesg ring buffer.
/// Completely safe across interrupts, early boot, and re-entrant allocator calls.
#[inline]
pub fn record_dmesg(s: &str) {
    DMESG_RING.lock().push_str(s);
}

/// Returns all recorded kernel messages joined as a formatted string with ANSI codes removed.
pub fn get_dmesg() -> String {
    let (_head, count) = {
        let ring = DMESG_RING.lock();
        (ring.head, ring.count)
    };
    // Allocate buffer outside the DMESG_RING spinlock to prevent lock inversion/deadlocks with heap expansion
    let mut out = String::with_capacity(count);
    {
        let ring = DMESG_RING.lock();
        let valid_count = ring.count.min(count);
        for i in 0..valid_count {
            let b = ring.buf[(ring.head + i) % DMESG_CAPACITY];
            out.push(b as char);
        }
    }
    strip_ansi_codes(&out)
}

/// Returns the last `max` lines from the dmesg buffer without allocating full 64 KiB ring buffers.
pub fn get_dmesg_lines(max: usize) -> Vec<String> {
    let mut raw_bytes = alloc::vec::Vec::new();
    {
        let ring = DMESG_RING.lock();
        if ring.count == 0 {
            return Vec::new();
        }

        // Scan backwards from tail to find `max` newline characters
        let mut newlines_found = 0;
        let mut start_offset = 0;

        for offset in (0..ring.count).rev() {
            let b = ring.buf[(ring.head + offset) % DMESG_CAPACITY];
            if b == b'\n' {
                newlines_found += 1;
                if newlines_found > max {
                    start_offset = offset + 1;
                    break;
                }
            }
        }

        let slice_len = ring.count - start_offset;
        raw_bytes.reserve(slice_len);
        for i in 0..slice_len {
            raw_bytes.push(ring.buf[(ring.head + start_offset + i) % DMESG_CAPACITY]);
        }
    } // DMESG_RING lock released before String allocations!

    let s = core::str::from_utf8(&raw_bytes).unwrap_or("");
    let clean = strip_ansi_codes(s);
    clean.split('\n')
        .filter(|l| !l.is_empty())
        .map(|l| l.to_string())
        .collect()
}

pub fn write_str(s: &str) {
    record_dmesg(s);
    if !is_suspended() {
        if let Some(ref mut console) = *CONSOLE.lock() {
            let _ = <FramebufferConsole as fmt::Write>::write_str(console, s);
        }
    }
}
