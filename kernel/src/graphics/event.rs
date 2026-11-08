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

