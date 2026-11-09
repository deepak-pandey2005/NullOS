#![no_std]
#![no_main]

extern crate alloc;

mod arch;
mod drivers;
mod fs;
mod graphics;
mod ipc;
mod memory;
mod net;
mod security;
mod sync;
mod task;

use arch::x86_64::{apic, gdt, idt};
use bootinfo::BootInfo;
use core::panic::PanicInfo;

#[unsafe(no_mangle)]
pub extern "C" fn kernel_main(boot_info: &'static BootInfo) -> ! {
    drivers::serial::init();
    drivers::console::init(&boot_info.framebuffer);

    // Initialize Global Descriptor Table & Task State Segment (M4)
    gdt::init();

    // Initialize Interrupt Descriptor Table & CPU Exceptions (M5)
    idt::init();

    // Initialize modern CPU features & security flags (CR0.WP, EFER.NXE, CR4.OSFXSR/PGE)
    arch::x86_64::cpu::init();

    // Initialize Fast SYSCALL / SYSRET ABI & MSRs (Phase 5 - M13)
    arch::x86_64::syscall::init();

    // Initialize Local APIC & Timers (M6)
    apic::init();

    // Enable CPU interrupts
    unsafe {
        core::arch::asm!("sti");
    }

    println!();
    println!("========================================");
    println!("       NullOS Kernel Reached!           ");
    println!("========================================");
    println!("[KERNEL] Control transferred from UEFI bootloader to kernel_main.");
    println!("[KERNEL] GDT & TSS initialized (Kernel CS: {:#x}, TSS: {:#x})", gdt::KERNEL_CODE_SELECTOR, gdt::TSS_SELECTOR);
    println!("[KERNEL] IDT initialized with 20 CPU exception handlers.");
    println!("[KERNEL] APIC initialized, 8259 PIC disabled, periodic timer active.");

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

    // Test timer interrupt & sleep primitive
    apic::sleep(10);
    println!("[KERNEL] APIC Timer verified: {} ticks recorded.", apic::get_ticks());

    // Initialize Physical Memory Manager (Phase 3 - M7, M8, M9)
    memory::init(boot_info);
    memory::run_self_tests();

    // Initialize ACPI Subsystem & MADT CPU Enumeration (Phase 9 - M25 SMP)
    arch::x86_64::acpi::init(boot_info.acpi_rsdp);
    arch::x86_64::acpi::run_self_tests();

    // Initialize Multiprocessing & Bootstrap Application Processors (Phase 9 - M25 SMP)
    arch::x86_64::smp::init();
    arch::x86_64::smp::run_self_tests();

    // Initialize Kernel Hardening & Security Subsystem (Phase 10 - M26)
    security::init_canary();
    security::run_self_tests();

    // Initialize Preemptive Multitasking & Kernel Threads (Phase 4 - M10, M11 & Phase 5 - M12, M13, M14)
    task::init();
    task::run_self_tests();

    // Initialize Virtual File System & Initramfs (Phase 6 - M15, M16)
    fs::init();
    fs::run_self_tests();

    // Initialize Inter-Process Communication Subsystem (Phase 10 - M27 IPC)
    ipc::run_self_tests();

    // Initialize Async & Event Infrastructure (Phase 11 - M29 Async)
    task::event::init();

    // Initialize 2D Graphics Canvas & Desktop Window Manager (Phase 12 - M30 Graphics)
    graphics::init(&boot_info.framebuffer);

    // Initialize PCI Bus Enumeration & Hardware Device Discovery (Phase 6 - M17 PCI)
    drivers::pci::init();
    drivers::pci::run_self_tests();

    // Initialize USB 3.0 xHCI Controller & Devices (Phase 12 - M31)
    drivers::usb::init();
    drivers::usb::run_self_tests();

    // Initialize Intel High Definition Audio (HDA) Subsystem (Phase 12 - M32)
    drivers::audio::init();
    drivers::audio::run_self_tests();

    // Initialize VirtIO Block Storage Subsystem (Phase 6 - M18 Block Devices)
    // NOTE: VirtIO block devices operate in virtualized environments (QEMU/KVM).
    // On physical bare-metal hardware, storage requires AHCI/SATA or NVMe drivers.
    // If no VirtIO disk is detected, the kernel boots using the in-memory RamFS ('/').
    drivers::virtio_blk::init();
    drivers::virtio_blk::run_self_tests();

    // Mount Persistent Block Storage Filesystems (Phase 6 - M19 Filesystems)
    fs::mount_block_devices();

    // Initialize Dynamic Linker & Shared Library Subsystem (Phase 14 - M34)
    task::dynlink::init();

    // Initialize VirtIO Network Subsystem (Phase 8 - M23 Network Driver)
    // NOTE: VirtIO network operates in hypervisors. Physical bare metal requires
    // native PCI Ethernet drivers (Intel e1000e / Realtek RTL8168) and DHCP.
    drivers::virtio_net::init();
    drivers::virtio_net::run_self_tests();

    // Initialize TCP/IP Network Stack (Phase 8 - M24 Network Stack)
    net::init();
    net::run_self_tests();

    // Initialize PS/2 Keyboard Driver (Scan Code Set 1 & Input Decoding)
    drivers::keyboard::init();
    drivers::keyboard::run_self_tests();


    // Verify Interactive Shell & CLI commands (Phase 7 - Userspace Shell)
    let cmdline_str = if boot_info.cmdline_addr != 0 && boot_info.cmdline_len > 0 {
        let slice = unsafe {
            core::slice::from_raw_parts(
                boot_info.cmdline_addr as *const u8,
                boot_info.cmdline_len,
            )
        };
        core::str::from_utf8(slice).unwrap_or("")
    } else {
        ""
    };

    let is_test_mode = cmdline_str.contains("test") || check_qemu_fw_cfg_test();
    if is_test_mode {
        println!("[KERNEL] Test mode detected. Running automated test suite...");
        task::shell::run_automated_tests();
    } else {
        println!("[KERNEL] Fast boot active. Type 'test' in the shell to run all automated tests.");
    }

    println!();
    println!("[KERNEL] Kernel initialization complete. Shell active.");
    println!();

    // Hand execution over to the interactive shell (with cooperative yield for background tasks)
    task::shell::interactive_loop();

    loop {
        unsafe {
            core::arch::asm!("hlt");
        }
    }
}

