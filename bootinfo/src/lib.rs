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

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct BootInfo {
    pub framebuffer: FramebufferInfo,
    pub memory_map: MemoryMapInfo,
    pub acpi_rsdp: u64,
    pub cmdline_addr: u64,
    pub cmdline_len: usize,
    pub kernel_image_start: u64,
    pub kernel_image_size: u64,
}
