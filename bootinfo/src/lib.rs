#![no_std]

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PixelFormat {
    Rgb,
    Bgr,
    Bitmask,
    BltOnly,
    Unknown,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct FramebufferInfo {
    pub address: u64,
    pub size: usize,
    pub width: usize,
    pub height: usize,
    pub stride: usize,
    pub bytes_per_pixel: usize,
    pub pixel_format: PixelFormat,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct MemoryMapInfo {
    pub address: u64,
    pub size: usize,
    pub descriptor_size: usize,
    pub descriptor_version: u32,
}

/// Unified bootloader-to-kernel handoff contract populated by UEFI before jumping to `kernel_main`.
///
/// The kernel receives a pointer to an instance of `BootInfo` in `%rdi` (System V AMD64) /
/// `%rcx` (Microsoft x64) / `x0` (AArch64 AAPCS64). All buffer addresses are physical addresses
/// identity-mapped initially by firmware and preserved in kernel page tables.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct BootInfo {
    /// Linear GOP framebuffer descriptor discovered by UEFI firmware.
    pub framebuffer: FramebufferInfo,
    /// UEFI physical memory map descriptor snapshot taken at `ExitBootServices`.
    pub memory_map: MemoryMapInfo,
    /// Physical address of ACPI 2.0 XSDT-capable RSDP (or ACPI 1.0 fallback), or `0` if absent.
    pub acpi_rsdp: u64,
    /// Physical address of UTF-8 kernel command line parsed from `nullos.cfg`, or `0` if empty.
    pub cmdline_addr: u64,
    /// Byte length of the kernel command line buffer.
    pub cmdline_len: usize,
    /// Lowest physical load address of the kernel ELF image (`PT_LOAD` minimum).
    pub kernel_image_start: u64,
    /// Total contiguous span of the loaded kernel image in bytes.
    pub kernel_image_size: u64,
    /// Physical address of the loaded POSIX USTAR `initramfs.tar` buffer, or `0` if none provided.
    pub initramfs_addr: u64,
    /// Byte length of the `initramfs.tar` image in memory.
    pub initramfs_size: usize,
}
