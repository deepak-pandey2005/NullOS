//! Minimal Linux-style Framebuffer (`fbdev`) & Input (`evdev`) Kernel Interface.
//!
//! Following the Linux kernel model:
//! - The kernel owns **no** window manager, desktop, or UI chrome.
//! - Ring 0 only provides hardware framebuffer metadata/mapping (`SYS_FB_INFO`,
//!   `SYS_FB_MAP`, `SYS_FB_RELEASE`) and raw hardware input events (`SYS_INPUT_POLL`).
//! - The entire Desktop Compositor runs in Ring 3 (`/bin/compositor.elf`).

#![allow(dead_code)]

pub mod event;
pub mod mouse;

use bootinfo::FramebufferInfo;
use crate::memory::address::{Page, PhysAddr, PhysFrame, VirtAddr, PAGE_SIZE_4K};
use crate::memory::{paging, PageTableFlags};
use crate::sync::Spinlock;

/// ABI-stable framebuffer geometry exposed to Ring 3 via `SYS_FB_INFO` / `SYS_FB_MAP`.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FbInfo {
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub bpp: u32,
}

struct KernelFbState {
    info: FbInfo,
    hw_address: u64,
    owner_pid: Option<u64>,
}

static FB_STATE: Spinlock<Option<KernelFbState>> = Spinlock::new(None);

/// Base user virtual address where `SYS_FB_MAP` maps the linear GOP framebuffer.
pub const USER_FB_MAP_VADDR: u64 = 0x5000_0000_0000;

/// Initializes framebuffer metadata and mouse screen bounds from `BootInfo`.
pub fn init(fb: &FramebufferInfo) {
    if fb.address == 0 || fb.width == 0 || fb.height == 0 {
        return;
    }

    *FB_STATE.lock() = Some(KernelFbState {
        info: FbInfo {
            width: fb.width as u32,
            height: fb.height as u32,
            stride: fb.stride as u32,
            bpp: 32,
        },
        hw_address: fb.address,
        owner_pid: None,
    });

    let center_x = (fb.width / 2) as isize;
    let center_y = (fb.height / 2) as isize;
    let mut mouse = mouse::MOUSE.lock();
    mouse.set_screen_size(fb.width, fb.height);
    mouse.x = center_x;
    mouse.y = center_y;
}

/// Returns the hardware framebuffer geometry if available.
pub fn get_fb_info() -> Option<FbInfo> {
    FB_STATE.lock().as_ref().map(|s| s.info)
}

/// Acquires framebuffer master ownership for `pid`, suspends the kernel text console,
/// enables PS/2 mouse reporting, and maps the linear GOP framebuffer into Ring 3.
pub fn fb_map_for_user(pid: u64) -> Option<(FbInfo, u64)> {
    let (info, hw_address) = {
        let mut guard = FB_STATE.lock();
        let state = guard.as_mut()?;
        if let Some(existing) = state.owner_pid {
            if existing != pid {
                return None;
            }
        }
        state.owner_pid = Some(pid);
        (state.info, state.hw_address)
    };

    crate::drivers::console::set_suspended(true);
    crate::drivers::keyboard::enable_mouse();

    let total_bytes = (info.stride as u64) * (info.height as u64) * 4;
    let page_count = (total_bytes + PAGE_SIZE_4K - 1) / PAGE_SIZE_4K;
    let flags = PageTableFlags::PRESENT
        | PageTableFlags::WRITABLE
        | PageTableFlags::USER_ACCESSIBLE
        | PageTableFlags::NO_EXECUTE;

    for i in 0..page_count {
        let virt = VirtAddr::new(USER_FB_MAP_VADDR + i * PAGE_SIZE_4K);
        let phys = PhysAddr::new(hw_address + i * PAGE_SIZE_4K);
        let page = Page::containing_address(virt);
        let frame = PhysFrame::containing_address(phys);
        if paging::map_page(page, frame, flags).is_err() {
            fb_release(pid);
            return None;
        }
    }

    Some((info, USER_FB_MAP_VADDR))
}

/// Releases framebuffer ownership held by `pid` and restores the kernel text console.
pub fn fb_release(pid: u64) {
    let mut released = false;
    {
        let mut guard = FB_STATE.lock();
        if let Some(ref mut state) = *guard {
            if state.owner_pid == Some(pid) {
                state.owner_pid = None;
                released = true;
            }
        }
    }
    if released {
        crate::drivers::console::set_suspended(false);
        crate::drivers::console::clear_screen();
    }
}
