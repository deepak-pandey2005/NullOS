//! Raw Input Event Queue (`evdev`-style kernel interface).

use alloc::collections::VecDeque;
use crate::sync::Spinlock;

/// Mouse button identifiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
}

pub const RAW_INPUT_MOUSE_MOVE: u32 = 1;
pub const RAW_INPUT_MOUSE_DOWN: u32 = 2;
pub const RAW_INPUT_MOUSE_UP: u32 = 3;
pub const RAW_INPUT_KEY: u32 = 4;

/// ABI-stable raw input event delivered to Ring 3 via `SYS_INPUT_POLL`.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RawInputEvent {
    pub kind: u32,
    pub x: i32,
    pub y: i32,
    pub dx: i32,
    pub dy: i32,
    pub button: u32,
    pub key: u32,
    pub pressed: u32,
}

/// Unified user input event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputEvent {
    Key {
        scancode: u8,
        character: Option<char>,
        pressed: bool,
    },
    MouseMove {
        x: isize,
        y: isize,
        dx: isize,
        dy: isize,
    },
    MouseDown {
        button: MouseButton,
        x: isize,
        y: isize,
    },
    MouseUp {
        button: MouseButton,
        x: isize,
        y: isize,
    },
}

/// Thread-safe input event queue.
pub struct EventQueue {
    events: Spinlock<VecDeque<InputEvent>>,
}

impl EventQueue {
    pub const fn new() -> Self {
        Self {
            events: Spinlock::new(VecDeque::new()),
        }
    }

    pub fn push(&self, event: InputEvent) {
        let mut queue = self.events.lock();
        if queue.len() < 1024 {
            queue.push_back(event);
        }
    }

    pub fn pop(&self) -> Option<InputEvent> {
        self.events.lock().pop_front()
    }

    pub fn drain_into<F: FnMut(InputEvent)>(&self, mut handler: F) {
        let mut queue = self.events.lock();
        for event in queue.drain(..) {
            handler(event);
        }
    }

    pub fn clear(&self) {
        self.events.lock().clear();
    }
}

pub static INPUT_QUEUE: EventQueue = EventQueue::new();

fn btn_code(b: MouseButton) -> u32 {
    match b {
        MouseButton::Left => 0,
        MouseButton::Right => 1,
        MouseButton::Middle => 2,
    }
}

/// Polls hardware input devices (USB HID, PS/2 keyboard, COM1 serial, and queued mouse events)
/// and returns the next raw input event for Ring 3.
pub fn poll_raw_input() -> Option<RawInputEvent> {
    crate::drivers::usb::poll_inputs();

    if let Some(ev) = INPUT_QUEUE.pop() {
        return Some(match ev {
            InputEvent::MouseMove { x, y, dx, dy } => RawInputEvent {
                kind: RAW_INPUT_MOUSE_MOVE,
                x: x as i32,
                y: y as i32,
                dx: dx as i32,
                dy: dy as i32,
                button: 0,
                key: 0,
                pressed: 0,
            },
            InputEvent::MouseDown { button, x, y } => RawInputEvent {
                kind: RAW_INPUT_MOUSE_DOWN,
                x: x as i32,
                y: y as i32,
                dx: 0,
                dy: 0,
                button: btn_code(button),
                key: 0,
                pressed: 1,
            },
            InputEvent::MouseUp { button, x, y } => RawInputEvent {
                kind: RAW_INPUT_MOUSE_UP,
                x: x as i32,
                y: y as i32,
                dx: 0,
                dy: 0,
                button: btn_code(button),
                key: 0,
                pressed: 0,
            },
            InputEvent::Key { scancode, character, pressed } => RawInputEvent {
                kind: RAW_INPUT_KEY,
                x: 0,
                y: 0,
                dx: 0,
                dy: 0,
                button: 0,
                key: character.map(|c| c as u32).unwrap_or(scancode as u32),
                pressed: if pressed { 1 } else { 0 },
            },
        });
    }

    let ch = if let Some(c) = crate::drivers::usb::read_char() {
        Some(c)
    } else if let Some(c) = crate::drivers::keyboard::read_char() {
        Some(c)
    } else if let Some(b) = crate::drivers::serial::read_byte() {
        match b {
            b'\r' | b'\n' => Some('\n'),
            0x08 | 0x7F => Some('\x08'),
            0x1B => Some('\x1B'),
            0x0E => Some('\x0E'),
            0x17 => Some('\x17'),
            c if c >= 0x20 && c <= 0x7E => Some(c as char),
            _ => None,
        }
    } else {
        None
    };

    ch.map(|c| RawInputEvent {
        kind: RAW_INPUT_KEY,
        x: 0,
        y: 0,
        dx: 0,
        dy: 0,
        button: 0,
        key: c as u32,
        pressed: 1,
    })
}
