#![no_std]
#![no_main]

extern crate alloc;

mod arch;
mod drivers;
mod fs;
mod memory;
mod sync;
mod task;

use arch::x86_64::{apic, gdt, idt};
use bootinfo::BootInfo;
use core::panic::PanicInfo;

#[unsafe(no_mangle)]
pub extern "C" fn kernel_main(boot_info: &'static BootInfo) -> ! {
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

    // Initialize Preemptive Multitasking & Kernel Threads (Phase 4 - M10, M11 & Phase 5 - M12, M13, M14)
    task::init();
    task::run_self_tests();

    // Initialize Virtual File System & Initramfs (Phase 6 - M15, M16)
    fs::init();
    fs::run_self_tests();

    // Verify Interactive Shell & CLI commands (Phase 7 - Userspace Shell)
    task::shell::run_automated_tests();

    // Launch interactive shell thread
    task::spawn("shell", task::shell::interactive_loop, 1);

    println!("[KERNEL] Kernel initialization complete. Shell active, CPU idling with interrupts enabled.");

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
