# Chapter 25: Framebuffer, 2D Graphics Canvas & Window Manager
### *Linear GOP Framebuffers, Tear-Free Double Buffering, Bresenham Rasterization, Alpha Compositing, Typography, PS/2 Mouse Tracking, and Desktop Window Management*

---

## 1. Evolution of the Display: From VGA Text Mode to Graphical Desktops

In early operating systems (and in early milestones of NullOS), interaction was confined to terminal text mode or simple character console drivers. While efficient for command-line navigation, modern operating systems demand rich visual presentation: overlapping application windows, interactive mouse pointers, anti-aliased typography, and graphical data visualization.

```
┌──────────────────────────────────────────────────────────────┐
│  NullOS 64-Bit Desktop Window Manager (Milestone 30)          │
├──────────────────────────────────────────────────────────────┤
│                                                              │
│   ┌───────────────────────────────┐                          │
│   │ System Monitor            [X] │                          │
│   ├───────────────────────────────┤   ┌──────────────────┐   │
│   │ CPU Cores:    4               │   │ Terminal     [X] │   │
│   │ APIC Ticks:   1,420           │   ├──────────────────┤   │
│   │ Free Memory:  444 MiB         │   │ nullos$ gui      │   │
│   │ Compositor:   Active          │   │ nullos$ _        │   │
│   └───────────────────────────────┘   └──────────────────┘   │
│                                                              │
│                                              ↖ (Mouse Cursor)│
├──────────────────────────────────────────────────────────────┤
│ [NullOS]  [System Monitor] [Terminal]                Up: 14s │
└──────────────────────────────────────────────────────────────┘
```

Milestone 30 equips NullOS with a complete 2D hardware graphics stack and Desktop Window Manager (WM):
1. **UEFI GOP Linear Framebuffer**: Direct hardware access to 32-bit true-color video memory.
2. **Double-Buffered Canvas**: Smooth, tear-free rendering using offscreen memory buffers.
3. **2D Graphics Primitives**: Bresenham lines, outlined/filled rectangles, midpoint circles, and alpha blending.
4. **Antialiased Font Rendering**: High-legibility monospaced typography via `noto-sans-mono-bitmap`.
5. **Unified Input Events Subsystem**: Non-blocking event queues for keyboard and mouse inputs.
6. **PS/2 Mouse Driver**: 3-byte packet decoder, screen boundary clamping, and anti-aliased cursor sprite.
7. **Window Abstraction**: Movable windows with decorated gradient title bars, close buttons, and isolated client canvases.
8. **Desktop Window Manager & Compositor**: Wallpaper generation, bottom taskbar, Z-order window stacking, and click-to-focus dragging.

---

## 2. UEFI GOP Linear Framebuffer Architecture

Under modern UEFI firmware, legacy BIOS INT 10h video interrupts and VGA 80x25 text mode are obsolete. Instead, the firmware discovers video hardware via PCI and configures a **Graphics Output Protocol (GOP)** linear framebuffer before handing control over to the operating system bootloader.

### Memory-Mapped Video RAM (MMIO)
The linear framebuffer is a contiguous array of physical memory mapped directly into the GPU's Video RAM (VRAM). In NullOS, the bootloader records this configuration into the `FramebufferInfo` structure:

```rust
pub struct FramebufferInfo {
    pub address: u64,          // Physical base address in MMIO (e.g., 0x8000_0000)
    pub size: usize,           // Total buffer size in bytes
    pub width: usize,          // Display horizontal resolution (e.g., 1280 pixels)
    pub height: usize,         // Display vertical resolution (e.g., 800 pixels)
    pub stride: usize,         // Number of pixels per scanline (>= width)
    pub bytes_per_pixel: u8,   // Typically 4 bytes (32 bits per pixel)
    pub pixel_format: PixelFormat,
}
```

### The Concept of Stride (Pitch)
A crucial hardware reality is that `stride` is not always equal to `width`:
- Modern GPU memory controllers align scanlines to 64-byte or 128-byte cache boundaries.
- For example, a screen width of 1,360 pixels may have a hardware stride of 1,408 pixels (with 48 padding pixels per line).
- Therefore, calculating the linear memory offset for pixel $(x, y)$ must always use `stride`:

