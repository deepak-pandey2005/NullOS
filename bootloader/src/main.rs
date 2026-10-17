#![no_std]
#![no_main]

mod acpi;
mod elf;
mod fs;
mod gop;
mod serial;

use bootinfo::{BootInfo, MemoryMapInfo};
use core::panic::PanicInfo;
use uefi::boot::{allocate_pool, MemoryType};
use uefi::entry;
use uefi::mem::memory_map::MemoryMap;

#[entry]
fn main() -> uefi::Status {
    let _ = uefi::helpers::init();
    unsafe {
        serial::init();
    }

    uefi::println!("========================================");
    uefi::println!("        NullOS UEFI Bootloader          ");
    uefi::println!("         Phase 1 — M1 Milestone         ");
    uefi::println!("========================================");

    // 1. Initialize GOP Framebuffer
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

    // 2. Discover ACPI RSDP
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

    // 3. Load kernel.elf from filesystem
    uefi::println!("[BOOT] Loading kernel.elf from filesystem...");
    let kernel_bytes = match fs::load_kernel_file() {
        Ok(bytes) => {
            uefi::println!("[BOOT] kernel.elf loaded: {} bytes", bytes.len());
            bytes
        }
        Err(err) => {
            uefi::println!("[BOOT] Fatal: {}", err);
            loop {
                core::hint::spin_loop();
            }
        }
    };

    // 4. Parse ELF64 and load segments
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

    // 4b. Prepare kernel command line
    const DEFAULT_CMDLINE: &[u8] = b"console=com1 loglevel=info";
    let cmdline_len = DEFAULT_CMDLINE.len();
    let cmdline_ptr = match allocate_pool(MemoryType::LOADER_DATA, cmdline_len) {
        Ok(ptr) => {
            let dst = ptr.as_ptr() as *mut u8;
            unsafe {
                core::ptr::copy_nonoverlapping(DEFAULT_CMDLINE.as_ptr(), dst, cmdline_len);
            }
            uefi::println!(
                "[BOOT] Kernel command line: \"{}\" ({} bytes)",
                core::str::from_utf8(DEFAULT_CMDLINE).unwrap_or("???"),
                cmdline_len
            );
            dst
        }
        Err(_) => {
            uefi::println!("[BOOT] Warning: Failed to allocate cmdline buffer");
            core::ptr::null_mut()
        }
    };

    // 5. Pre-allocate buffer for MemoryMap descriptors
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

    // 6. Pre-allocate BootInfo structure
    let boot_info_ptr = match allocate_pool(MemoryType::LOADER_DATA, core::mem::size_of::<BootInfo>()) {
        Ok(ptr) => ptr.as_ptr() as *mut BootInfo,
        Err(_) => {
            uefi::println!("[BOOT] Fatal: Failed to allocate BootInfo memory");
            loop {
                core::hint::spin_loop();
            }
        }
    };

    // 7. Exit UEFI Boot Services
    uefi::println!("[BOOT] Exiting UEFI Boot Services...");
    let memory_map = unsafe { uefi::boot::exit_boot_services(Some(MemoryType::LOADER_DATA)) };

    // Copy memory descriptors into persistent mmap_storage_ptr
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

    // 8. Populate BootInfo and transfer control to kernel
    unsafe {
        boot_info_ptr.write(BootInfo {
            framebuffer,
            memory_map: memory_map_info,
            acpi_rsdp,
            cmdline_addr: cmdline_ptr as u64,
            cmdline_len,
            kernel_image_start: loaded_kernel.image_start,
            kernel_image_size: loaded_kernel.image_size,
        });

        serial_println!(
            "[BOOT] Handing off to kernel at {:#x} with BootInfo at {:#x}...",
            loaded_kernel.entry_point,
            boot_info_ptr as u64
        );

        // Populate both %rdi (SysV ABI) and %rcx (Win64 ABI) to ensure compatibility
        core::arch::asm!(
            "jmp {entry}",
            in("rdi") boot_info_ptr as u64,
            in("rcx") boot_info_ptr as u64,
            entry = in(reg) loaded_kernel.entry_point,
            options(noreturn)
        );
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}
