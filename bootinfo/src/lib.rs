#![no_std]

#[repr(C)]
pub struct BootInfo {
    pub framebuffer: FramebufferInfo,
    pub memory_map: MemoryMapInfo,
    pub acpi_rsdp: u64,
}
#[repr(C)]
pub struct FramebufferInfo {
    pub address: u64,
    pub size: usize,
    pub width: usize,
    pub height: usize,
    pub stride: usize,
}

#[repr(C)]
pub struct MemoryMapInfo {
    pub address: u64,
    pub size: usize,
    pub descriptor_size: usize,
}
