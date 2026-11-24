//! Boot Protocol Dispatcher and Compatibility Layer.
//!
//! Handles multiple bootloader standards:
//! 1. Native NullOS UEFI BootInfo handoff.
//! 2. Limine Boot Protocol v2 (framebuffer, memory map, RSDP, HHDM).
//! 3. Multiboot2 (GRUB / QEMU direct kernel boot).

use bootinfo::{BootInfo, FramebufferInfo, MemoryMapInfo, PixelFormat};

/// Multiboot2 magic number passed in %eax.
pub const MULTIBOOT2_MAGIC: u64 = 0x36d76289;

// Limine request markers exported from assembly (or declared here)
unsafe extern "C" {
    static limine_fb_request: [u64; 4];
    static limine_mmap_request: [u64; 4];
    static limine_rsdp_request: [u64; 4];
}

/// Statically allocated fallback BootInfo for external bootloaders (Limine, Multiboot2).
static mut FALLBACK_BOOTINFO: BootInfo = BootInfo {
    framebuffer: FramebufferInfo {
        address: 0,
        size: 0,
        width: 0,
        height: 0,
        stride: 0,
        bytes_per_pixel: 4,
        pixel_format: PixelFormat::Rgb,
    },
    memory_map: MemoryMapInfo {
        address: 0,
        size: 0,
        descriptor_size: 48,
        descriptor_version: 1,
    },
    acpi_rsdp: 0,
    cmdline_addr: 0,
    cmdline_len: 0,
    kernel_image_start: 0x100000,
    kernel_image_size: 0x400000,
    initramfs_addr: 0,
    initramfs_size: 0,
};

/// Universal entry dispatcher called from `entry.S`.
///
/// # Arguments
/// - `rdi`: First SysV AMD64 argument (points to `BootInfo` in NullOS UEFI bootloader).
/// - `rsi`: Second argument.
/// - `rdx`: Original `%rax` value on entry (contains `MULTIBOOT2_MAGIC` if booted via GRUB).
/// - `rcx`: Original `%rbx` value on entry (contains Multiboot2 info pointer).
#[unsafe(no_mangle)]
pub extern "C" fn boot_dispatcher(rdi: u64, _rsi: u64, rdx: u64, rcx: u64) -> ! {
    // Check 1: Multiboot2 magic in %rax (passed as rdx)
    if (rdx & 0xFFFFFFFF) == (MULTIBOOT2_MAGIC & 0xFFFFFFFF) {
        let mbi_ptr = rcx;
        let boot_info = parse_multiboot2(mbi_ptr);
        crate::kernel_main(boot_info);
    }

    // Check 2: Limine protocol response
    if let Some(boot_info) = check_limine_boot() {
        crate::kernel_main(boot_info);
    }

    // Check 3: Native NullOS UEFI Bootloader (BootInfo pointer in %rdi)
    if rdi != 0 {
        let boot_info = unsafe { &*(rdi as *const BootInfo) };
        crate::kernel_main(boot_info);
    }

    // Fallback: safe default BootInfo if no metadata could be determined
    let boot_info = unsafe { &*(&raw const FALLBACK_BOOTINFO) };
    crate::kernel_main(boot_info);
}

/// Checks if Limine populated the response structures.
fn check_limine_boot() -> Option<&'static BootInfo> {
    unsafe {
        let fb_resp = limine_fb_request[2];
        let mmap_resp = limine_mmap_request[2];
        let rsdp_resp = limine_rsdp_request[2];

        if fb_resp == 0 && mmap_resp == 0 {
            return None;
        }

        let bi = &raw mut FALLBACK_BOOTINFO;

        // Parse Limine Framebuffer
        if fb_resp != 0 {
            let fb_count = *(fb_resp.wrapping_add(8) as *const u64);
            if fb_count > 0 {
                let fb_array_ptr = *(fb_resp.wrapping_add(16) as *const u64) as *const *const u64;
                if !fb_array_ptr.is_null() {
                    let fb0 = *fb_array_ptr;
                    let addr = *fb0;
                    let width = *fb0.add(1);
                    let height = *fb0.add(2);
                    let pitch = *fb0.add(3);
                    let bpp = *fb0.add(4) as u16;

                    (*bi).framebuffer.address = addr;
                    (*bi).framebuffer.width = width as usize;
                    (*bi).framebuffer.height = height as usize;
                    (*bi).framebuffer.stride = (pitch / 4) as usize;
                    (*bi).framebuffer.bytes_per_pixel = (bpp / 8) as usize;
                    (*bi).framebuffer.size = (pitch * height) as usize;
                    (*bi).framebuffer.pixel_format = PixelFormat::Rgb;
                }
            }
        }

        // Parse Limine RSDP
        if rsdp_resp != 0 {
            let rsdp_addr = *(rsdp_resp.wrapping_add(8) as *const u64);
            (*bi).acpi_rsdp = rsdp_addr;
        }

        Some(&*(&raw const FALLBACK_BOOTINFO))
    }
}

/// Minimal parser for Multiboot2 info tags (framebuffer, memory map, RSDP).
fn parse_multiboot2(mbi_addr: u64) -> &'static BootInfo {
    unsafe {
        let bi = &raw mut FALLBACK_BOOTINFO;
        if mbi_addr == 0 {
            return &*(&raw const FALLBACK_BOOTINFO);
        }

        let total_size = *(mbi_addr as *const u32);
        let mut offset = 8usize; // tags start at offset 8

        while offset < total_size as usize {
            let tag_ptr = (mbi_addr as usize + offset) as *const u32;
            let tag_type = *tag_ptr;
            let tag_size = *tag_ptr.add(1) as usize;

            if tag_type == 0 {
                // End tag
                break;
            }

            match tag_type {
                // Multiboot2 Tag 8: Framebuffer info
                8 => {
                    let addr = *(tag_ptr.add(2) as *const u64);
                    let pitch = *tag_ptr.add(4);
                    let width = *tag_ptr.add(5);
                    let height = *tag_ptr.add(6);
                    let bpp = *(tag_ptr.add(7) as *const u8);

                    (*bi).framebuffer.address = addr;
                    (*bi).framebuffer.width = width as usize;
                    (*bi).framebuffer.height = height as usize;
                    (*bi).framebuffer.stride = (pitch / 4) as usize;
                    (*bi).framebuffer.bytes_per_pixel = (bpp / 8) as usize;
                    (*bi).framebuffer.size = (pitch as usize) * (height as usize);
                    (*bi).framebuffer.pixel_format = PixelFormat::Rgb;
                }
                // Multiboot2 Tag 14 or 15: ACPI RSDP (new/old)
                14 | 15 => {
                    let rsdp_ptr = tag_ptr.add(2) as *const u8;
                    (*bi).acpi_rsdp = rsdp_ptr as u64;
                }
                // Multiboot2 Tag 1: Boot command line
                1 => {
                    let cmdline_ptr = tag_ptr.add(2) as *const u8;
                    let len = tag_size.saturating_sub(8);
                    (*bi).cmdline_addr = cmdline_ptr as u64;
                    (*bi).cmdline_len = len;
                }
                _ => {}
            }

            // Tags are 8-byte aligned
            offset += (tag_size + 7) & !7;
        }

        &*(&raw const FALLBACK_BOOTINFO)
    }
}
