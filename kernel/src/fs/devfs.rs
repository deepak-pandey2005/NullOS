//! Standard Linux Character Device Nodes (/dev/fb0, /dev/input/event0, etc.)
//!
//! Enables standard Linux GUI compositors (Wayland / cage / tinywl / Xorg fbdev)
//! and input stacks to query screen geometry and receive event updates.

#![allow(dead_code)]

use alloc::sync::Arc;
use alloc::vec::Vec;
use crate::fs::vfs::{DirEntry, FileStat, FileType, FsError, Inode, POLLIN, POLLOUT};
use crate::graphics;
use crate::graphics::event;

/// Linux fbdev ioctl commands
pub const FBIOGET_VSCREENINFO: u32 = 0x4600;
pub const FBIOPUT_VSCREENINFO: u32 = 0x4601;
pub const FBIOGET_FSCREENINFO: u32 = 0x4602;

/// Linux terminal ioctls
pub const TCGETS: u32 = 0x5401;
pub const TCSETS: u32 = 0x5402;
pub const TIOCGWINSZ: u32 = 0x5413;

/// Linux struct fb_var_screeninfo (simplified ABI for userspace fbdev)
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct FbVarScreeninfo {
    pub xres: u32,
    pub yres: u32,
    pub xres_virtual: u32,
    pub yres_virtual: u32,
    pub xoffset: u32,
    pub yoffset: u32,
    pub bits_per_pixel: u32,
    pub grayscale: u32,
    /* red, green, blue bitfield offsets and lengths */
    pub red_offset: u32,
    pub red_length: u32,
    pub red_msb_right: u32,
    pub green_offset: u32,
    pub green_length: u32,
    pub green_msb_right: u32,
    pub blue_offset: u32,
    pub blue_length: u32,
    pub blue_msb_right: u32,
    pub transp_offset: u32,
    pub transp_length: u32,
    pub transp_msb_right: u32,
}

/// Linux struct fb_fix_screeninfo
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct FbFixScreeninfo {
    pub id: [u8; 16],
    pub smem_start: u64,
    pub smem_len: u32,
    pub fb_type: u32,
    pub type_aux: u32,
    pub visual: u32,
    pub xpanstep: u16,
    pub ypanstep: u16,
    pub ywrapstep: u16,
    pub line_length: u32,
    pub mmio_start: u64,
    pub mmio_len: u32,
    pub accel: u32,
    pub capabilities: u16,
    pub reserved: [u16; 2],
}

/// Linux struct winsize (TIOCGWINSZ)
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct WinSize {
    pub ws_row: u16,
    pub ws_col: u16,
    pub ws_xpixel: u16,
    pub ws_ypixel: u16,
}

/// Linux struct input_event for evdev (/dev/input/event0)
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct LinuxInputEvent {
    pub time_sec: u64,
    pub time_usec: u64,
    pub event_type: u16,
    pub code: u16,
    pub value: i32,
}

pub const EV_SYN: u16 = 0x00;
pub const EV_KEY: u16 = 0x01;
pub const EV_REL: u16 = 0x02;
pub const EV_ABS: u16 = 0x03;

pub const REL_X: u16 = 0x00;
pub const REL_Y: u16 = 0x01;
pub const BTN_LEFT: u16 = 0x110;
pub const BTN_RIGHT: u16 = 0x111;
pub const BTN_MIDDLE: u16 = 0x112;

/// Character device representing /dev/fb0
pub struct FbDevice {
    inode_num: u64,
}

impl FbDevice {
    pub fn new(inode_num: u64) -> Self {
        Self { inode_num }
    }
}

impl Inode for FbDevice {
    fn stat(&self) -> FileStat {
        let size = if let Some(info) = graphics::get_fb_info() {
            (info.stride * info.height * (info.bpp / 8)) as usize
        } else {
            0
        };
        FileStat {
            inode_num: self.inode_num,
            file_type: FileType::CharDevice,
            size,
            mode: 0o660,
        }
    }

    fn read_at(&self, offset: usize, buf: &mut [u8]) -> Result<usize, FsError> {
        let fb_info = graphics::get_fb_info().ok_or(FsError::IoError)?;
        let total_size = (fb_info.stride * fb_info.height * (fb_info.bpp / 8)) as usize;
        if offset >= total_size {
            return Ok(0);
        }
        let available = (total_size - offset).min(buf.len());
        // Framebuffer read
        Ok(available)
    }

    fn write_at(&self, offset: usize, buf: &[u8]) -> Result<usize, FsError> {
        let fb_info = graphics::get_fb_info().ok_or(FsError::IoError)?;
        let total_size = (fb_info.stride * fb_info.height * (fb_info.bpp / 8)) as usize;
        if offset >= total_size {
            return Ok(0);
        }
        let to_write = (total_size - offset).min(buf.len());
        Ok(to_write)
    }

