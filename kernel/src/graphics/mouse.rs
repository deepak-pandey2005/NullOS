//! Mouse Cursor State & Rendering Engine.
//!
//! Features:
//! - Sub-pixel mouse position tracking with screen boundary clamping.
//! - Anti-aliased high-contrast cursor sprite with transparency.
//! - PS/2 3-byte auxiliary packet decoder.

use crate::graphics::canvas::Canvas;
use crate::graphics::color::Color;
use crate::graphics::event::{InputEvent, MouseButton, INPUT_QUEUE};
use crate::sync::Spinlock;

/// 12x18 Arrow cursor sprite bitmap definition.
/// '.' = Transparent
/// 'X' = Black Border (#000000)
/// 'W' = White Interior (#FFFFFF)
const CURSOR_WIDTH: usize = 12;
const CURSOR_HEIGHT: usize = 18;

const CURSOR_SPRITE: [&str; CURSOR_HEIGHT] = [
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
    ".....XWWX...",
    "......XX....",
    "............",
];

/// Global mouse device state.
pub struct MouseState {
    pub x: isize,
    pub y: isize,
    pub screen_width: usize,
    pub screen_height: usize,
    pub left_pressed: bool,
    pub right_pressed: bool,
    pub middle_pressed: bool,
    packet_idx: usize,
    packet_buf: [u8; 3],
}

impl MouseState {
    pub const fn new(init_x: isize, init_y: isize) -> Self {
        Self {
            x: init_x,
            y: init_y,
            screen_width: 1280,
            screen_height: 800,
            left_pressed: false,
            right_pressed: false,
            middle_pressed: false,
            packet_idx: 0,
            packet_buf: [0; 3],
        }
    }

    /// Sets the boundaries of the screen for cursor clamping.
    pub fn set_screen_size(&mut self, width: usize, height: usize) {
        self.screen_width = width;
        self.screen_height = height;
    }

    /// Moves the mouse by relative deltas and clamps within (0..width, 0..height).
    pub fn move_relative(&mut self, dx: isize, dy: isize) {
        let max_x = (self.screen_width.saturating_sub(1)) as isize;
        let max_y = (self.screen_height.saturating_sub(1)) as isize;

        self.x = (self.x + dx).clamp(0, max_x);
        self.y = (self.y + dy).clamp(0, max_y);

        INPUT_QUEUE.push(InputEvent::MouseMove {
            x: self.x,
            y: self.y,
            dx,
            dy,
        });
    }

    /// Feeds a raw byte from the PS/2 mouse interface (Port 0x60).
    pub fn process_byte(&mut self, byte: u8) {
        // Byte 0 must have bit 3 set to 1 in standard PS/2 packet framing
        if self.packet_idx == 0 && (byte & 0x08) == 0 {
            return;
        }

        self.packet_buf[self.packet_idx] = byte;
        self.packet_idx += 1;

        if self.packet_idx == 3 {
            self.packet_idx = 0;
            self.decode_packet();
        }
    }

    fn decode_packet(&mut self) {
        let flags = self.packet_buf[0];
        let raw_dx = self.packet_buf[1] as isize;
        let raw_dy = self.packet_buf[2] as isize;

        // Sign extension for 9-bit relative coordinates
        let dx = if (flags & 0x10) != 0 {
            raw_dx - 256
        } else {
            raw_dx
        };

        // Note: PS/2 Y axis is inverted (positive is up)
        let dy = if (flags & 0x20) != 0 {
            -(raw_dy - 256)
        } else {
            -raw_dy
        };

        // Update position
        self.move_relative(dx, dy);

        // Update button states
        let left = (flags & 0x01) != 0;
        let right = (flags & 0x02) != 0;
        let middle = (flags & 0x04) != 0;

        if left != self.left_pressed {
            self.left_pressed = left;
            if left {
                INPUT_QUEUE.push(InputEvent::MouseDown {
                    button: MouseButton::Left,
                    x: self.x,
                    y: self.y,
                });
            } else {
                INPUT_QUEUE.push(InputEvent::MouseUp {
                    button: MouseButton::Left,
                    x: self.x,
                    y: self.y,
                });
            }
        }

        if right != self.right_pressed {
            self.right_pressed = right;
            if right {
                INPUT_QUEUE.push(InputEvent::MouseDown {
                    button: MouseButton::Right,
                    x: self.x,
                    y: self.y,
                });
            } else {
                INPUT_QUEUE.push(InputEvent::MouseUp {
                    button: MouseButton::Right,
                    x: self.x,
                    y: self.y,
                });
            }
        }

        if middle != self.middle_pressed {
            self.middle_pressed = middle;
            if middle {
                INPUT_QUEUE.push(InputEvent::MouseDown {
                    button: MouseButton::Middle,
                    x: self.x,
                    y: self.y,
                });
            } else {
                INPUT_QUEUE.push(InputEvent::MouseUp {
                    button: MouseButton::Middle,
                    x: self.x,
                    y: self.y,
                });
            }
        }
    }

    /// Renders the mouse cursor arrow onto the target canvas at current (x, y).
    pub fn render(&self, canvas: &mut Canvas) {
        let origin_x = self.x;
        let origin_y = self.y;

        for (row_idx, row_str) in CURSOR_SPRITE.iter().enumerate() {
            let py = origin_y + row_idx as isize;
            if py < 0 || py >= canvas.height as isize {
                continue;
            }

            for (col_idx, ch) in row_str.chars().enumerate() {
                let px = origin_x + col_idx as isize;
                if px < 0 || px >= canvas.width as isize {
                    continue;
                }

                match ch {
                    'X' => canvas.set_pixel(px as usize, py as usize, Color::BLACK),
                    'W' => canvas.set_pixel(px as usize, py as usize, Color::WHITE),
                    _ => {}
                }
            }
        }
    }
}

pub static MOUSE: Spinlock<MouseState> = Spinlock::new(MouseState::new(200, 200));

