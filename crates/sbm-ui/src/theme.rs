//! Local copy of the SBM palette.
//!
//! The shared design tokens are not in `sbm-common` yet. These values match
//! `docs/design-tokens.json` and can move there when that crate gains a theme.

use iced::theme::Palette;
use iced::{Color, Theme};

pub const BACKGROUND: Color = Color::from_rgb8(0x12, 0x14, 0x18);
pub const TEXT: Color = Color::from_rgb8(0xE7, 0xEC, 0xF3);
pub const MUTED: Color = Color::from_rgb8(0x9A, 0xA3, 0xB2);
pub const ACCENT: Color = Color::from_rgb8(0x3D, 0x8B, 0xFD);
pub const OK: Color = Color::from_rgb8(0x3D, 0xDC, 0x97);
pub const WARNING: Color = Color::from_rgb8(0xE6, 0xB4, 0x50);
pub const DANGER: Color = Color::from_rgb8(0xE3, 0x5D, 0x6A);

pub fn theme() -> Theme {
    Theme::custom(
        "SBM",
        Palette {
            background: BACKGROUND,
            text: TEXT,
            primary: ACCENT,
            success: OK,
            warning: WARNING,
            danger: DANGER,
        },
    )
}
