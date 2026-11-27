//! Shared accessible typography roles for SBM Core (via Iced).
//!
//! Uses the exact same Atkinson Hyperlegible Next and Atkinson Hyperlegible Mono
//! TrueType vector assets and five-role hierarchy (`Display`, `Heading`, `Body`,
//! `Caption`, `Technical`) as `sbm-uefi`'s `sbm-gfx`.

use iced::Font;
use iced::font::{Family, Weight};
use iced::widget::{Text, text};

/// Optimized Atkinson Hyperlegible Next Regular (SIL OFL 1.1).
pub const ATKINSON_NEXT_REGULAR_BYTES: &[u8] =
    include_bytes!("../assets/fonts/AtkinsonHyperlegibleNext-Regular.ttf");

/// Optimized Atkinson Hyperlegible Next SemiBold (SIL OFL 1.1).
pub const ATKINSON_NEXT_SEMIBOLD_BYTES: &[u8] =
    include_bytes!("../assets/fonts/AtkinsonHyperlegibleNext-SemiBold.ttf");

/// Optimized Atkinson Hyperlegible Mono Regular (SIL OFL 1.1).
pub const ATKINSON_MONO_REGULAR_BYTES: &[u8] =
    include_bytes!("../assets/fonts/AtkinsonHyperlegibleMono-Regular.ttf");

/// Atkinson Hyperlegible Next Regular for Body and Caption roles.
pub const ATKINSON_NEXT_REGULAR: Font = Font {
    family: Family::Name("Atkinson Hyperlegible Next"),
    weight: Weight::Normal,
    ..Font::DEFAULT
};

/// Atkinson Hyperlegible Next SemiBold for Display and Heading roles.
pub const ATKINSON_NEXT_SEMIBOLD: Font = Font {
    family: Family::Name("Atkinson Hyperlegible Next"),
    weight: Weight::Semibold,
    ..Font::DEFAULT
};

/// Atkinson Hyperlegible Mono Regular for Technical identifiers and diagnostics.
pub const ATKINSON_MONO_REGULAR: Font = Font {
    family: Family::Name("Atkinson Hyperlegible Mono"),
    weight: Weight::Normal,
    ..Font::DEFAULT
};

/// Shared semantic typography roles matching `sbm_gfx::typography::TextRole`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TextRole {
    /// Large hardware specifications and primary figures.
    Display,
    /// Device names, operating system names, and section headings.
    Heading,
    /// Ordinary interface content and descriptions.
    Body,
    /// Secondary information, explanations, and metadata captions.
    Caption,
    /// Technical identifiers, hexadecimal values, paths, and diagnostic output.
    Technical,
}

/// Adjustable interface scaling percentage (clamped to `75..=200`), matching `sbm-gfx`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct UiScale(u16);

impl UiScale {
    pub const PRESETS: [u16; 7] = [85, 100, 115, 125, 150, 175, 200];

    pub const fn new(percent: u16) -> Self {
        let clamped = if percent < 75 {
            75
        } else if percent > 200 {
            200
        } else {
            percent
        };
        Self(clamped)
    }

    pub const fn percent(self) -> u16 {
        self.0
    }

    pub fn step_up(self) -> Self {
        for &p in &Self::PRESETS {
            if p > self.0 {
                return Self(p);
            }
        }
        Self(*Self::PRESETS.last().unwrap_or(&200))
    }

    pub fn step_down(self) -> Self {
        for &p in Self::PRESETS.iter().rev() {
            if p < self.0 {
                return Self(p);
            }
        }
        Self(*Self::PRESETS.first().unwrap_or(&85))
    }
}

impl Default for UiScale {
    fn default() -> Self {
        Self::new(100)
    }
}

/// Base body size in logical pixels at 100% scale (matches `sbm-gfx` at 1280x800).
const BASE_BODY_PX: f32 = 16.0;

/// Compute the logical pixel size for `role` at `scale`, using the same role
/// multipliers as `sbm_gfx::typography::role_pixel_size`.
pub fn role_size(role: TextRole, scale: UiScale) -> f32 {
    let mult = match role {
        TextRole::Display => 2.00,
        TextRole::Heading => 1.32,
        TextRole::Body => 1.00,
        TextRole::Caption => 0.84,
        TextRole::Technical => 0.90,
    };
    let scaled = BASE_BODY_PX * mult * (f32::from(scale.percent()) / 100.0);
    ((scaled * 2.0).round() * 0.5).clamp(11.0, 96.0)
}

/// Select the Atkinson Hyperlegible font face for `role`.
pub const fn role_font(role: TextRole) -> Font {
    match role {
        TextRole::Display | TextRole::Heading => ATKINSON_NEXT_SEMIBOLD,
        TextRole::Body | TextRole::Caption => ATKINSON_NEXT_REGULAR,
        TextRole::Technical => ATKINSON_MONO_REGULAR,
    }
}

/// Build a styled Iced `Text` widget for `role` at `scale`.
pub fn role_text<'a>(
    content: impl text::IntoFragment<'a>,
    role: TextRole,
    scale: UiScale,
) -> Text<'a> {
    text(content)
        .font(role_font(role))
        .size(role_size(role, scale))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_fonts_are_non_empty_valid_truetype() {
        for bytes in [
            ATKINSON_NEXT_REGULAR_BYTES,
            ATKINSON_NEXT_SEMIBOLD_BYTES,
            ATKINSON_MONO_REGULAR_BYTES,
        ] {
            assert!(bytes.len() > 20_000);
            assert_eq!(
                &bytes[0..4],
                b"\x00\x01\x00\x00",
                "expected TrueType sfnt header"
            );
        }
    }

    #[test]
    fn role_hierarchy_is_monotonic_across_scales() {
        for &pct in &UiScale::PRESETS {
            let scale = UiScale::new(pct);
            let display = role_size(TextRole::Display, scale);
            let heading = role_size(TextRole::Heading, scale);
            let body = role_size(TextRole::Body, scale);
            let technical = role_size(TextRole::Technical, scale);
            let caption = role_size(TextRole::Caption, scale);
            assert!(display > heading);
            assert!(heading > body);
            assert!(body >= technical);
            assert!(technical >= caption);
            assert!(caption >= 11.0);
        }
    }

    #[test]
    fn scale_clamping_and_stepping() {
        assert_eq!(UiScale::new(10).percent(), 75);
        assert_eq!(UiScale::new(500).percent(), 200);
        assert_eq!(UiScale::default().step_up().percent(), 115);
        assert_eq!(UiScale::default().step_down().percent(), 85);
    }

    #[test]
    fn roles_map_to_expected_atkinson_faces() {
        assert_eq!(role_font(TextRole::Display), ATKINSON_NEXT_SEMIBOLD);
        assert_eq!(role_font(TextRole::Heading), ATKINSON_NEXT_SEMIBOLD);
        assert_eq!(role_font(TextRole::Body), ATKINSON_NEXT_REGULAR);
        assert_eq!(role_font(TextRole::Caption), ATKINSON_NEXT_REGULAR);
        assert_eq!(role_font(TextRole::Technical), ATKINSON_MONO_REGULAR);
    }
}
