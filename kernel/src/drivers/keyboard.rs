/// Intel 8042 PS/2 Keyboard Controller Driver for x86_64.
///
/// Implements standard Scan Code Set 1 translation, modifier state tracking
/// (Shift, CapsLock, Ctrl, Alt), non-blocking input polling, and integration
/// with the interactive shell and graphical window.
///
/// // TODO(phase12-m31-mouse): Implement PS/2 Auxiliary Mouse Driver:
/// // 1. Enable 2nd PS/2 port (auxiliary mouse device) via 8042 command 0xA8.
/// // 2. Enable IRQ12 interrupt delivery via Local APIC / IOAPIC (IDT Vector 0x2C).
/// // 3. Decode 3-byte mouse packets: byte 0 flags (buttons, sign bits), byte 1 (X delta), byte 2 (Y delta).
/// // 4. Dispatch mouse position updates to the graphical window canvas and cursor overlay.

use crate::println;
use crate::sync::Spinlock;

/// I/O Port Addresses for 8042 PS/2 Controller
const DATA_PORT: u16 = 0x60;
const STATUS_PORT: u16 = 0x64;
const COMMAND_PORT: u16 = 0x64;

/// Status Register Flags (Port 0x64)
const STATUS_OUTPUT_BUFFER_FULL: u8 = 0x01; // Bit 0: 1 = data ready to read from port 0x60
const STATUS_INPUT_BUFFER_FULL: u8 = 0x02;  // Bit 1: 1 = controller busy, do not write yet

/// 8042 Controller Commands (Port 0x64)
const CMD_ENABLE_FIRST_PORT: u8 = 0xAE;     // Enable PS/2 keyboard interface

/// Read a single byte from an x86_64 I/O port.
#[inline]
unsafe fn inb(port: u16) -> u8 {
    let value: u8;
    unsafe {
        core::arch::asm!(
            "in al, dx",
            in("dx") port,
            out("al") value,
            options(nomem, nostack),
        );
    }
    value
}

/// Write a single byte to an x86_64 I/O port.
#[inline]
unsafe fn outb(port: u16, val: u8) {
    unsafe {
        core::arch::asm!(
            "out dx, al",
            in("dx") port,
            in("al") val,
            options(nomem, nostack),
        );
    }
}

/// Internal modifier tracking state for Scan Code Set 1 decoding.
#[derive(Debug, Clone, Copy)]
pub struct KeyboardState {
    pub shift: bool,
    pub caps_lock: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub extended: bool,
}

impl KeyboardState {
    pub const fn new() -> Self {
        Self {
            shift: false,
            caps_lock: false,
            ctrl: false,
            alt: false,
            extended: false,
        }
    }

    /// Processes a single raw scan code and updates state, returning an ASCII
    /// character if a printable or control key event occurred.
    pub fn process_scancode(&mut self, scancode: u8) -> Option<char> {
        // Handle extended prefix (0xE0)
        if scancode == 0xE0 {
            self.extended = true;
            return None;
        }

        // If extended sequence active
        if self.extended {
            self.extended = false;
            // Handled extended release or special keys
            match scancode {
                0x1D => { self.ctrl = true; return None; }       // Right Ctrl press
                0x9D => { self.ctrl = false; return None; }      // Right Ctrl release
                0x38 => { self.alt = true; return None; }        // Right Alt press
                0xB8 => { self.alt = false; return None; }       // Right Alt release
                _ => return None,
            }
        }

        // Check for Break code (Key Release: bit 7 set)
        if (scancode & 0x80) != 0 {
            let make_code = scancode & 0x7F;
            match make_code {
                0x2A | 0x36 => self.shift = false, // Left / Right Shift released
                0x1D => self.ctrl = false,         // Ctrl released
                0x38 => self.alt = false,          // Alt released
                _ => {}
            }
            return None;
        }

        // Handle Make codes (Key Press: bit 7 clear)
        match scancode {
            0x2A | 0x36 => {
                self.shift = true;
                None
            }
            0x1D => {
                self.ctrl = true;
                None
            }
            0x38 => {
                self.alt = true;
                None
            }
            0x3A => {
                // CapsLock toggles state
                self.caps_lock = !self.caps_lock;
                None
            }
            0x1C => Some('\n'),     // Enter
            0x0E => Some('\x08'),   // Backspace
            0x0F => Some('\t'),     // Tab
            0x01 => Some('\x1B'),   // Escape
            0x39 => Some(' '),      // Spacebar
            // Printable characters
            code => self.translate_printable(code),
        }
    }

