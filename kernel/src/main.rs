#![no_std]
#![no_main]

mod console;
mod serial;

use bootinfo::BootInfo;
use core::panic::PanicInfo;

#[unsafe(no_mangle)]
pub extern "C" fn kernel_main(boot_info: &'static BootInfo) -> ! {
    console::init(&boot_info.framebuffer);

    println!();
    println!("========================================");
    println!("       NullOS Kernel Reached!           ");
    println!("========================================");
    println!("[KERNEL] Control transferred from UEFI bootloader to kernel_main.");

    println!("[KERNEL] BootInfo received at: {:#018x}", boot_info as *const _ as u64);
    println!("[KERNEL] Framebuffer base:     {:#018x}", boot_info.framebuffer.address);
    println!("[KERNEL] MemoryMap buffer:     {:#018x}", boot_info.memory_map.address);
    println!("[KERNEL] ACPI RSDP table:      {:#018x}", boot_info.acpi_rsdp);

    // Display kernel command line
    if boot_info.cmdline_len > 0 && boot_info.cmdline_addr != 0 {
        let cmdline = unsafe {
            core::slice::from_raw_parts(
                boot_info.cmdline_addr as *const u8,
                boot_info.cmdline_len,
            )
        };
        match core::str::from_utf8(cmdline) {
            Ok(s) => println!("[KERNEL] Command line: \"{}\"", s),
            Err(_) => println!("[KERNEL] Command line: (invalid UTF-8, {} bytes)", boot_info.cmdline_len),
        }
    } else {
        println!("[KERNEL] Command line: (none)");
    }

    // Validate framebuffer
    let fb = &boot_info.framebuffer;
    if fb.address != 0 && fb.width > 0 && fb.height > 0 {
        println!(
            "[KERNEL] GOP Framebuffer verified: {}x{}, stride: {}, base: {:#x}",
            fb.width, fb.height, fb.stride, fb.address
        );

        let has_mmap = boot_info.memory_map.address != 0 && boot_info.memory_map.size > 0;
        let has_acpi = boot_info.acpi_rsdp != 0;

        if has_mmap {
            println!("[KERNEL] UEFI MemoryMap verified.");
        }
        if has_acpi {
            println!("[KERNEL] ACPI RSDP verified.");
        }
    } else {
        println!("[KERNEL] Warning: No framebuffer found in BootInfo.");
    }

    println!("[KERNEL] Kernel initialization complete. Halting CPU.");

    loop {
        unsafe {
            core::arch::asm!("hlt");
        }
    }
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    println!("[KERNEL PANIC] {}", info);
    loop {
        unsafe {
            core::arch::asm!("cli; hlt");
        }
    }
}