$$\text{Pixel Offset} = (y \times \text{stride} + x) \times \text{bytes\_per\_pixel}$$

Writing directly to $(y \times \text{width} + x)$ on hardware with alignment padding would result in severe diagonal visual distortion ("shearing").

---

## 3. Tear-Free Double Buffering

If an operating system draws graphical elements directly into video RAM while the display hardware scans out pixels to the monitor (60 to 144 times per second), the user observes **screen tearing**—a visual artifact where top and bottom portions of the screen display frames from different moments in time:

```
Scanout Beam ───►  [ Top Half: Frame N (New Window Position)    ]
                   ──────────────────────────────────────────────  ◄── Tear Line
                   [ Bottom Half: Frame N-1 (Old Window Position)]
```

### The Solution: Offscreen Backbuffer & Single Blit Flush
To eliminate tearing, NullOS utilizes **double buffering**:
1. All drawing operations (wallpaper, windows, text, mouse cursor) are rendered into a heap-allocated memory buffer in system RAM (`Canvas.buffer: Vec<u32>`).
2. When the frame is complete, `flush_to()` copies the backbuffer to video RAM in a single, high-speed block copy:

```rust
pub fn flush_to(&self, hw_address: u64, hw_stride: usize, hw_height: usize) {
    if hw_address == 0 {
        return;
    }
    let hw_ptr = hw_address as *mut u32;
    let copy_height = self.height.min(hw_height);
    let copy_stride = self.stride.min(hw_stride);

    if self.stride == hw_stride {
        // Fast path: contiguous memory block copy
        let total_pixels = copy_stride * copy_height;
        unsafe {
            core::ptr::copy_nonoverlapping(self.buffer.as_ptr(), hw_ptr, total_pixels);
        }
    } else {
        // Stride mismatch: copy row-by-row
        for row in 0..copy_height {
            let src_offset = row * self.stride;
            let dst_offset = row * hw_stride;
            unsafe {
                core::ptr::copy_nonoverlapping(
                    self.buffer.as_ptr().add(src_offset),
                    hw_ptr.add(dst_offset),
                    copy_stride,
                );
            }
        }
    }
}
```

---

## 4. 2D Graphics Primitives

NullOS implements fundamental 2D rasterization algorithms entirely in integer arithmetic to ensure high performance and safety within the `#![no_std]` kernel.

### 1. Alpha Compositing (Porter-Duff Source-Over)
Transparent window elements, anti-aliased font edges, and translucent drop shadows require alpha compositing. Given a source color $(C_s, \alpha_s)$ drawn over a background color $(C_d, \alpha_d)$, the blended output color is computed using the standard Porter-Duff formula:

$$C_{out} = \frac{C_s \cdot \alpha_s + C_d \cdot (255 - \alpha_s)}{255}$$

In `kernel/src/graphics/color.rs`, this is implemented with fast integer math:

```rust
pub fn blend_over(self, bg: Color) -> Color {
    if self.a == 255 {
        return self;
    }
    if self.a == 0 {
        return bg;
    }

    let alpha = self.a as u32;
    let inv_alpha = 255 - alpha;

    let r = ((self.r as u32 * alpha + bg.r as u32 * inv_alpha) / 255) as u8;
    let g = ((self.g as u32 * alpha + bg.g as u32 * inv_alpha) / 255) as u8;
    let b = ((self.b as u32 * alpha + bg.b as u32 * inv_alpha) / 255) as u8;

    Color::rgb(r, g, b)
}
```

### 2. Bresenham's Line Algorithm
Rasterizing a continuous line between coordinates $(x_0, y_0)$ and $(x_1, y_1)$ without floating-point math requires **Bresenham's Algorithm**. The algorithm tracks an accumulated integer error term:

