//! USB HID (Human Interface Device) Driver (Phase 12 - M31).
//!
//! Decodes USB Boot Protocol input reports:
//! - 8-byte Keyboard Boot Report (Modifiers + 6 Key Usage IDs).
//! - 3/4-byte Mouse Boot Report (Buttons + Relative X/Y deltas).
//! - Dispatches keyboard events to console/shell and graphics event system.
//! - Dispatches mouse events to window manager and mouse cursor engine.

#![allow(dead_code)]

use alloc::collections::VecDeque;
use crate::graphics::event::{InputEvent, MouseButton, INPUT_QUEUE};
use crate::graphics::mouse::MOUSE;
use crate::sync::Spinlock;

pub static USB_CHAR_QUEUE: Spinlock<VecDeque<char>> = Spinlock::new(VecDeque::new());

/// State tracker for USB Keyboard Boot Protocol reports.
#[derive(Clone, Copy, Debug, Default)]
pub struct UsbKeyboardState {
    pub prev_modifiers: u8,
    pub prev_keys: [u8; 6],
}

impl UsbKeyboardState {
    pub const fn new() -> Self {
        Self {
            prev_modifiers: 0,
            prev_keys: [0; 6],
        }
    }

    /// Processes an 8-byte standard USB Boot Protocol Keyboard Report.
    pub fn process_report(&mut self, report: &[u8; 8]) {
        let modifiers = report[0];
        let keys = [
            report[2], report[3], report[4], report[5], report[6], report[7],
        ];

        let shift = (modifiers & 0x22) != 0; // Left Shift (0x02) or Right Shift (0x20)

        // Detect newly pressed keys (present in current keys, but absent in prev_keys)
        for &key in &keys {
            if key != 0 && !self.prev_keys.contains(&key) {
                if let Some(ch) = hid_usage_to_char(key, shift) {
                    // Buffer character for interactive shell
                    USB_CHAR_QUEUE.lock().push_back(ch);

                    // Push to Desktop Window Manager input queue
                    INPUT_QUEUE.push(InputEvent::Key {
                        scancode: key,
                        character: Some(ch),
                        pressed: true,
                    });
                }
            }
        }

        // Detect released keys
        for &old_key in &self.prev_keys {
            if old_key != 0 && !keys.contains(&old_key) {
                let ch = hid_usage_to_char(old_key, shift);
                INPUT_QUEUE.push(InputEvent::Key {
                    scancode: old_key,
                    character: ch,
                    pressed: false,
                });
            }
        }

        self.prev_modifiers = modifiers;
        self.prev_keys = keys;
    }
}

/// State tracker for USB Mouse Boot Protocol reports.
#[derive(Clone, Copy, Debug, Default)]
pub struct UsbMouseState {
    pub prev_buttons: u8,
}

impl UsbMouseState {
    pub const fn new() -> Self {
        Self { prev_buttons: 0 }
    }

    /// Processes a standard 3-byte or 4-byte USB Boot Protocol Mouse Report.
    pub fn process_report(&mut self, report: &[u8]) {
        if report.len() < 3 {
            return;
        }

        let buttons = report[0];
        let dx = (report[1] as i8) as isize;
        let dy = (report[2] as i8) as isize;

        // Move cursor via global MOUSE state
        {
            let mut mouse = MOUSE.lock();
            mouse.move_relative(dx, dy);

            let x = mouse.x;
            let y = mouse.y;

            // Check button state transitions
            let check_btn = |mask: u8, btn: MouseButton, prev: u8, curr: u8| {
                let was_down = (prev & mask) != 0;
                let is_down = (curr & mask) != 0;
                if !was_down && is_down {
                    INPUT_QUEUE.push(InputEvent::MouseDown { button: btn, x, y });
                } else if was_down && !is_down {
                    INPUT_QUEUE.push(InputEvent::MouseUp { button: btn, x, y });
                }
            };

            check_btn(0x01, MouseButton::Left, self.prev_buttons, buttons);
            check_btn(0x02, MouseButton::Right, self.prev_buttons, buttons);
            check_btn(0x04, MouseButton::Middle, self.prev_buttons, buttons);

            mouse.left_pressed = (buttons & 0x01) != 0;
            mouse.right_pressed = (buttons & 0x02) != 0;
            mouse.middle_pressed = (buttons & 0x04) != 0;
        }

        self.prev_buttons = buttons;
    }
}

/// Global USB HID input state trackers.
pub static USB_KEYBOARD: Spinlock<UsbKeyboardState> = Spinlock::new(UsbKeyboardState::new());
pub static USB_MOUSE: Spinlock<UsbMouseState> = Spinlock::new(UsbMouseState::new());

/// Reads a pending character from the USB keyboard input queue.
pub fn read_char() -> Option<char> {
    USB_CHAR_QUEUE.lock().pop_front()
}

/// Translates a USB HID Usage ID to an ASCII character.
pub fn hid_usage_to_char(usage: u8, shift: bool) -> Option<char> {
    match usage {
        // Letters A-Z
        0x04..=0x1D => {
            let base = if shift { b'A' } else { b'a' };
            Some((base + (usage - 0x04)) as char)
        }
        // Numbers 1-9, 0
        0x1E => Some(if shift { '!' } else { '1' }),
        0x1F => Some(if shift { '@' } else { '2' }),
        0x20 => Some(if shift { '#' } else { '3' }),
        0x21 => Some(if shift { '$' } else { '4' }),
        0x22 => Some(if shift { '%' } else { '5' }),
        0x23 => Some(if shift { '^' } else { '6' }),
        0x24 => Some(if shift { '&' } else { '7' }),
        0x25 => Some(if shift { '*' } else { '8' }),
        0x26 => Some(if shift { '(' } else { '9' }),
        0x27 => Some(if shift { ')' } else { '0' }),

        // Control characters
        0x28 => Some('\n'),   // Enter / Return
        0x29 => Some('\x1B'), // Escape
        0x2A => Some('\x08'), // Backspace
        0x2B => Some('\t'),   // Tab
        0x2C => Some(' '),    // Space

        // Symbols
        0x2D => Some(if shift { '_' } else { '-' }),
        0x2E => Some(if shift { '+' } else { '=' }),
        0x2F => Some(if shift { '{' } else { '[' }),
        0x30 => Some(if shift { '}' } else { ']' }),
        0x31 => Some(if shift { '|' } else { '\\' }),
        0x33 => Some(if shift { ':' } else { ';' }),
        0x34 => Some(if shift { '"' } else { '\'' }),
        0x35 => Some(if shift { '~' } else { '`' }),
        0x36 => Some(if shift { '<' } else { ',' }),
        0x37 => Some(if shift { '>' } else { '.' }),
        0x38 => Some(if shift { '?' } else { '/' }),

        _ => None,
    }
}
