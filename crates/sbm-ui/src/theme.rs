//! Shared SBM Core palette supporting dark, light, and high-contrast themes.
//!
//! Palette tokens mirror `sbm-uefi`'s `sbm-gfx::theme::Theme` so the EFI boot
//! picker and SBM Core management shell feel like one coherent system.

use iced::theme::Palette;
use iced::{Color, Theme};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Appearance {
    pub dark: bool,
    pub high_contrast: bool,
}

impl Default for Appearance {
    fn default() -> Self {
        Self {
            dark: true,
            high_contrast: false,
        }
    }
}

impl Appearance {
    pub fn label(self) -> &'static str {
        match (self.dark, self.high_contrast) {
            (true, false) => "Dark",
            (false, false) => "Light",
            (true, true) => "High contrast dark",
            (false, true) => "High contrast light",
        }
    }

    pub fn muted_color(self) -> Color {
        match (self.dark, self.high_contrast) {
            (true, false) => Color::from_rgb8(0x9A, 0xA6, 0xB2),
            (false, false) => Color::from_rgb8(0x52, 0x5E, 0x69),
            (true, true) => Color::from_rgb8(0xD2, 0xDC, 0xE6),
            (false, true) => Color::from_rgb8(0x2B, 0x33, 0x3C),
        }
    }

    pub fn palette(self) -> Palette {
        match (self.dark, self.high_contrast) {
            (true, false) => Palette {
                background: Color::from_rgb8(0x10, 0x14, 0x1A),
                text: Color::from_rgb8(0xF2, 0xF5, 0xF8),
                primary: Color::from_rgb8(0x4C, 0x8D, 0xFF),
                success: Color::from_rgb8(0x3D, 0xDC, 0x97),
                warning: Color::from_rgb8(0xE6, 0xB4, 0x50),
                danger: Color::from_rgb8(0xFF, 0x5D, 0x5D),
            },
            (false, false) => Palette {
                background: Color::from_rgb8(0xF3, 0xF5, 0xF8),
                text: Color::from_rgb8(0x1A, 0x1D, 0x21),
                primary: Color::from_rgb8(0x0B, 0x57, 0xD0),
                success: Color::from_rgb8(0x0D, 0x7A, 0x4F),
                warning: Color::from_rgb8(0x9A, 0x67, 0x00),
                danger: Color::from_rgb8(0xC5, 0x22, 0x1F),
            },
            (true, true) => Palette {
                background: Color::from_rgb8(0x00, 0x00, 0x00),
                text: Color::from_rgb8(0xFF, 0xFF, 0xFF),
                primary: Color::from_rgb8(0x7A, 0xB8, 0xFF),
                success: Color::from_rgb8(0x5E, 0xF2, 0xB0),
                warning: Color::from_rgb8(0xFF, 0xD8, 0x66),
                danger: Color::from_rgb8(0xFF, 0x80, 0x80),
            },
            (false, true) => Palette {
                background: Color::from_rgb8(0xFF, 0xFF, 0xFF),
                text: Color::from_rgb8(0x00, 0x00, 0x00),
                primary: Color::from_rgb8(0x00, 0x3B, 0x99),
                success: Color::from_rgb8(0x00, 0x5C, 0x36),
                warning: Color::from_rgb8(0x7A, 0x4A, 0x00),
                danger: Color::from_rgb8(0x9E, 0x00, 0x00),
            },
        }
    }

    pub fn theme(self) -> Theme {
        Theme::custom(format!("SBM {}", self.label()), self.palette())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_four_appearance_variants_build_distinct_palettes() {
        let variants = [
            Appearance {
                dark: true,
                high_contrast: false,
            },
            Appearance {
                dark: false,
                high_contrast: false,
            },
            Appearance {
                dark: true,
                high_contrast: true,
            },
            Appearance {
                dark: false,
                high_contrast: true,
            },
        ];
        for (i, a) in variants.iter().enumerate() {
            let _ = a.theme();
            for (j, b) in variants.iter().enumerate() {
                if i != j {
                    assert_ne!(a.palette().background, b.palette().background);
                }
            }
        }
    }
}
