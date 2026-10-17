#![no_std]
#![no_main]

mod serial;

use bootinfo::BootInfo;
use core::panic::PanicInfo;

#[unsafe(no_mangle)]
pub extern "C" fn kernel_main(boot_info: &'static BootInfo) -> ! {
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

    // Validate framebuffer and draw visual confirmation
    let fb = &boot_info.framebuffer;
    if fb.address != 0 && fb.width > 0 && fb.height > 0 {
        println!("[KERNEL] GOP Framebuffer verified: rendering test graphics...");
        let fb_ptr = fb.address as *mut u32;
        let width = fb.width;
        let height = fb.height;
        let stride = fb.stride;

        // Clear screen to deep slate blue (0x0F172A)
        for y in 0..height {
            for x in 0..width {
                unsafe {
                    *fb_ptr.add(y * stride + x) = 0x000F172A;
                }
            }
        }

        // Draw center confirmation box (Cyan: 0x38BDF8)
        let box_w = if width > 400 { 400 } else { width / 2 };
        let box_h = if height > 200 { 200 } else { height / 2 };
        let box_x = (width - box_w) / 2;
        let box_y = (height - box_h) / 2;

        for y in box_y..(box_y + box_h) {
            for x in box_x..(box_x + box_w) {
                unsafe {
                    *fb_ptr.add(y * stride + x) = 0x0038BDF8;
                }
            }
        }

        // Status bar at top:
        // Green (0x22C55E): MemoryMap and ACPI RSDP present
        // Yellow (0xEAB308): MemoryMap present, ACPI absent
        // Red (0xEF4444): Failed checks
        let has_mmap = boot_info.memory_map.address != 0 && boot_info.memory_map.size > 0;
        let has_acpi = boot_info.acpi_rsdp != 0;

        if has_mmap {
            println!("[KERNEL] UEFI MemoryMap verified.");
        }
        if has_acpi {
            println!("[KERNEL] ACPI RSDP verified.");
        }

        let status_color = if has_mmap && has_acpi {
            0x0022C55E
        } else if has_mmap {
            0x00EAB308
        } else {
            0x00EF4444
        };

        for y in 0..8 {
            for x in 0..width {
                unsafe {
                    *fb_ptr.add(y * stride + x) = status_color;
                }
            }
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
