//! # USB Human Interface Device (HID) Driver
//!
//! Implements USB HID Boot Protocol decoders and input dispatchers:
//! - **8-byte Keyboard Boot Report**: Modifier keys (Left/Right Ctrl, Shift, Alt, GUI) + 6-key rollover array.
//! - **3/4-byte Mouse Boot Report**: Button bitmask (Left, Right, Middle) + relative signed X/Y deltas.
//! - **Event Routing**: Key events are queued to [`USB_CHAR_QUEUE`] for console/shell consumption
//!   and [`INPUT_QUEUE`] for GUI desktop applications. Mouse events update [`MOUSE`] coordinates
//!   and generate button down/up events.
//!
//! ### USB Keyboard Boot Protocol Format (8 Bytes)
//!
//! ```text
//! Byte 0: Modifier keys bitmask:
//!         Bit 0: Left Ctrl,   Bit 1: Left Shift,   Bit 2: Left Alt,   Bit 3: Left GUI
//!         Bit 4: Right Ctrl,  Bit 5: Right Shift,  Bit 6: Right Alt,  Bit 7: Right GUI
//! Byte 1: Reserved (OEM / padding)
//! Byte 2..7: Key Array (Up to 6 simultaneous USB HID Usage IDs)
//! ```
//!
//! ### USB Mouse Boot Protocol Format (3 Bytes)
//!
//! ```text
//! Byte 0: Button bitmask: Bit 0 = Left, Bit 1 = Right, Bit 2 = Middle
//! Byte 1: Relative X displacement (signed 8-bit integer, two's complement)
//! Byte 2: Relative Y displacement (signed 8-bit integer, two's complement)
//! ```

#![allow(dead_code)]

use alloc::collections::VecDeque;
use crate::graphics::event::{InputEvent, MouseButton, INPUT_QUEUE};
use crate::graphics::mouse::MOUSE;
use crate::sync::Spinlock;

/// FIFO buffer storing decoded ASCII characters from USB keyboard inputs for interactive shell reading.
pub static USB_CHAR_QUEUE: Spinlock<VecDeque<char>> = Spinlock::new(VecDeque::new());

/// State tracker for USB Keyboard Boot Protocol reports.
///
/// Keeps track of previously pressed keys and modifier states to synthesize
/// edge-triggered key-down and key-up events.
#[derive(Clone, Copy, Debug, Default)]
pub struct UsbKeyboardState {
    /// Modifier bitmask from the previous report.
    pub prev_modifiers: u8,
    /// 6-key array from the previous report.
    pub prev_keys: [u8; 6],
}

impl UsbKeyboardState {
    /// Creates a new keyboard state tracker with no keys initially pressed.
    pub const fn new() -> Self {
        Self {
            prev_modifiers: 0,
            prev_keys: [0; 6],
        }
    }

    /// Processes an 8-byte standard USB Boot Protocol Keyboard Report.
    ///
    /// Computes set differences between current and previous keys to fire:
    /// - `pressed: true` events for newly detected key usages.
    /// - `pressed: false` events for keys that were released.
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
    /// Button bitmask from the previous report.
    pub prev_buttons: u8,
}

impl UsbMouseState {
    /// Creates a new mouse state tracker with all buttons unpressed.
    pub const fn new() -> Self {
        Self { prev_buttons: 0 }
    }

    /// Processes a standard 3-byte or 4-byte USB Boot Protocol Mouse Report.
    ///
    /// Updates cursor position on screen and detects button press and release transitions.
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

/// Global USB keyboard state tracker.
pub static USB_KEYBOARD: Spinlock<UsbKeyboardState> = Spinlock::new(UsbKeyboardState::new());
/// Global USB mouse state tracker.
pub static USB_MOUSE: Spinlock<UsbMouseState> = Spinlock::new(UsbMouseState::new());

/// Reads a pending character from the USB keyboard input queue, if one is available.
pub fn read_char() -> Option<char> {
    USB_CHAR_QUEUE.lock().pop_front()
}

/// Translates a standard USB HID Keyboard Usage ID to its corresponding ASCII character.
///
/// Handles letters, numerical digits, control keys (Enter, Esc, Backspace, Tab, Space),
/// and punctuation symbols, accounting for the `shift` modifier state.
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