```rust
pub fn draw_line(&mut self, mut x0: isize, mut y0: isize, x1: isize, y1: isize, color: Color) {
    let dx = (x1 - x0).abs();
    let dy = -(y1 - y0).abs();
    let sx = if x0 < x1 { 1 } else { -1 };
    let sy = if y0 < y1 { 1 } else { -1 };
    let mut err = dx + dy;

    loop {
        if x0 >= 0 && y0 >= 0 && (x0 as usize) < self.width && (y0 as usize) < self.height {
            self.set_pixel(x0 as usize, y0 as usize, color);
        }
        if x0 == x1 && y0 == y1 {
            break;
        }
        let e2 = 2 * err;
        if e2 >= dy {
            err += dy;
            x0 += sx;
        }
        if e2 <= dx {
            err += dx;
            y0 += sy;
        }
    }
}
```

### 3. Midpoint Circle Algorithm
Circles are rasterized using the **Midpoint Circle Algorithm**, exploiting 8-way octant symmetry: a single calculated point $(x, y)$ generates 8 symmetric points around the center:

```
        (-y, x)   (y, x)
   (-x, y)             (x, y)
             (cx, cy)
   (-x,-y)             (x,-y)
        (-y,-x)   (y,-x)
```

By stepping $y$ by 1 on each iteration and adjusting $x$ based on the decision parameter $d = 1 - r$, circles are rendered with perfect curvature using pure integer additions and subtractions.

---

## 5. Typography & Font Rendering

To render crisp text in window headers, terminal consoles, and taskbars, NullOS integrates `noto-sans-mono-bitmap`. Each character is rasterized into a high-contrast bitmap where pixel intensity represents the coverage alpha channel:

```rust
pub fn draw_char(&mut self, x: usize, y: usize, c: char, fg: Color, bg: Option<Color>) {
    if let Some(bg_color) = bg {
        self.fill_rect(x, y, CHAR_WIDTH, CHAR_HEIGHT, bg_color);
    }

    let raster = get_raster(c, FontWeight::Regular, FONT_HEIGHT)
        .unwrap_or_else(|| get_raster('?', FontWeight::Regular, FONT_HEIGHT).unwrap());

    for (row_idx, row) in raster.raster().iter().enumerate() {
        let py = y + row_idx;
        if py >= self.height { break; }

        for (col_idx, &intensity) in row.iter().enumerate() {
            let px = x + col_idx;
            if px >= self.width { break; }

            if intensity > 0 {
                let mut glyph_color = fg;
                glyph_color.a = intensity;
                self.set_pixel(px, py, glyph_color);
            }
        }
    }
}
```

This design provides anti-aliasing against arbitrary window backgrounds without jagged aliasing edges.

---

## 6. PS/2 Mouse & Unified Input Events

The PS/2 auxiliary device controller sends 3-byte packets for mouse movement and button actions:

```
Byte 0: [ Y_OVF | X_OVF | Y_SIGN | X_SIGN | ALWAYS_1 | MID_BTN | RGHT_BTN | LEFT_BTN ]
Byte 1: [ X Movement Delta (9-bit signed integer when combined with X_SIGN)           ]
Byte 2: [ Y Movement Delta (9-bit signed integer when combined with Y_SIGN)           ]
```

### Decoding & Sign Extension
Because the deltas are 9-bit two's complement numbers, the high bit is extracted from Byte 0 and sign-extended:

```rust
let dx = if (flags & 0x10) != 0 { raw_dx - 256 } else { raw_dx };
// Note: PS/2 hardware reports positive Y for upward movement
let dy = if (flags & 0x20) != 0 { -(raw_dy - 256) } else { -raw_dy };
```

Mouse positions are clamped to screen boundaries $(0 \le x < \text{width}, 0 \le y < \text{height})$ and posted into the unified `INPUT_QUEUE`.

---

## 7. Desktop Window Manager & Compositor

The `WindowManager` coordinates the complete user interface:

### 1. Z-Order Window Stacking
Windows are maintained in an ordered `Vec<Window>`:
- The window at index $0$ is at the bottom of the stack.
- The window at the last index is on top (active, receiving input focus).
- When the user clicks on an inactive window, `bring_to_front(win_id)` moves it to the end of the array.

