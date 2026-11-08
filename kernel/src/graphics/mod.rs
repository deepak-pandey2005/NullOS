//! Framebuffer, 2D Graphics Canvas & Desktop Window Manager (Phase 12 - M30).
//!
//! Subsystem features:
//! - Double-buffered 2D graphics canvas with fast offscreen tear-free swapping.
//! - Basic graphics primitives: Bresenham lines, rectangles, midpoint circles, and alpha blending.
//! - Anti-aliased font and text rendering via noto-sans-mono-bitmap.
//! - Input event routing (keyboard and mouse events).
//! - Window abstraction with decorated title bars, close buttons, and client surfaces.
//! - Desktop Window Manager & Compositor with wallpaper, taskbar, and Z-ordering.
//! - Exclusive GUI compositor mode with dedicated event loop and dmesg log isolation.

#![allow(dead_code)]

pub mod canvas;
pub mod color;
pub mod event;
pub mod mouse;
pub mod window;
pub mod wm;

use alloc::string::ToString;
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
    mouse.set_screen_size(fb.width, fb.height);
    mouse.x = center_x;
    mouse.y = center_y;
}

/// Formats a string with a label, number, and suffix into a stack buffer without dynamic allocation.
fn format_stat<'a>(buf: &'a mut [u8], label: &str, val: u64, suffix: &str) -> &'a str {
    let l_bytes = label.as_bytes();
    buf[..l_bytes.len()].copy_from_slice(l_bytes);
    let mut pos = l_bytes.len();
    let mut n = val;
    if n == 0 {
        buf[pos] = b'0';
        pos += 1;
    } else {
        let start = pos;
        while n > 0 {
            buf[pos] = b'0' + (n % 10) as u8;
            pos += 1;
            n /= 10;
        }
        buf[start..pos].reverse();
    }
    let s_bytes = suffix.as_bytes();
    buf[pos..pos + s_bytes.len()].copy_from_slice(s_bytes);
    pos += s_bytes.len();
    core::str::from_utf8(&buf[..pos]).unwrap_or("")
}

/// Spawns standard desktop demonstration windows.
pub fn launch_demo() {
    let mut wm_lock = WINDOW_MANAGER.lock();
    if let Some(ref mut wm) = *wm_lock {
        // Clear any previous windows to start fresh
        wm.windows.clear();

        // Window 1: System Monitor
        let win1_id = wm.create_window("System Monitor", 30, 30, 340, 230);
        if let Some(win) = wm.windows.iter_mut().find(|w| w.id == win1_id) {
            win.surface.clear(Color::PANEL_BG);
            win.surface.draw_string(12, 12, "NullOS 64-Bit Desktop", Color::TITLE_ACTIVE, None);
            win.surface.draw_line(12, 32, 328, 32, Color::BORDER);

            let ticks = crate::arch::x86_64::apic::get_ticks();
            let cpus = crate::arch::x86_64::smp::online_cpu_count();
            let (free_mem, total_mem) = if let Some(s) = crate::memory::stats() {
                (s.free_bytes, s.total_bytes)
            } else {
                (0, 0)
            };

            let mut b1 = [0u8; 48];
            win.surface.draw_string(12, 44, format_stat(&mut b1, "CPU Cores:    ", cpus as u64, ""), Color::LIGHT_SLATE, None);
            let mut b2 = [0u8; 48];
            win.surface.draw_string(12, 64, format_stat(&mut b2, "APIC Ticks:   ", ticks, ""), Color::LIGHT_SLATE, None);
            let mut b3 = [0u8; 48];
            win.surface.draw_string(12, 84, format_stat(&mut b3, "Free Memory:  ", free_mem / (1024 * 1024), " MiB"), Color::GREEN, None);
            let mut b4 = [0u8; 48];
            win.surface.draw_string(12, 104, format_stat(&mut b4, "Total RAM:    ", total_mem / (1024 * 1024), " MiB"), Color::LIGHT_SLATE, None);
            win.surface.draw_string(12, 128, "Compositor:   Exclusive GUI Mode", Color::YELLOW, None);
            win.surface.draw_string(12, 148, "Double-Buffer: Enabled (60 FPS)", Color::CYAN, None);
            win.surface.draw_string(12, 175, "Tip: Type 'exit' or click [Exit GUI]", Color::MUTED_TEXT, None);
            win.surface.draw_string(12, 195, "     Press [ESC] to return to CLI", Color::MUTED_TEXT, None);
        }

        // Window 2: Kernel Log (dmesg viewer)
        let win2_id = wm.create_window("Kernel Log (dmesg)", 390, 30, 500, 240);
        if let Some(win) = wm.windows.iter_mut().find(|w| w.id == win2_id) {
            let logs = crate::drivers::console::get_dmesg_lines(14);
            win.render_log_surface(&logs);
        }

        // Window 3: Terminal Console (Interactive)
        let win3_id = wm.create_window("Terminal Console", 80, 290, 520, 220);
        if let Some(win) = wm.windows.iter_mut().find(|w| w.id == win3_id) {
            win.term_lines.push("NullOS Interactive Desktop Terminal (v0.2.0)".to_string());
            win.term_lines.push("Type commands: help, dmesg, mem, uptime, clear, exit".to_string());
            win.term_lines.push("".to_string());
            win.render_terminal_surface();
            win.is_focused = true;
        }

        // Window 4: 2D Graphics Demo
        let win4_id = wm.create_window("2D Graphics Demo", 620, 290, 310, 220);
        if let Some(win) = wm.windows.iter_mut().find(|w| w.id == win4_id) {
            win.surface.clear(Color::DARK_SLATE);
            win.surface.draw_string(10, 10, "Bresenham & Midpoint Demo", Color::WHITE, None);

            // Lines
            win.surface.draw_line(15, 35, 130, 85, Color::RED);
            win.surface.draw_line(130, 35, 15, 85, Color::BLUE);
            win.surface.draw_line(15, 60, 130, 60, Color::GREEN);

            // Rectangles
            win.surface.draw_rect(155, 35, 60, 50, Color::YELLOW);
            win.surface.fill_rect(225, 35, 65, 50, Color::MAGENTA);

            // Circles
            win.surface.draw_circle(75, 145, 35, Color::CYAN);
            win.surface.fill_circle(200, 145, 30, Color::rgba(34, 197, 94, 200));

            win.surface.draw_string(10, 195, "Alpha Blending Active", Color::MUTED_TEXT, None);
        }

        wm.focused_window = Some(win3_id);
        wm.render_desktop();
    }
}

