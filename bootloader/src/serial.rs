//! 16550 UART Serial Port Driver for Bootloader Diagnostics.
//!
//! # Overview
//! Provides low-level port-mapped I/O communication over standard IBM PC COM1 (`0x3F8`).
//! Serial communication operates independently of screen resolution, graphics protocols,
//! or firmware video drivers, providing an indispensable early diagnostic link.
//!
//! # Hardware Port Mapping (COM1 Base = 0x3F8)
//! - `0x3F8`: Data Register (TX/RX) or Baud Rate Divisor Latch Low (when DLAB=1)
//! - `0x3F9`: Interrupt Enable Register (IER) or Baud Rate Divisor Latch High (when DLAB=1)
//! - `0x3FA`: FIFO Control Register (FCR)
//! - `0x3FB`: Line Control Register (LCR) - Bit 7 controls DLAB
//! - `0x3FC`: Modem Control Register (MCR)
//! - `0x3FD`: Line Status Register (LSR) - Bit 5 indicates Transmitter Empty (THRE)

use core::fmt;

/// Initializes the 16550 UART COM1 controller to 115,200 baud, 8-N-1 format.
///
/// # Hardware Steps
/// 1. Disables all UART interrupts (`IER = 0x00`) to operate in polled mode.
/// 2. Sets the Divisor Latch Access Bit (`LCR |= 0x80`) to program the baud rate generator.
/// 3. Writes divisor value `1` (`115,200` baud = `1,843,200 Hz / (16 * 1)`).
/// 4. Configures 8 data bits, no parity, and 1 stop bit (8-N-1 = `0x03`).
/// 5. Enables and flushes both 14-byte transmit and receive hardware FIFOs (`FCR = 0xC7`).
/// 6. Asserts Request-To-Send (RTS) and Data-Terminal-Ready (DTR) lines (`MCR = 0x0B`).
///
/// # Safety
/// Performs raw I/O port instructions (`outb`) to legacy PC ports `0x3F8..0x3FD`.
pub unsafe fn init() {
    unsafe {
        // Step 1: Disable UART interrupts for clean polling operation
        core::arch::asm!("out dx, al", in("dx") 0x3F9u16, in("al") 0x00u8, options(nomem, nostack));

        // Step 2: Enable DLAB (Divisor Latch Access Bit) to configure baud rate
        core::arch::asm!("out dx, al", in("dx") 0x3FBu16, in("al") 0x80u8, options(nomem, nostack));

        // Step 3: Set baud rate divisor to 1 (115,200 baud)
        core::arch::asm!("out dx, al", in("dx") 0x3F8u16, in("al") 0x01u8, options(nomem, nostack)); // DLL
        core::arch::asm!("out dx, al", in("dx") 0x3F9u16, in("al") 0x00u8, options(nomem, nostack)); // DLH

        // Step 4: 8 data bits, no parity, 1 stop bit (8-N-1), clear DLAB
        core::arch::asm!("out dx, al", in("dx") 0x3FBu16, in("al") 0x03u8, options(nomem, nostack));

        // Step 5: Enable 16-byte FIFO buffers and clear TX/RX contents
        core::arch::asm!("out dx, al", in("dx") 0x3FAu16, in("al") 0xC7u8, options(nomem, nostack));

        // Step 6: Set RTS/DSR modem signals and auxiliary output
        core::arch::asm!("out dx, al", in("dx") 0x3FCu16, in("al") 0x0Bu8, options(nomem, nostack));
    }
}

/// Transmits a single byte over COM1, blocking until the transmitter FIFO is ready.
///
/// Polls Line Status Register (LSR at `0x3FD`) until bit 5 (Transmitter Holding
/// Register Empty - THRE) is asserted before writing the byte to port `0x3F8`.
pub fn write_byte(b: u8) {
    unsafe {
        let mut status: u8;
        loop {
            core::arch::asm!("in al, dx", in("dx") 0x3FDu16, out("al") status, options(nomem, nostack));
            if status & 0x20 != 0 {
                break;
            }
        }
        core::arch::asm!("out dx, al", in("dx") 0x3F8u16, in("al") b, options(nomem, nostack));
    }
}

/// Transmits a string slice over COM1 with automatic CRLF (`\r\n`) normalization.
pub fn write_str(s: &str) {
    for b in s.bytes() {
        if b == b'\n' {
            write_byte(b'\r');
        }
        write_byte(b);
    }
}

/// Zero-sized marker type implementing `core::fmt::Write` for serial output formatting.
pub struct SerialWriter;

impl fmt::Write for SerialWriter {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        write_str(s);
        Ok(())
    }
}

/// Formatted print macro outputting directly to COM1 serial.
#[macro_export]
macro_rules! serial_print {
    ($($arg:tt)*) => {{
        use core::fmt::Write;
        let _ = write!($crate::serial::SerialWriter, $($arg)*);
    }};
}

/// Formatted print line macro outputting to COM1 serial with trailing newline.
#[macro_export]
macro_rules! serial_println {
    () => ($crate::serial_print!("\n"));
    ($($arg:tt)*) => {{
        use core::fmt::Write;
        let _ = writeln!($crate::serial::SerialWriter, $($arg)*);
    }};
}
