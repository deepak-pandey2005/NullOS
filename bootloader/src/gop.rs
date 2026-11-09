//! UEFI Graphics Output Protocol (GOP) Framebuffer Initialization.
//!
//! # Overview
//! The UEFI Graphics Output Protocol (GOP) provides hardware-independent access
//! to the system's display controller. Prior to UEFI, x86 operating systems relied
//! on 16-bit BIOS Interrupt `0x10` (VESA VBE) to set screen resolutions, which
//! required dropping out of Protected/Long Mode or running real-mode thunks.
//!
//! GOP eliminates this legacy limitation by exposing a linear 32-bit physical
//! memory framebuffer directly to Long Mode code. Once configured, writing color
//! values into this memory range immediately illuminates pixels on the screen.

use bootinfo::{FramebufferInfo, PixelFormat};
use uefi::proto::console::gop::{GraphicsOutput, PixelFormat as UefiPixelFormat};

/// Initializes the Graphics Output Protocol (GOP) and extracts linear framebuffer metrics.
///
/// # Hardware Mechanics
/// 1. Locates the active display adapter handle implementing `GraphicsOutput`.
/// 2. Opens the protocol exclusively to prevent concurrent firmware interference.
/// 3. Queries `current_mode_info()` to obtain active screen dimensions:
///    - **Horizontal Resolution** (`width` in pixels)
///    - **Vertical Resolution** (`height` in scanlines)
///    - **Stride / Pitch**: Number of pixels per physical scanline (may include padding
///      for GPU memory alignment, e.g. 1280 pixels stride for 1280 width).
/// 4. Maps UEFI pixel format (`Rgb`, `Bgr`, `Bitmask`) into the OS-independent
///    `bootinfo::PixelFormat` enum (standard x86 GPUs commonly utilize 32-bit BGR).
/// 5. Obtains the base physical memory address and total byte size of the video RAM.
///
/// # Errors
/// Returns `Err(&'static str)` if no GOP display device is present or if the firmware
/// fails to open the protocol exclusively.
pub fn init_gop() -> Result<FramebufferInfo, &'static str> {
    // Locate the first device handle supporting the Graphics Output Protocol
    let handle = uefi::boot::get_handle_for_protocol::<GraphicsOutput>()
        .map_err(|_| "Failed to locate GraphicsOutput protocol handle")?;

    // Open the protocol exclusively to configure and extract framebuffer metadata
    let mut gop = uefi::boot::open_protocol_exclusive::<GraphicsOutput>(handle)
        .map_err(|_| "Failed to open GraphicsOutput protocol exclusively")?;

    // Query active video mode metrics (resolution, stride, color format)
    let mode_info = gop.current_mode_info();
    let (width, height) = mode_info.resolution();
    let stride = mode_info.stride();

    // Map UEFI color ordering to the kernel's bootinfo representation
    let pixel_format = match mode_info.pixel_format() {
        UefiPixelFormat::Rgb => PixelFormat::Rgb,
        UefiPixelFormat::Bgr => PixelFormat::Bgr,
        UefiPixelFormat::Bitmask => PixelFormat::Bitmask,
        UefiPixelFormat::BltOnly => PixelFormat::BltOnly,
    };

    // Obtain raw pointer to the linear video memory buffer and total capacity
    let mut fb = gop.frame_buffer();
    let address = fb.as_mut_ptr() as u64;
    let size = fb.size();

    Ok(FramebufferInfo {
        address,
        size,
        width,
        height,
        stride,
        bytes_per_pixel: 4, // 32-bit color depth (4 bytes per pixel)
        pixel_format,
    })
}
