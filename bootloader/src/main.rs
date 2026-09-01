#![no_std]
#![no_main]

use core::panic::PanicInfo;
use uefi::entry;

#[entry]
fn main() -> uefi::Status {
    let _ = uefi::helpers::init();

    uefi::println!("================================");
    uefi::println!("       NullOS Bootloader        ");
    uefi::println!("================================");
    uefi::println!("UEFI Boot Successful!");
    loop {
        core::hint::spin_loop();
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}
