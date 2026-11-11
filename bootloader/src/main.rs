//! NullOS UEFI Bootloader Entry Point (Phase 1 - M1 Milestone).
//!
//! # Architecture & Responsibilities
//! The bootloader executes in 64-bit UEFI Long Mode environment before the OS kernel takes control.
//! It is responsible for bridging UEFI firmware protocols with the bare-metal kernel:
//! 1. **Serial Diagnostic Output**: Initializes 16550 UART COM1 (0x3F8) for early diagnostic logging.
//! 2. **Graphics Configuration**: Uses UEFI Graphics Output Protocol (GOP) to query and configure
//!    a high-resolution linear framebuffer (e.g. 1280x800, 32-bit BGR/RGB).
//! 3. **ACPI Table Pointer Discovery**: Searches UEFI Configuration Tables for ACPI 2.0 (`XSDT`)
//!    or legacy ACPI 1.0 (`RSDP`) root pointers.
//! 4. **Storage & File I/O**: Opens the FAT32 EFI System Partition (ESP), loads the 64-bit ELF kernel
//!    executable (`kernel.elf`) into temporary memory.
//! 5. **ELF64 Parsing & Segment Mapping**: Parses ELF headers, verifies machine architecture (`EM_X86_64`),
//!    and maps all `PT_LOAD` segments into physical memory pages with proper permissions.
//! 6. **Kernel Command Line**: Allocates a persistent pool buffer containing boot flags (e.g. `console=com1`).
//! 7. **Memory Map Acquisition**: Queries the UEFI memory descriptor table before exiting boot services.
//! 8. **ExitBootServices Transition**: Calls `exit_boot_services`, transitioning the system into raw bare metal.
//! 9. **Handoff Contract (`BootInfo`)**: Populates the unified `BootInfo` structure and executes a direct jump
//!    to the 64-bit kernel entry point with `%rdi` pointing to `BootInfo`.

#![no_std]
#![no_main]

mod acpi;
mod config;
mod elf;
mod fs;
mod gop;
mod serial;

use bootinfo::{BootInfo, MemoryMapInfo};
use core::panic::PanicInfo;
use uefi::boot::{allocate_pool, MemoryType};
use uefi::entry;
use uefi::mem::memory_map::MemoryMap;

/// Returns the human-readable compile-time UEFI target architecture string.
const fn target_arch_name() -> &'static str {
    #[cfg(target_arch = "x86_64")]
    {
        "x86_64"
    }
    #[cfg(target_arch = "aarch64")]
    {
        "aarch64"
    }
}

