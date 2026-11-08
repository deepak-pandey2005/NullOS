//! Color representation and alpha blending primitives for NullOS 2D Graphics.

/// A 32-bit ARGB/RGBA color representation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Color {
    pub a: u8,
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Color {
    pub const BLACK: Color = Color::rgb(0x00, 0x00, 0x00);
    pub const WHITE: Color = Color::rgb(0xFF, 0xFF, 0xFF);
    pub const RED: Color = Color::rgb(0xEF, 0x44, 0x44);
    pub const GREEN: Color = Color::rgb(0x22, 0xC5, 0x5E);
    pub const BLUE: Color = Color::rgb(0x3B, 0x82, 0xF6);
    pub const YELLOW: Color = Color::rgb(0xEA, 0xB3, 0x08);
    pub const MAGENTA: Color = Color::rgb(0xA8, 0x55, 0xF7);
    pub const CYAN: Color = Color::rgb(0x06, 0xB6, 0xD4);
    pub const TRANSPARENT: Color = Color::rgba(0, 0, 0, 0);

    // Modern NullOS Desktop Theme Palette
    pub const DARK_SLATE: Color = Color::rgb(0x0F, 0x17, 0x2A);   // Background #0f172a
    pub const PANEL_BG: Color = Color::rgb(0x1E, 0x29, 0x3B);     // Surface #1e293b
    pub const TITLE_BAR: Color = Color::rgb(0x33, 0x41, 0x55);    // Window Header #334155
    pub const TITLE_ACTIVE: Color = Color::rgb(0x25, 0x63, 0xEB); // Active Header #2563eb
    pub const LIGHT_SLATE: Color = Color::rgb(0xE2, 0xE8, 0xF0);   // Text #e2e8f0
    pub const MUTED_TEXT: Color = Color::rgb(0x94, 0xA3, 0xB8);    // Subtext #94a3b8
    pub const BORDER: Color = Color::rgb(0x47, 0x55, 0x69);        // Border #475569
    pub const CLOSE_BUTTON: Color = Color::rgb(0xDC, 0x26, 0x26);  // Close Button #dc2626
    pub const TASKBAR_BG: Color = Color::rgb(0x09, 0x0D, 0x16);    // Taskbar #090d16

    /// Creates an opaque RGB color (Alpha = 255).
    #[inline]
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { a: 255, r, g, b }
    }

    /// Creates an RGBA color with explicit alpha.
    #[inline]
    pub const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { a, r, g, b }
    }

    /// Creates a Color from a 32-bit integer packed as `0x00RRGGBB` or `0xAARRGGBB`.
    #[inline]
    pub const fn from_u32(val: u32) -> Self {
        let a = ((val >> 24) & 0xFF) as u8;
        let r = ((val >> 16) & 0xFF) as u8;
        let g = ((val >> 8) & 0xFF) as u8;
        let b = (val & 0xFF) as u8;
        Self {
            a: if a == 0 { 255 } else { a },
            r,
            g,
            b,
        }
    }

    /// Converts Color to a 32-bit integer packed as `0xAARRGGBB`.
    #[inline]
    pub const fn to_u32(self) -> u32 {
        ((self.a as u32) << 24)
            | ((self.r as u32) << 16)
            | ((self.g as u32) << 8)
            | (self.b as u32)
    }

    /// Performs alpha blending of foreground `self` over background `bg`.
    ///
    /// Uses standard linear interpolation:
    /// `C_out = (C_fg * alpha + C_bg * (255 - alpha)) / 255`
    #[inline]
    pub fn blend_over(self, bg: Color) -> Color {
        if self.a == 255 {
            return self;
        }
        if self.a == 0 {
            return bg;
        }

        let a = self.a as u32;
        let inv_a = 255 - a;

        let r = ((self.r as u32 * a + bg.r as u32 * inv_a) / 255) as u8;
        let g = ((self.g as u32 * a + bg.g as u32 * inv_a) / 255) as u8;
        let b = ((self.b as u32 * a + bg.b as u32 * inv_a) / 255) as u8;

        Color::rgba(r, g, b, 255)
    }
}

