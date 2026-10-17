use core::fmt;

pub unsafe fn init() {
    unsafe {
        // Disable interrupts
        core::arch::asm!("out dx, al", in("dx") 0x3F9u16, in("al") 0x00u8, options(nomem, nostack));
        // Enable DLAB (baud rate divisor)
        core::arch::asm!("out dx, al", in("dx") 0x3FBu16, in("al") 0x80u8, options(nomem, nostack));
        // Set divisor to 1 (115200 baud)
        core::arch::asm!("out dx, al", in("dx") 0x3F8u16, in("al") 0x01u8, options(nomem, nostack));
        core::arch::asm!("out dx, al", in("dx") 0x3F9u16, in("al") 0x00u8, options(nomem, nostack));
        // 8 bits, no parity, 1 stop bit
        core::arch::asm!("out dx, al", in("dx") 0x3FBu16, in("al") 0x03u8, options(nomem, nostack));
        // Enable FIFO, clear them
        core::arch::asm!("out dx, al", in("dx") 0x3FAu16, in("al") 0xC7u8, options(nomem, nostack));
        // RTS/DSR set
        core::arch::asm!("out dx, al", in("dx") 0x3FCu16, in("al") 0x0Bu8, options(nomem, nostack));
    }
}

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

pub fn write_str(s: &str) {
    for b in s.bytes() {
        if b == b'\n' {
            write_byte(b'\r');
        }
        write_byte(b);
    }
}

pub struct SerialWriter;

impl fmt::Write for SerialWriter {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        write_str(s);
        Ok(())
    }
}

#[macro_export]
macro_rules! serial_print {
    ($($arg:tt)*) => {{
        use core::fmt::Write;
        let _ = write!($crate::serial::SerialWriter, $($arg)*);
    }};
}

#[macro_export]
macro_rules! serial_println {
    () => ($crate::serial_print!("\n"));
    ($($arg:tt)*) => {{
        use core::fmt::Write;
        let _ = writeln!($crate::serial::SerialWriter, $($arg)*);
    }};
}