    fn ioctl(&self, cmd: u32, arg: usize) -> Result<i32, FsError> {
        let fb_info = graphics::get_fb_info().ok_or(FsError::IoError)?;
        match cmd {
            FBIOGET_VSCREENINFO => {
                if arg == 0 {
                    return Err(FsError::InvalidInput);
                }
                let var = unsafe { &mut *(arg as *mut FbVarScreeninfo) };
                var.xres = fb_info.width;
                var.yres = fb_info.height;
                var.xres_virtual = fb_info.stride;
                var.yres_virtual = fb_info.height;
                var.xoffset = 0;
                var.yoffset = 0;
                var.bits_per_pixel = fb_info.bpp;
                var.red_offset = 16;
                var.red_length = 8;
                var.green_offset = 8;
                var.green_length = 8;
                var.blue_offset = 0;
                var.blue_length = 8;
                var.transp_offset = 24;
                var.transp_length = 8;
                Ok(0)
            }
            FBIOGET_FSCREENINFO => {
                if arg == 0 {
                    return Err(FsError::InvalidInput);
                }
                let fix = unsafe { &mut *(arg as *mut FbFixScreeninfo) };
                let mut id = [0u8; 16];
                let name = b"NullOS-FB\0";
                id[..name.len()].copy_from_slice(name);
                fix.id = id;
                fix.smem_start = graphics::USER_FB_MAP_VADDR;
                fix.smem_len = fb_info.stride * fb_info.height * (fb_info.bpp / 8);
                fix.line_length = fb_info.stride * (fb_info.bpp / 8);
                fix.fb_type = 0; // FB_TYPE_PACKED_PIXELS
                fix.visual = 2;  // FB_VISUAL_TRUECOLOR
                Ok(0)
            }
            FBIOPUT_VSCREENINFO => Ok(0),
            _ => Err(FsError::Unsupported),
        }
    }

    fn lookup(&self, _name: &str) -> Result<Arc<dyn Inode>, FsError> {
        Err(FsError::NotADirectory)
    }

    fn read_dir(&self) -> Result<Vec<DirEntry>, FsError> {
        Err(FsError::NotADirectory)
    }

    fn create(&self, _name: &str, _file_type: FileType) -> Result<Arc<dyn Inode>, FsError> {
        Err(FsError::NotADirectory)
    }
}

/// Character device representing /dev/input/event0 (evdev)
pub struct EvdevDevice {
    inode_num: u64,
}

impl EvdevDevice {
    pub fn new(inode_num: u64) -> Self {
        Self { inode_num }
    }
}

impl Inode for EvdevDevice {
    fn stat(&self) -> FileStat {
        FileStat {
            inode_num: self.inode_num,
            file_type: FileType::CharDevice,
            size: 0,
            mode: 0o660,
        }
    }

    fn read_at(&self, _offset: usize, buf: &mut [u8]) -> Result<usize, FsError> {
        let ev_size = core::mem::size_of::<LinuxInputEvent>();
        if buf.len() < ev_size {
            return Err(FsError::InvalidInput);
        }

        if let Some(raw) = event::poll_raw_input() {
            let mut lev = LinuxInputEvent::default();
            lev.time_sec = crate::arch::x86_64::apic::get_ticks() / 1000;
            lev.time_usec = (crate::arch::x86_64::apic::get_ticks() % 1000) * 1000;

            match raw.kind {
                event::RAW_INPUT_MOUSE_MOVE => {
                    lev.event_type = EV_REL;
                    lev.code = REL_X;
                    lev.value = raw.dx;
                }
                event::RAW_INPUT_MOUSE_DOWN | event::RAW_INPUT_MOUSE_UP => {
                    lev.event_type = EV_KEY;
                    lev.code = match raw.button {
                        1 => BTN_RIGHT,
                        2 => BTN_MIDDLE,
                        _ => BTN_LEFT,
                    };
                    lev.value = if raw.pressed == 1 { 1 } else { 0 };
                }
                event::RAW_INPUT_KEY => {
                    lev.event_type = EV_KEY;
                    lev.code = raw.key as u16;
                    lev.value = if raw.pressed == 1 { 1 } else { 0 };
                }
                _ => return Err(FsError::WouldBlock),
            }

            let slice = unsafe {
                core::slice::from_raw_parts(
                    (&lev as *const LinuxInputEvent) as *const u8,
                    ev_size,
                )
            };
            buf[..ev_size].copy_from_slice(slice);
            Ok(ev_size)
        } else {
            Err(FsError::WouldBlock)
        }
    }

    fn write_at(&self, _offset: usize, _buf: &[u8]) -> Result<usize, FsError> {
        Ok(0)
    }

    fn poll(&self) -> u32 {
        if crate::drivers::keyboard::has_data() || crate::drivers::serial::has_byte() {
            POLLIN | POLLOUT
        } else {
            POLLOUT
        }
    }

    fn lookup(&self, _name: &str) -> Result<Arc<dyn Inode>, FsError> {
        Err(FsError::NotADirectory)
    }

    fn read_dir(&self) -> Result<Vec<DirEntry>, FsError> {
        Err(FsError::NotADirectory)
    }

    fn create(&self, _name: &str, _file_type: FileType) -> Result<Arc<dyn Inode>, FsError> {
        Err(FsError::NotADirectory)
    }
}
