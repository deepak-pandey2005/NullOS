//! # NullOS Kernel Main Entry Point & Boot Sequencing (Phase 1 through Phase 14).
//!
//! # Architecture Overview
//!
//! The NullOS kernel is a 64-bit freestanding (`no_std`, `no_main`) micro/hybrid hobby
//! operating system kernel written in Rust and x86_64 assembly.
//!
//! ## Control Transfer Protocol (UEFI Handover)
//! 1. The UEFI firmware executes the NullOS UEFI bootloader (`bootloader/src/main.rs`).
//! 2. The bootloader configures Graphics Output Protocol (GOP), queries the UEFI memory map,
//!    loads the 64-bit ELF kernel binary (`kernel.elf`), and constructs a [`BootInfo`] block.
//! 3. The bootloader calls `ExitBootServices()`, identity-maps initial physical memory,
//!    switches to the kernel's PML4 page tables, and jumps to `kernel_main` at entry address.
//!
//! ## Initialization Dependency Order:
//! - **Phase 1-2 (Hardware Early Bringup)**:
//!   1. Serial Port (`COM1`, `0x3F8`): Low-level text logging output.
//!   2. Framebuffer Console: Graphical text rendering via UEFI GOP.
//!   3. GDT & TSS: Segment selectors and Task State Segment privilege stacks.
//!   4. IDT & CPU Exceptions: Vectors 0..31 with assembly stubs and page fault handlers.
//!   5. Modern CPU Features: `CR0.WP` (Write Protect), `EFER.NXE` (No-Execute), `CR4.OSFXSR/PGE`.
//!   6. Fast Syscalls: `STAR`, `LSTAR`, `SFMASK` MSRs for `syscall`/`sysret` instructions.
//!   7. Local APIC & Calibrated Timer: 8259 PIC disable, periodic timer interrupts.
//! - **Phase 3 (Physical Memory Management)**:
//!   8. Frame Allocator & Virtual Paging: UEFI memory map conversion, buddy/bitmap allocation, heap init.
//! - **Phase 9 (Multiprocessing - SMP)**:
//!   9. ACPI & MADT: CPU core enumeration and HPET timer discovery.
//!   10. Application Processor (AP) Bootstrapping: SIPI-INIT-SIPI sequence with real-mode trampoline.
//! - **Phase 10 (Kernel Security & Hardening)**:
//!   11. Stack Canary & Guard Pages: Entropy seeding via RDTSC, address boundary validation.
//! - **Phase 4-5 (Preemptive Multitasking & User Processes)**:
//!   12. Task Scheduler & Threads: Preemptive Round-Robin scheduling, kernel threads, context switching.
//! - **Phase 6 (Virtual File System & Storage)**:
//!   13. In-memory RamFS & VFS: Mount root at `"/"`, `/dev/console`, `/dev/null`, `/dev/zero`.
//!   14. PCI Bus Enumeration: Class and subclass scanning for hardware peripherals.
//!   15. VirtIO Block Storage: Ring buffers, requests, and mounting FAT32/ext2 partitions.
//! - **Phase 8 (TCP/IP Networking)**:
//!   16. VirtIO Net & TCP/IP Stack: Ethernet II, ARP, IPv4, ICMP, UDP, TCP, and Sockets API.
//! - **Phase 12 (Modern Peripherals & GUI)**:
//!   17. USB 3.0 xHCI Controller: TRB rings, device slots, and HID keyboard/mouse decoding.
//!   18. Intel HD Audio: CORB/RIRB rings, codec widget tree traversal, and PCM chime synthesis.
//!   19. Desktop Compositor: Window Manager, 2D Canvas, wallpaper, taskbar, and interactive GUI loop.
//! - **Phase 7 (Interactive Shell & Test Harness)**:
//!   20. Automated test suite or interactive CLI shell prompt.

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
mod perf;
mod security;
mod storage;
mod sync;
mod task;

use arch::x86_64::{apic, gdt, idt};
use bootinfo::BootInfo;
use core::panic::PanicInfo;

/// Primary kernel entry point invoked directly by the UEFI bootloader.
///
/// # Calling Convention & Safety
/// - Declared with `#[unsafe(no_mangle)]` and `extern "C"` so the linker preserves
///   the exact symbol name and standard System V AMD64 ABI.
/// - Execution starts in 64-bit Long Mode with paging enabled and interrupts disabled (`cli`).
/// - The `boot_info` pointer references a statically allocated structure in physical memory
///   containing the GOP framebuffer pointer, UEFI memory map descriptor buffer, ACPI RSDP,
///   and kernel command line arguments.
///
/// # Divergence
/// This function returns `!` (never returns), concluding in an interactive shell loop or
/// an infinite low-power idle halt loop (`hlt`).
#[unsafe(no_mangle)]
pub extern "C" fn kernel_main(boot_info: &'static BootInfo) -> ! {
    // 1. Initialize serial port (COM1 0x3F8, 115200 baud) for debug output
    drivers::serial::init();

    // 2. Initialize graphical framebuffer console font renderer
    drivers::console::init(&boot_info.framebuffer);

    // 3. Initialize Global Descriptor Table & Task State Segment (M4)
    gdt::init();

    // 4. Initialize Interrupt Descriptor Table & CPU Exceptions (M5)
    idt::init();

    // 5. Initialize modern CPU features & security flags (CR0.WP, EFER.NXE, CR4.OSFXSR/PGE)
    arch::x86_64::cpu::init();

    // 6. Initialize Fast SYSCALL / SYSRET ABI & MSRs (Phase 5 - M13)
    arch::x86_64::syscall::init();

    // 7. Initialize Local APIC & Timers (M6)
    apic::init();

    // 8. Enable CPU hardware interrupts (sti)
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

    // Initialize Phase 17 M37 Kernel Profiling (TSC calibration, PMU & Tracepoints)
    perf::init();

    // Initialize Physical Memory Manager (Phase 3 - M7, M8, M9)
    memory::init(boot_info);
    memory::run_self_tests();

    // Initialize ACPI Subsystem & MADT CPU Enumeration (Phase 9 - M25 SMP)
    arch::x86_64::acpi::init(boot_info.acpi_rsdp);
    arch::x86_64::acpi::run_self_tests();

    // Initialize Multiprocessing & Bootstrap Application Processors (Phase 9 - M25 SMP)
    arch::x86_64::smp::init();
    arch::x86_64::smp::run_self_tests();

    // Initialize Phase 17 M38 Kernel Optimizations (NUMA SRAT/SLIT, FSGSBASE, Slab caches)
    perf::opt::init();

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

    // Initialize PCIe NVMe Storage Controller Driver (Phase 15 - M35)
    drivers::nvme::init();
    drivers::nvme::run_self_tests();

    // Initialize Advanced Storage Subsystem (Block/Page Cache, I/O Scheduler, Async I/O, WAL Journal)
    storage::init();

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

    // Fallback CPU low-power idle halt loop
    loop {
        unsafe {
            core::arch::asm!("hlt");
        }
    }
}

/// Queries the QEMU fw_cfg port (`0x510`/`0x511`) to check if automated test mode was requested.
///
/// Checks for the presence of the `opt/test` key inside the QEMU firmware configuration directory.
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

/// Panic handler invoked when a runtime panic occurs in freestanding kernel code.
///
/// Logs the panic message, source code file, and line number to the serial port and
/// graphical console, then halts the CPU indefinitely with interrupts disabled.
#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    println!("[KERNEL PANIC] {}", info);
    loop {
        unsafe {
            core::arch::asm!("cli; hlt");
        }
    }
}
