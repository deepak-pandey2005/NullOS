# Shared Boot Information Subsystem (`bootinfo/`)

The `bootinfo` crate is a minimal, `#![no_std]` zero-dependency crate that defines the **Application Binary Interface (ABI) contract** between the UEFI Bootloader (`bootloader/`) and the NullOS Kernel (`kernel/`).

---

## Data Structures

All structures are marked with `#[repr(C)]` to ensure C-compatible layout and explicit alignment across crate boundaries.

### `BootInfo`

```rust
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
```

- **`framebuffer`**: Details about the GOP linear frame buffer.
- **`memory_map`**: Address, size, and descriptor layout of the physical memory map.
- **`acpi_rsdp`**: 64-bit physical pointer to the ACPI RSDP table (or `0` if absent).
- **`cmdline_addr`**: 64-bit physical address of the kernel command line ASCII string.
- **`cmdline_len`**: Length in bytes of the command line string.
- **`kernel_image_start`**: Starting physical address where `kernel.elf` segments were loaded (`0x100000`).
- **`kernel_image_size`**: Total memory footprint of loaded kernel segments.

---

### `FramebufferInfo`

```rust
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
```

- **`address`**: 64-bit physical address of the linear display memory.
- **`size`**: Total frame buffer size in bytes (`stride * height * bytes_per_pixel`).
- **`width`**: Horizontal display resolution in pixels (e.g. 1280).
- **`height`**: Vertical display resolution in pixels (e.g. 800).
- **`stride`**: Pixels per scanline (may include off-screen padding).
- **`bytes_per_pixel`**: Typically `4` for 32-bit color depths.
- **`pixel_format`**: `PixelFormat::Rgb`, `PixelFormat::Bgr`, `Bitmask`, `BltOnly`, or `Unknown`.

---

### `MemoryMapInfo`

```rust
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct MemoryMapInfo {
    pub address: u64,
    pub size: usize,
    pub descriptor_size: usize,
    pub descriptor_version: u32,
}
```

- **`address`**: Physical pointer to array of UEFI memory descriptors copied before `ExitBootServices`.
- **`size`**: Total byte length of memory map descriptor array.
- **`descriptor_size`**: Byte size of each individual UEFI `MemoryDescriptor` (typically 48 bytes).
- **`descriptor_version`**: UEFI memory descriptor version identifier (typically `1`).

---

## Memory Safety Rules

1. **Persistence**: All pointers inside `BootInfo` point to memory allocated in `LOADER_DATA` pool before `ExitBootServices` to guarantee they remain valid after UEFI termination.
2. **Read-Only Ownership**: The kernel receives `&'static BootInfo`. The kernel treats `BootInfo` metadata as read-only.
3. **No Direct Heap Dependencies**: `bootinfo` operates strictly without heap allocations (`no_std`) and without dynamic runtime dependencies.