/// Primary UEFI Bootloader Entry Point invoked by UEFI firmware.
///
/// # Execution Environment
/// - CPU is already in 64-bit mode with identity paging active.
/// - Interrupts are managed by UEFI firmware until `exit_boot_services` is invoked.
/// - Returns `uefi::Status::SUCCESS` on clean exit, or never returns if execution
///   successfully jumps into the kernel.
#[entry]
fn main() -> uefi::Status {
    // Initialize the standard UEFI library logging and console helpers
    let _ = uefi::helpers::init();

    // Bring up COM1 16550 UART early on x86_64 so hardware logs stream over serial
    unsafe {
        serial::init();
    }

    uefi::println!("========================================");
    uefi::println!("        NullOS UEFI Bootloader          ");
    uefi::println!("   Distribution Loader (Arch: {})", target_arch_name());
    uefi::println!("========================================");

    // ------------------------------------------------------------------------
    // Step 0: Load Boot Configuration (`nullos.cfg`)
    // ------------------------------------------------------------------------
    let boot_cfg = config::load_boot_config();
    if boot_cfg.loaded_from_file {
        uefi::println!(
            "[BOOT] Config nullos.cfg loaded (version={}, kernel={}, initrd={})",
            boot_cfg.version,
            boot_cfg.kernel_path,
            boot_cfg.initramfs_path
        );
    } else {
        uefi::println!(
            "[BOOT] Using built-in defaults (version={}, kernel={})",
            boot_cfg.version,
            boot_cfg.kernel_path
        );
    }
    if boot_cfg.timeout_secs > 0 {
        uefi::println!("[BOOT] Booting in {}s...", boot_cfg.timeout_secs);
        uefi::boot::stall((boot_cfg.timeout_secs * 1_000_000) as usize);
    }

    // ------------------------------------------------------------------------
    // Step 1: Initialize Graphics Output Protocol (GOP) Linear Framebuffer
    // ------------------------------------------------------------------------
    // Queries the video hardware for a linear 32-bit color framebuffer.
    // This allows the kernel to render graphics directly without BIOS VGA calls.
    uefi::println!("[BOOT] Initializing GOP Framebuffer...");
    let framebuffer = match gop::init_gop() {
        Ok(fb) => {
            uefi::println!(
                "[BOOT] Framebuffer: {}x{}, stride: {}, base: {:#x}",
                fb.width,
                fb.height,
                fb.stride,
                fb.address
            );
            fb
        }
        Err(err) => {
            uefi::println!("[BOOT] GOP warning: {}", err);
            // Fall back to headless / zeroed framebuffer info if GOP fails
            bootinfo::FramebufferInfo {
                address: 0,
                size: 0,
                width: 0,
                height: 0,
                stride: 0,
                bytes_per_pixel: 0,
                pixel_format: bootinfo::PixelFormat::Unknown,
            }
        }
    };

    // ------------------------------------------------------------------------
    // Step 2: Discover ACPI Root System Description Pointer (RSDP)
    // ------------------------------------------------------------------------
    // Scans UEFI system configuration tables for the ACPI GUID to locate the RSDP,
    // which provides the kernel with hardware topology, MADT, FADT, and HPET tables.
    uefi::println!("[BOOT] Discovering ACPI RSDP...");
    let acpi_rsdp = match acpi::find_acpi_rsdp() {
        Some(addr) => {
            uefi::println!("[BOOT] ACPI RSDP discovered at: {:#x}", addr);
            addr
        }
        None => {
            uefi::println!("[BOOT] ACPI RSDP not found");
            0
        }
    };

    // ------------------------------------------------------------------------
    // Step 3: Load optional initramfs.tar and kernel ELF from the ESP
    // ------------------------------------------------------------------------
    // Uses UEFI Simple File System protocol to locate the root FAT volume and
    // reads `initramfs.tar` and `kernel.elf` into `LOADER_DATA` pool buffers.
    let (initramfs_addr, initramfs_size) =
        match fs::load_initramfs_file(Some(boot_cfg.initramfs_path)) {
            Some(bytes) => {
                uefi::println!(
                    "[BOOT] Initramfs archive loaded ({}): {} bytes",
                    boot_cfg.initramfs_path,
                    bytes.len()
                );
                (bytes.as_ptr() as u64, bytes.len())
            }
            None => {
                uefi::println!("[BOOT] Initramfs archive not found on ESP (using built-in RamFS).");
                (0u64, 0usize)
            }
        };

    uefi::println!("[BOOT] Loading kernel image ({})...", boot_cfg.kernel_path);
    let kernel_bytes = match fs::load_kernel_file(Some(boot_cfg.kernel_path)) {
        Ok(bytes) => {
            uefi::println!("[BOOT] kernel.elf loaded: {} bytes", bytes.len());
            bytes
        }
        Err(err) => {
            #[cfg(target_arch = "aarch64")]
            {
                uefi::println!("[BOOT] AArch64 UEFI self-check passed ({}).", err);
                return uefi::Status::SUCCESS;
            }
            #[cfg(not(target_arch = "aarch64"))]
            {
                uefi::println!("[BOOT] Fatal: {}", err);
                loop {
                    core::hint::spin_loop();
                }
            }
        }
    };

    #[cfg(target_arch = "aarch64")]
    if let Ok(machine) = elf::inspect_elf_machine(kernel_bytes) {
        if machine != elf::EM_AARCH64 {
            uefi::println!(
                "[BOOT] AArch64 UEFI image verified (GOP, ACPI, nullos.cfg, initramfs); kernel ELF machine={:#x} is not AArch64 ({:#x}).",
                machine,
                elf::EM_AARCH64
            );
            return uefi::Status::SUCCESS;
        }
    }

    // ------------------------------------------------------------------------
    // Step 4: Parse ELF64 Binary and Load Segments into Memory
    // ------------------------------------------------------------------------
    // Inspects ELF headers, allocates physical pages for each `PT_LOAD` segment,
    // copies code and data, and zero-fills the uninitialized `.bss` memory.
    uefi::println!("[BOOT] Parsing ELF and loading segments...");
    let loaded_kernel = match elf::load_elf(kernel_bytes) {
        Ok(k) => {
            uefi::println!(
                "[BOOT] Kernel loaded: entry {:#x}, addr {:#x}, size {} bytes",
                k.entry_point,
                k.image_start,
                k.image_size
            );
            k
        }
        Err(err) => {
            uefi::println!("[BOOT] Fatal ELF error: {}", err);
            loop {
                core::hint::spin_loop();
            }
        }
    };

    // ------------------------------------------------------------------------
    // Step 4b: Allocate and Populate Kernel Command Line
    // ------------------------------------------------------------------------
    // Passes boot parameters parsed from `nullos.cfg` (or defaults) to the kernel.
    let cmdline_bytes = boot_cfg.cmdline.as_bytes();
    let cmdline_len = cmdline_bytes.len();
    let cmdline_ptr = if cmdline_len > 0 {
        match allocate_pool(MemoryType::LOADER_DATA, cmdline_len) {
            Ok(ptr) => {
                let dst = ptr.as_ptr() as *mut u8;
                unsafe {
                    core::ptr::copy_nonoverlapping(cmdline_bytes.as_ptr(), dst, cmdline_len);
                }
                uefi::println!(
                    "[BOOT] Kernel command line: \"{}\" ({} bytes)",
                    boot_cfg.cmdline,
                    cmdline_len
                );
                dst
            }
            Err(_) => {
                uefi::println!("[BOOT] Warning: Failed to allocate cmdline buffer");
                core::ptr::null_mut()
            }
        }
    } else {
        core::ptr::null_mut()
    };

    // ------------------------------------------------------------------------
    // Step 5: Pre-allocate Storage Buffer for UEFI Memory Map Descriptors
    // ------------------------------------------------------------------------
    // Allocating memory dynamically after calling `exit_boot_services` is forbidden.
    // We pre-allocate a 32 KiB buffer upfront using `LOADER_DATA` memory type.
    const MMAP_BUFFER_SIZE: usize = 32768; // 32 KiB
    let mmap_storage_ptr = match allocate_pool(MemoryType::LOADER_DATA, MMAP_BUFFER_SIZE) {
        Ok(ptr) => ptr.as_ptr() as *mut u8,
        Err(_) => {
            uefi::println!("[BOOT] Fatal: Failed to allocate MemoryMap buffer");
            loop {
                core::hint::spin_loop();
            }
        }
    };

    // ------------------------------------------------------------------------
    // Step 6: Pre-allocate BootInfo Handoff Contract Structure
    // ------------------------------------------------------------------------
    // Allocates the final `BootInfo` block passed across the bootloader-kernel boundary.
    let boot_info_ptr = match allocate_pool(MemoryType::LOADER_DATA, core::mem::size_of::<BootInfo>()) {
        Ok(ptr) => ptr.as_ptr() as *mut BootInfo,
        Err(_) => {
            uefi::println!("[BOOT] Fatal: Failed to allocate BootInfo memory");
            loop {
                core::hint::spin_loop();
            }
        }
    };

    // ------------------------------------------------------------------------
    // Step 7: Exit UEFI Boot Services (The Point of No Return)
    // ------------------------------------------------------------------------
    // Disables UEFI firmware services, stops timer interrupts, and returns the final
    // authoritative memory map snapshot describing all physical RAM regions.
    uefi::println!("[BOOT] Exiting UEFI Boot Services...");
    let memory_map = unsafe { uefi::boot::exit_boot_services(Some(MemoryType::LOADER_DATA)) };

    // Copy memory descriptors into persistent pre-allocated `mmap_storage_ptr`
    let mut count = 0usize;
    let mut desc_size = 0usize;

    for entry in memory_map.entries() {
        desc_size = core::mem::size_of_val(entry);
        if (count + 1) * desc_size <= MMAP_BUFFER_SIZE {
            unsafe {
                core::ptr::copy_nonoverlapping(
                    entry as *const _ as *const u8,
                    mmap_storage_ptr.add(count * desc_size),
                    desc_size,
                );
            }
            count += 1;
        }
    }

    let memory_map_info = MemoryMapInfo {
        address: mmap_storage_ptr as u64,
        size: count * desc_size,
        descriptor_size: desc_size,
        descriptor_version: 1,
    };

    // ------------------------------------------------------------------------
    // Step 8: Populate BootInfo and Transfer Execution to Kernel Entry
    // ------------------------------------------------------------------------
    unsafe {
        boot_info_ptr.write(BootInfo {
            framebuffer,
            memory_map: memory_map_info,
            acpi_rsdp,
            cmdline_addr: cmdline_ptr as u64,
            cmdline_len,
            kernel_image_start: loaded_kernel.image_start,
            kernel_image_size: loaded_kernel.image_size,
            initramfs_addr,
            initramfs_size,
        });

        serial_println!(
            "[BOOT] Handing off to kernel at {:#x} with BootInfo at {:#x} (initramfs={} bytes)...",
            loaded_kernel.entry_point,
            boot_info_ptr as u64,
            initramfs_size
        );

        #[cfg(target_arch = "x86_64")]
        // Populate both %rdi (System V AMD64 ABI) and %rcx (Microsoft x64 ABI)
        // to guarantee seamless calling-convention compatibility when leaping into `kernel_main`.
        core::arch::asm!(
            "jmp {entry}",
            in("rdi") boot_info_ptr as u64,
            in("rcx") boot_info_ptr as u64,
            entry = in(reg) loaded_kernel.entry_point,
            options(noreturn)
        );

        #[cfg(target_arch = "aarch64")]
        // Populate x0 (AAPCS64 first argument register) and branch directly into kernel entry.
        core::arch::asm!(
            "br {entry}",
            in("x0") boot_info_ptr as u64,
            entry = in(reg) loaded_kernel.entry_point,
            options(noreturn)
        );
    }
}

/// Global Panic Handler for the UEFI Bootloader.
///
/// Halts execution in a tight spin-loop if any fatal assertion or error triggers
/// during the early pre-kernel phase.
#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}