### 2. Interactive Title Bar Dragging
When a mouse down event occurs within `is_titlebar(px, py)`, the window manager records:
$$\text{drag\_state} = (\text{win\_id}, \text{mouse\_x} - \text{win.x}, \text{mouse\_y} - \text{win.y})$$
Subsequent `MouseMove` events update `win.x` and `win.y` dynamically, allowing fluid window dragging.

### 3. The Desktop Taskbar
The bottom 32 scanlines are reserved for the system taskbar:
- Displays the **Start** badge ("NullOS").
- Displays clickable buttons for each open window with focus highlights.
- Right-aligned system stats (uptime timer clock and memory consumption).

---

## 8. Automated Verification & Testing

Milestone 30 is thoroughly validated through automated test suite `Test 22` in `kernel/src/task/shell.rs`:

```rust
println!("[SHELL] 22. Testing 2D Graphics Canvas & Desktop Window Manager (M30)...");
{
    let mut test_canvas = Canvas::new(100, 100, 100);
    test_canvas.clear(Color::BLACK);
    assert_eq!(test_canvas.get_pixel(0, 0), Color::BLACK);

    test_canvas.set_pixel(10, 10, Color::RED);
    assert_eq!(test_canvas.get_pixel(10, 10), Color::RED);

    test_canvas.draw_line(0, 0, 99, 99, Color::WHITE);
    assert_eq!(test_canvas.get_pixel(50, 50), Color::WHITE);

    test_canvas.fill_rect(20, 20, 10, 10, Color::GREEN);
    assert_eq!(test_canvas.get_pixel(25, 25), Color::GREEN);

    test_canvas.draw_circle(50, 50, 20, Color::BLUE);
    test_canvas.draw_string(5, 5, "Hi", Color::YELLOW, None);

    let mut wm_lock = crate::graphics::wm::WINDOW_MANAGER.lock();
    if let Some(ref mut wm) = *wm_lock {
        let win_id = wm.create_window("Unit Test Window", 10, 10, 80, 60);
        assert_eq!(wm.windows.len(), 1);
        assert_eq!(wm.focused_window, Some(win_id));

        // Test mouse drag interaction via event handler
        wm.handle_event(InputEvent::MouseDown { button: MouseButton::Left, x: 20, y: 15 });
        assert!(wm.dragging.is_some());

        wm.handle_event(InputEvent::MouseMove { x: 30, y: 25, dx: 10, dy: 10 });
        assert_eq!(wm.windows[0].x, 20);

        wm.handle_event(InputEvent::MouseUp { button: MouseButton::Left, x: 30, y: 25 });
        assert!(wm.dragging.is_none());

        wm.render_desktop();
        wm.close_window(win_id);
        assert_eq!(wm.windows.len(), 0);
    }
}
crate::graphics::launch_demo();
```

---

## 9. Summary & Milestone Checklist

Milestone 30 elevates NullOS from a headless command-line kernel into a full graphical desktop operating system:

| Component | Status | Implementation Details |
| :--- | :---: | :--- |
| **Linear Framebuffer** | ✅ Done | UEFI GOP 32-bit MMIO linear scanout buffer |
| **Double Buffering** | ✅ Done | Tear-free offscreen backbuffer with fast block flush |
| **2D Primitives API** | ✅ Done | Bresenham lines, midpoint circles, rectangles, alpha blending |
| **Typography** | ✅ Done | Monospaced bitmap font rasterization with antialiasing |
| **Input Events Queue**| ✅ Done | Centralized lockless `InputEvent` FIFO |
| **PS/2 Mouse** | ✅ Done | 3-byte packet decoder, clamping, and arrow cursor sprite |
| **Window Abstraction**| ✅ Done | Decorated frames, title bars, close buttons, client surfaces |
| **Window Manager** | ✅ Done | Wallpaper, bottom taskbar, Z-order stack, title dragging |
| **Shell Command** | ✅ Done | `gui` command and automated test 22 |
