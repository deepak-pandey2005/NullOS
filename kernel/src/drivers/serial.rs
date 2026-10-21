/// COM1 serial port driver for kernel debug output.
///
/// Provides low-level byte-oriented serial I/O and `core::fmt::Write`-based
/// `print!` / `println!` macros for formatted kernel output over COM1 (0x3F8).

use core::fmt;

const COM1_DATA: u16 = 0x3F8;
const COM1_LINE_STATUS: u16 = 0x3FD;
const TRANSMIT_EMPTY: u8 = 0x20;
const DATA_READY: u8 = 0x01;

/// Checks if an incoming byte is available in the COM1 receive buffer.
pub fn has_incoming_byte() -> bool {
    unsafe {
        let mut status: u8;
        core::arch::asm!(
            "in al, dx",
            in("dx") COM1_LINE_STATUS,
            out("al") status,
            options(nomem, nostack),
        );
        (status & DATA_READY) != 0
    }
}

/// Reads a byte from COM1 if one is available, without blocking.
pub fn read_byte() -> Option<u8> {
    if has_incoming_byte() {
        unsafe {
            let mut b: u8;
            core::arch::asm!(
                "in al, dx",
                in("dx") COM1_DATA,
                out("al") b,
                options(nomem, nostack),
            );
            Some(b)
        }
    } else {
        None
    }
}

/// Write a single byte to COM1, busy-waiting until the transmit buffer is empty.
pub unsafe fn write_byte(b: u8) {
    unsafe {
        let mut status: u8;
        loop {
            core::arch::asm!(
                "in al, dx",
                in("dx") COM1_LINE_STATUS,
                out("al") status,
                options(nomem, nostack),
            );
            if status & TRANSMIT_EMPTY != 0 {
                break;
            }
        }
        core::arch::asm!(
            "out dx, al",
            in("dx") COM1_DATA,
            in("al") b,
            options(nomem, nostack),
        );
    }
}

/// Write a string slice to COM1 and to the framebuffer console (if initialized).
pub fn write_str(s: &str) {
    for b in s.bytes() {
        if b == b'\n' {
            unsafe { write_byte(b'\r'); }
        }
        unsafe { write_byte(b); }
    }
    crate::drivers::console::write_str(s);
}

/// Zero-sized writer that implements `core::fmt::Write` for COM1 serial output.
pub struct SerialWriter;

impl fmt::Write for SerialWriter {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        write_str(s);
        Ok(())
    }
}

/// Print formatted text to COM1 serial port.
///
/// Usage: `print!("Hello, {}!", "world");`
#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => {{
        use core::fmt::Write;
        let _ = write!($crate::drivers::serial::SerialWriter, $($arg)*);
    }};
}

/// Print formatted text to COM1 serial port, followed by a newline.
///
/// Usage: `println!("Value: {:#x}", 42);`
#[macro_export]
macro_rules! println {
    () => ($crate::print!("\n"));
    ($($arg:tt)*) => {{
        use core::fmt::Write;
        let _ = writeln!($crate::drivers::serial::SerialWriter, $($arg)*);
    }};
}