/// Dynamically updates stats in the System Monitor window.
pub fn update_system_monitor() {
    let mut wm_lock = WINDOW_MANAGER.lock();
    if let Some(ref mut wm) = *wm_lock {
        if let Some(win) = wm.windows.iter_mut().find(|w| w.title.starts_with("System Monitor")) {
            let ticks = crate::arch::x86_64::apic::get_ticks();
            let cpus = crate::arch::x86_64::smp::online_cpu_count();
            let (free_mem, total_mem) = if let Some(s) = crate::memory::stats() {
                (s.free_bytes, s.total_bytes)
            } else {
                (0, 0)
            };

            win.surface.clear(Color::PANEL_BG);
            win.surface.draw_string(12, 12, "NullOS 64-Bit Desktop", Color::TITLE_ACTIVE, None);
            win.surface.draw_line(12, 32, 328, 32, Color::BORDER);
            let mut b1 = [0u8; 48];
            win.surface.draw_string(12, 44, format_stat(&mut b1, "CPU Cores:    ", cpus as u64, ""), Color::LIGHT_SLATE, None);
            let mut b2 = [0u8; 48];
            win.surface.draw_string(12, 64, format_stat(&mut b2, "APIC Ticks:   ", ticks, ""), Color::LIGHT_SLATE, None);
            let mut b3 = [0u8; 48];
            win.surface.draw_string(12, 84, format_stat(&mut b3, "Free Memory:  ", free_mem / (1024 * 1024), " MiB"), Color::GREEN, None);
            let mut b4 = [0u8; 48];
            win.surface.draw_string(12, 104, format_stat(&mut b4, "Total RAM:    ", total_mem / (1024 * 1024), " MiB"), Color::LIGHT_SLATE, None);
            win.surface.draw_string(12, 128, "Compositor:   Exclusive GUI Mode", Color::YELLOW, None);
            win.surface.draw_string(12, 148, "Double-Buffer: Enabled (60 FPS)", Color::CYAN, None);
            win.surface.draw_string(12, 175, "Tip: Type 'exit' or click [Exit GUI]", Color::MUTED_TEXT, None);
            win.surface.draw_string(12, 195, "     Press [ESC] to return to CLI", Color::MUTED_TEXT, None);
        }
    }
}

/// Dynamically refreshes lines in the Kernel Log window.
pub fn update_kernel_log() {
    let mut wm_lock = WINDOW_MANAGER.lock();
    if let Some(ref mut wm) = *wm_lock {
        if let Some(win) = wm.windows.iter_mut().find(|w| w.title.starts_with("Kernel Log")) {
            let logs = crate::drivers::console::get_dmesg_lines(14);
            win.render_log_surface(&logs);
        }
    }
}

