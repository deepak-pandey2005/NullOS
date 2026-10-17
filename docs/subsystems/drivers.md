# Subsystem: Kernel Drivers (`kernel/src/drivers/`)

The `drivers/` directory encapsulates hardware communication devices that produce output or accept input.

---

## 1. Serial Driver (`serial.rs`)

### Purpose
The COM1 serial port (`0x3F8`) provides reliable, hardware-level diagnostic logging that can be piped directly into QEMU's stdout (`-serial stdio`) and automated testing scripts.

### I/O Port Specifications
- **Data Register**: `0x3F8`
- **Line Status Register**: `0x3FD`
- **Transmit Empty Flag**: Bit 5 (`0x20`)

### How `println!` Works
```
           println!("Hello {}", world);
                       │
                       ▼
       [$crate::drivers::serial::SerialWriter]
                       │
                       ▼
        [drivers::serial::write_str]
           │                       │
           ▼                       ▼
  [COM1 Outb (0x3F8)]    [drivers::console::write_str]
  (Terminal Output)       (GOP Screen Pixel Rendering)
```

Because `write_str` distributes every string to both COM1 and the Framebuffer Console, all kernel logging is seamlessly mirrored on the physical monitor and the terminal without requiring two separate logging macros.

---

## 2. Framebuffer Console (`console.rs`)

### Purpose
Provides a graphical text console rendered directly on the raw GOP pixel buffer once UEFI services have terminated.

### Features
1. **Font Engine**:
   - Uses `noto-sans-mono-bitmap`, an allocation-free, `no_std` font rasterizer.
   - Size 16 raster height (~9×16 pixels per glyph).
2. **Color Palette**:
   - Background: Deep Slate Blue (`0x000F172A`)
   - Foreground Text: Light Slate White (`0x00E2E8F0`)
   - Soft sub-pixel antialiasing blending algorithm for smooth character edges.
3. **Terminal Emulation Logic**:
   - Cursor `x` and `y` tracking.
   - Automatic line wrapping when `cursor_x + CHAR_WIDTH > screen_width`.
   - Newline (`\n`), carriage return (`\r`), and 4-space tab (`\t`) handling.
   - Smooth vertical scrolling (`scroll_up`): moves pixel rows upward in memory and repaints the bottom row with the background color when text exceeds screen height.
