//! Graphical User Interface Input Event System.
//!
//! Captures and queues keyboard strokes, mouse cursor displacements,
//! and button click events for window dispatching.

use alloc::collections::VecDeque;
use crate::sync::Spinlock;

/// Mouse button identifiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
}

pub const GUI_EVENT_MOUSE_DOWN: u32 = 1;
pub const GUI_EVENT_MOUSE_UP: u32 = 2;
pub const GUI_EVENT_MOUSE_MOVE: u32 = 3;
pub const GUI_EVENT_KEY_DOWN: u32 = 4;
pub const GUI_EVENT_CLOSE_REQUESTED: u32 = 5;

/// ABI-stable GUI event delivered to Ring 3 user applications via `SYS_GUI_POLL_EVENT`.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GuiUserEvent {
    pub kind: u32,
    pub param1: i32,
    pub param2: i32,
    pub param3: i32,
}

/// Unified user input event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputEvent {
    /// Keyboard key press or release.
    Key {
        scancode: u8,
        character: Option<char>,
        pressed: bool,
    },
    /// Mouse pointer movement.
    MouseMove {
        x: isize,
        y: isize,
        dx: isize,
        dy: isize,
    },
    /// Mouse button pressed down.
    MouseDown {
        button: MouseButton,
        x: isize,
        y: isize,
    },
    /// Mouse button released.
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

    /// Pushes a new input event to the back of the queue.
    pub fn push(&self, event: InputEvent) {
        let mut queue = self.events.lock();
        if queue.len() < 1024 {
            queue.push_back(event);
        }
    }

    /// Pops the oldest event from the front of the queue.
    pub fn pop(&self) -> Option<InputEvent> {
        self.events.lock().pop_front()
    }

    /// Drains all pending events, passing them to a handler closure under a single lock acquisition.
    pub fn drain_into<F: FnMut(InputEvent)>(&self, mut handler: F) {
        let mut queue = self.events.lock();
        for event in queue.drain(..) {
            handler(event);
        }
    }

    /// Clears all pending events in the queue.
    pub fn clear(&self) {
        self.events.lock().clear();
    }
}

pub static INPUT_QUEUE: EventQueue = EventQueue::new();