/// Queries the QEMU fw_cfg port (0x510/0x511) to check if automated test mode was requested.
fn check_qemu_fw_cfg_test() -> bool {
    unsafe {
        // Select signature (0x0000)
        core::arch::asm!("out dx, ax", in("dx") 0x510u16, in("ax") 0x0000u16, options(nomem, nostack));
        let mut sig = [0u8; 4];
        for b in &mut sig {
            core::arch::asm!("in al, dx", in("dx") 0x511u16, out("al") *b, options(nomem, nostack));
        }
        if &sig != b"QEMU" {
            return false;
        }

        // Query file directory at selector 0x0019
        core::arch::asm!("out dx, ax", in("dx") 0x510u16, in("ax") 0x0019u16, options(nomem, nostack));
        let mut count_bytes = [0u8; 4];
        for b in &mut count_bytes {
            core::arch::asm!("in al, dx", in("dx") 0x511u16, out("al") *b, options(nomem, nostack));
        }
        let count = u32::from_be_bytes(count_bytes);
        if count > 256 {
            return false;
        }
        for _ in 0..count {
            let mut file_entry = [0u8; 64];
            for b in &mut file_entry {
                core::arch::asm!("in al, dx", in("dx") 0x511u16, out("al") *b, options(nomem, nostack));
            }
            let name_bytes = &file_entry[8..64];
            if let Ok(name) = core::str::from_utf8(name_bytes) {
                if name.starts_with("opt/test") {
                    return true;
                }
            }
        }
        false
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