    /// Maps standard Scan Code Set 1 make codes to ASCII characters.
    fn translate_printable(&self, code: u8) -> Option<char> {
        let is_uppercase = self.shift ^ self.caps_lock;

        // Letters
        let letter = match code {
            0x1E => Some('a'),
            0x30 => Some('b'),
            0x2E => Some('c'),
            0x20 => Some('d'),
            0x12 => Some('e'),
            0x21 => Some('f'),
            0x22 => Some('g'),
            0x23 => Some('h'),
            0x17 => Some('i'),
            0x24 => Some('j'),
            0x25 => Some('k'),
            0x26 => Some('l'),
            0x32 => Some('m'),
            0x31 => Some('n'),
            0x18 => Some('o'),
            0x19 => Some('p'),
            0x10 => Some('q'),
            0x13 => Some('r'),
            0x1F => Some('s'),
            0x14 => Some('t'),
            0x16 => Some('u'),
            0x2F => Some('v'),
            0x11 => Some('w'),
            0x2D => Some('x'),
            0x15 => Some('y'),
            0x2C => Some('z'),
            _ => None,
        };

        if let Some(ch) = letter {
            return if is_uppercase {
                Some(ch.to_ascii_uppercase())
            } else {
                Some(ch)
            };
        }

        // Numbers & Symbols (affected only by shift, not caps lock)
        let symbol = if self.shift {
            match code {
                0x02 => Some('!'),
                0x03 => Some('@'),
                0x04 => Some('#'),
                0x05 => Some('$'),
                0x06 => Some('%'),
                0x07 => Some('^'),
                0x08 => Some('&'),
                0x09 => Some('*'),
                0x0A => Some('('),
                0x0B => Some(')'),
                0x0C => Some('_'),
                0x0D => Some('+'),
                0x1A => Some('{'),
                0x1B => Some('}'),
                0x27 => Some(':'),
                0x28 => Some('"'),
                0x29 => Some('~'),
                0x2B => Some('|'),
                0x33 => Some('<'),
                0x34 => Some('>'),
                0x35 => Some('?'),
                _ => None,
            }
        } else {
            match code {
                0x02 => Some('1'),
                0x03 => Some('2'),
                0x04 => Some('3'),
                0x05 => Some('4'),
                0x06 => Some('5'),
                0x07 => Some('6'),
                0x08 => Some('7'),
                0x09 => Some('8'),
                0x0A => Some('9'),
                0x0B => Some('0'),
                0x0C => Some('-'),
                0x0D => Some('='),
                0x1A => Some('['),
                0x1B => Some(']'),
                0x27 => Some(';'),
                0x28 => Some('\''),
                0x29 => Some('`'),
                0x2B => Some('\\'),
                0x33 => Some(','),
                0x34 => Some('.'),
                0x35 => Some('/'),
                _ => None,
            }
        };

        symbol
    }
}

/// Global synchronized keyboard decoder state.
pub static KEYBOARD: Spinlock<KeyboardState> = Spinlock::new(KeyboardState::new());

/// Checks if an unread byte is pending in the PS/2 controller output buffer.
#[inline]
pub fn has_data() -> bool {
    unsafe {
        let status = inb(STATUS_PORT);
        (status & STATUS_OUTPUT_BUFFER_FULL) != 0
    }
}