/// Runs the dedicated, exclusive Desktop Window Manager & Compositor event loop.
///
/// While this loop is executing:
/// - Screen rendering is owned exclusively by the Compositor backbuffer.
/// - Direct console text rendering to video memory is suspended to prevent visual corruption.
/// - Background kernel messages are safely recorded into the `dmesg` ring buffer.
/// - Mouse clicks, window dragging, and keyboard input are dispatched interactively.
/// - Pressing `ESC`, typing `exit` in the terminal, or clicking `[Exit GUI]` exits cleanly.
pub fn run_compositor() {
    // 1. Suspend direct console video writing to give compositor 100% display ownership
    crate::drivers::console::set_suspended(true);

    // 2. Enable PS/2 mouse interface
    crate::drivers::keyboard::enable_mouse();

    // 3. Spawn desktop windows
    launch_demo();

    let mut last_render_tick = crate::arch::x86_64::apic::get_ticks();
    let mut last_stats_tick = last_render_tick;

    // Reset running state
    {
        let mut wm_lock = WINDOW_MANAGER.lock();
        if let Some(ref mut wm) = *wm_lock {
            wm.is_running = true;
        }
    }

    loop {
        // Check running flag
        {
            let wm_lock = WINDOW_MANAGER.lock();
            if let Some(ref wm) = *wm_lock {
                if !wm.is_running {
                    break;
                }
            } else {
                break;
            }
        }

        // 1. Poll USB inputs (HID keyboards and mice) and check incoming characters
        crate::drivers::usb::poll_inputs();

        let incoming_char = if let Some(ch) = crate::drivers::usb::read_char() {
            Some(ch)
        } else if let Some(ch) = crate::drivers::keyboard::read_char() {
            Some(ch)
        } else if let Some(byte) = crate::drivers::serial::read_byte() {
            match byte {
                b'\r' | b'\n' => Some('\n'),
                0x08 | 0x7F => Some('\x08'),
                0x1B => Some('\x1B'), // ESC key
                b if b >= 0x20 && b <= 0x7E => Some(b as char),
                _ => None,
            }
        } else {
            None
        };

        if let Some(ch) = incoming_char {
            if ch == '\x1B' {
                // ESC key: Exit GUI compositor back to text shell
                break;
            }

            // Dispatch keyboard character to focused window
            let mut wm_lock = WINDOW_MANAGER.lock();
            if let Some(ref mut wm) = *wm_lock {
                wm.is_dirty = true;
                if let Some(focused_id) = wm.focused_window {
                    if let Some(win) = wm.windows.iter_mut().find(|w| w.id == focused_id) {
                        if win.title.starts_with("Terminal") {
                            match ch {
                                '\n' => {
                                    let cmd = win.input_line.clone();
                                    win.term_lines.push(alloc::format!("nullos$ {}", cmd));
                                    let trimmed = cmd.trim();
                                    if trimmed == "exit" || trimmed == "quit" {
                                        wm.is_running = false;
                                    } else if trimmed == "clear" {
                                        win.term_lines.clear();
                                    } else if trimmed == "dmesg" {
                                        let logs = crate::drivers::console::get_dmesg_lines(12);
                                        for l in logs {
                                            win.term_lines.push(l);
                                        }
                                    } else if !trimmed.is_empty() {
                                        let (output, _) = crate::task::shell::execute_command_capture(trimmed);
                                        for line in output.split('\n') {
                                            if !line.is_empty() {
                                                win.term_lines.push(line.to_string());
                                            }
                                        }
                                    }
                                    win.input_line.clear();
                                    win.render_terminal_surface();
                                }
                                '\x08' => {
                                    win.input_line.pop();
                                    win.render_terminal_surface();
                                }
                                c if c >= ' ' && c <= '~' => {
                                    win.input_line.push(c);
                                    win.render_terminal_surface();
                                }
                                _ => {}
                            }
                        }
                    }
                }
            }
        }

        // 2. Drain any queued input events (Mouse movements and clicks) under a single lock
        {
            let mut wm_lock = WINDOW_MANAGER.lock();
            if let Some(ref mut wm) = *wm_lock {
                event::INPUT_QUEUE.drain_into(|event| {
                    wm.handle_event(event);
                });
            }
        }

        // 3. Periodically update dynamic statistics (System Monitor & Kernel Log) every 1 second
        let current_tick = crate::arch::x86_64::apic::get_ticks();
        if current_tick.saturating_sub(last_stats_tick) >= 100 {
            last_stats_tick = current_tick;
            update_system_monitor();
            update_kernel_log();
            let mut wm_lock = WINDOW_MANAGER.lock();
            if let Some(ref mut wm) = *wm_lock {
                wm.is_dirty = true;
            }
        }

        // 4. Render desktop frame (~30 FPS) only when the display state is dirty
        let mut should_render = false;
        {
            let wm_lock = WINDOW_MANAGER.lock();
            if let Some(ref wm) = *wm_lock {
                if wm.is_dirty && (current_tick.saturating_sub(last_render_tick) >= 3 || incoming_char.is_some()) {
                    should_render = true;
                }
            }
        }
        if should_render {
            last_render_tick = current_tick;
            let mut wm_lock = WINDOW_MANAGER.lock();
            if let Some(ref mut wm) = *wm_lock {
                wm.render_desktop();
            }
        }

        // When idle with no pending render or input, sleep for ~1ms to avoid burning 100% CPU on tight spinlocks
        if !should_render && incoming_char.is_none() {
            crate::arch::x86_64::apic::spin_delay_us(1000);
        } else {
            crate::task::yield_now();
        }
    }

    // Exiting GUI:
    // Resume direct console video writing, clear physical screen, and report return
    crate::drivers::console::set_suspended(false);
    crate::drivers::console::clear_screen();
    crate::println!("[GUI] Desktop Compositor closed. Returned to text shell.");
}
