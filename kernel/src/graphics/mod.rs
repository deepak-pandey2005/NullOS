//! Framebuffer, 2D Graphics Canvas & Desktop Window Manager (Phase 12 - M30).
//!
//! Subsystem features:
//! - Double-buffered 2D graphics canvas with fast offscreen tear-free swapping.
//! - Basic graphics primitives: Bresenham lines, rectangles, midpoint circles, and alpha blending.
//! - Anti-aliased font and text rendering via noto-sans-mono-bitmap.
//! - Input event routing (keyboard and mouse events).
//! - Window abstraction with decorated title bars, close buttons, and client surfaces.
//! - Desktop Window Manager & Compositor with wallpaper, taskbar, and Z-ordering.

#![allow(dead_code)]

pub mod canvas;
pub mod color;
pub mod event;
pub mod mouse;
pub mod window;
pub mod wm;

use bootinfo::FramebufferInfo;
use self::color::Color;
use self::wm::{WindowManager, WINDOW_MANAGER};

/// Initializes the 2D graphics engine and Desktop Window Manager.
pub fn init(fb: &FramebufferInfo) {
    if fb.address == 0 || fb.width == 0 || fb.height == 0 {
        return;
    }

    let wm = WindowManager::new(fb.width, fb.height, fb.stride, fb.address);
    *WINDOW_MANAGER.lock() = Some(wm);

    // Initialize mouse position to center of screen
    let center_x = (fb.width / 2) as isize;
    let center_y = (fb.height / 2) as isize;
    let mut mouse = mouse::MOUSE.lock();
    mouse.x = center_x;
    mouse.y = center_y;
}

/// Spawns standard desktop demonstration windows.
pub fn launch_demo() {
    let mut wm_lock = WINDOW_MANAGER.lock();
    if let Some(ref mut wm) = *wm_lock {
        // Window 1: System Monitor
        let win1_id = wm.create_window("System Monitor", 50, 50, 320, 200);
        if let Some(win) = wm.windows.iter_mut().find(|w| w.id == win1_id) {
            win.surface.clear(Color::PANEL_BG);
            win.surface.draw_string(12, 12, "NullOS 64-Bit Desktop", Color::TITLE_ACTIVE, None);
            win.surface.draw_line(12, 32, 308, 32, Color::BORDER);

            let ticks = crate::arch::x86_64::apic::get_ticks();
            let cpus = crate::arch::x86_64::smp::online_cpu_count();
            let (free_mem, total_mem) = if let Some(s) = crate::memory::stats() {
                (s.free_bytes, s.total_bytes)
            } else {
                (0, 0)
            };

            win.surface.draw_string(12, 44, &alloc::format!("CPU Cores:    {}", cpus), Color::LIGHT_SLATE, None);
            win.surface.draw_string(12, 64, &alloc::format!("APIC Ticks:   {}", ticks), Color::LIGHT_SLATE, None);
            win.surface.draw_string(12, 84, &alloc::format!("Free Memory:  {} MiB", free_mem / (1024 * 1024)), Color::GREEN, None);
            win.surface.draw_string(12, 104, &alloc::format!("Total RAM:    {} MiB", total_mem / (1024 * 1024)), Color::LIGHT_SLATE, None);
            win.surface.draw_string(12, 130, "Double-Buffering: Enabled", Color::CYAN, None);
            win.surface.draw_string(12, 150, "Compositor:       Active", Color::YELLOW, None);
        }

        // Window 2: 2D Graphics Demo
        let win2_id = wm.create_window("Graphics Demo", 410, 80, 360, 240);
        if let Some(win) = wm.windows.iter_mut().find(|w| w.id == win2_id) {
            win.surface.clear(Color::DARK_SLATE);
            win.surface.draw_string(12, 12, "2D Primitives Showcase", Color::WHITE, None);

            // Bresenham lines
            win.surface.draw_line(20, 40, 160, 100, Color::RED);
            win.surface.draw_line(160, 40, 20, 100, Color::BLUE);
            win.surface.draw_line(20, 70, 160, 70, Color::GREEN);

            // Outlined and filled rectangles
            win.surface.draw_rect(190, 40, 70, 60, Color::YELLOW);
            win.surface.fill_rect(275, 40, 70, 60, Color::MAGENTA);

            // Midpoint circles
            win.surface.draw_circle(90, 170, 40, Color::CYAN);
            win.surface.fill_circle(240, 170, 35, Color::rgba(34, 197, 94, 200));

            win.surface.draw_string(12, 218, "Lines, Rects, Circles & Alpha", Color::MUTED_TEXT, None);
        }

        // Window 3: Terminal Console
        let win3_id = wm.create_window("Terminal", 150, 280, 420, 200);
        if let Some(win) = wm.windows.iter_mut().find(|w| w.id == win3_id) {
            win.surface.clear(Color::BLACK);
            win.surface.draw_string(8, 8, "NullOS Interactive Console (v0.1.0)", Color::GREEN, None);
            win.surface.draw_string(8, 28, "nullos$ help", Color::LIGHT_SLATE, None);
            win.surface.draw_string(8, 48, "Available: gui, ls, cat, mem, ps, exec", Color::MUTED_TEXT, None);
            win.surface.draw_string(8, 68, "nullos$ gui --status", Color::LIGHT_SLATE, None);
            win.surface.draw_string(8, 88, "[GUI] Desktop compositor running at 60 FPS", Color::YELLOW, None);
            win.surface.draw_string(8, 108, "nullos$ _", Color::WHITE, None);
        }

        wm.render_desktop();
    }
}