/// Reads a raw scan code from the PS/2 controller data port, or returns `None`
/// if no key event is currently waiting.
pub fn read_scancode() -> Option<u8> {
    if has_data() {
        let code = unsafe { inb(DATA_PORT) };
        Some(code)
    } else {
        None
    }
}

/// Polls for an available scan code and decodes it into an ASCII character,
/// updating modifier states (Shift, CapsLock, Ctrl, Alt).
///
/// Returns `Some(char)` if a valid ASCII key or control character was pressed.
pub fn read_char() -> Option<char> {
    while let Some(scancode) = read_scancode() {
        if let Some(ch) = KEYBOARD.lock().process_scancode(scancode) {
            return Some(ch);
        }
    }
    None
}

/// Waits until the PS/2 controller input buffer is empty and ready to receive a command.
fn wait_for_input_buffer() {
    for _ in 0..100_000 {
        let status = unsafe { inb(STATUS_PORT) };
        if (status & STATUS_INPUT_BUFFER_FULL) == 0 {
            break;
        }
        core::hint::spin_loop();
    }
}

/// Initializes the PS/2 8042 Keyboard Controller.
///
/// 1. Flushes any residual or stale scan codes from the controller output buffer.
/// 2. Enables the first PS/2 port (keyboard interface).
pub fn init() {
    println!("----------------------------------------");
    println!(" [KEYBOARD] Initializing PS/2 Controller");
    println!("----------------------------------------");

    // Flush any pending data in the output buffer (up to 32 bytes)
    for _ in 0..32 {
        if has_data() {
            unsafe {
                let _ = inb(DATA_PORT);
            }
        } else {
            break;
        }
    }

    // Wait for input buffer to clear, then enable first PS/2 port
    wait_for_input_buffer();
    unsafe {
        outb(COMMAND_PORT, CMD_ENABLE_FIRST_PORT);
    }

    println!("[KEYBOARD] Intel 8042 PS/2 Keyboard Driver active (Scan Code Set 1).");
}

/// Runs automated verification tests on the Scan Code Set 1 decoder and state machine.
pub fn run_self_tests() {
    println!("[KEYBOARD] Running PS/2 keyboard verification self-tests...");

    let mut state = KeyboardState::new();

    // Test 1: Simple lowercase letter 'a' (make code 0x1E)
    let c = state.process_scancode(0x1E);
    assert_eq!(c, Some('a'), "Expected 'a'");

    // Test 2: Shift + 'a' -> 'A'
    state.process_scancode(0x2A); // Left Shift press
    let c = state.process_scancode(0x1E);
    assert_eq!(c, Some('A'), "Expected 'A' with Shift held");

    // Test 3: Shift release -> 'a'
    state.process_scancode(0xAA); // Left Shift release (0x2A | 0x80)
    let c = state.process_scancode(0x1E);
    assert_eq!(c, Some('a'), "Expected 'a' after Shift released");

    // Test 4: CapsLock toggle -> 'A'
    state.process_scancode(0x3A); // CapsLock make
    let c = state.process_scancode(0x1E);
    assert_eq!(c, Some('A'), "Expected 'A' with CapsLock on");

    // Test 5: Number key '1' (0x02) and Shift + '1' -> '!'
    state.process_scancode(0x3A); // CapsLock off
    let c = state.process_scancode(0x02);
    assert_eq!(c, Some('1'), "Expected '1'");
    state.process_scancode(0x2A); // Shift press
    let c = state.process_scancode(0x02);
    assert_eq!(c, Some('!'), "Expected '!' with Shift");
    state.process_scancode(0xAA); // Shift release

    // Test 6: Control keys: Enter (0x1C), Backspace (0x0E), Space (0x39)
    assert_eq!(state.process_scancode(0x1C), Some('\n'), "Expected newline for Enter");
    assert_eq!(state.process_scancode(0x0E), Some('\x08'), "Expected backspace for 0x0E");
    assert_eq!(state.process_scancode(0x39), Some(' '), "Expected space for 0x39");

    println!("[KEYBOARD] All PS/2 keyboard self-tests passed successfully!");
}
