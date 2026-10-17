use bootinfo::{FramebufferInfo, PixelFormat};
use uefi::proto::console::gop::{GraphicsOutput, PixelFormat as UefiPixelFormat};

pub fn init_gop() -> Result<FramebufferInfo, &'static str> {
    let handle = uefi::boot::get_handle_for_protocol::<GraphicsOutput>()
        .map_err(|_| "Failed to locate GraphicsOutput protocol handle")?;

    let mut gop = uefi::boot::open_protocol_exclusive::<GraphicsOutput>(handle)
        .map_err(|_| "Failed to open GraphicsOutput protocol exclusively")?;

    let mode_info = gop.current_mode_info();
    let (width, height) = mode_info.resolution();
    let stride = mode_info.stride();
    let pixel_format = match mode_info.pixel_format() {
        UefiPixelFormat::Rgb => PixelFormat::Rgb,
        UefiPixelFormat::Bgr => PixelFormat::Bgr,
        UefiPixelFormat::Bitmask => PixelFormat::Bitmask,
        UefiPixelFormat::BltOnly => PixelFormat::BltOnly,
    };

    let mut fb = gop.frame_buffer();
    let address = fb.as_mut_ptr() as u64;
    let size = fb.size();

    Ok(FramebufferInfo {
        address,
        size,
        width,
        height,
        stride,
        bytes_per_pixel: 4,
        pixel_format,
    })
}
