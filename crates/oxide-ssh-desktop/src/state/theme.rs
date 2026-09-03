//! Theme resolution and the fixed terminal palette.

use oxide_ssh_core::model::ThemeSetting;
use oxide_ssh_terminal::{RgbColor, TerminalColors};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResolvedTheme {
    Light,
    Dark,
}

impl ResolvedTheme {
    pub fn resolve(setting: ThemeSetting, system_is_dark: bool) -> Self {
        match setting {
            ThemeSetting::Light => Self::Light,
            ThemeSetting::Dark => Self::Dark,
            ThemeSetting::System if system_is_dark => Self::Dark,
            ThemeSetting::System => Self::Light,
        }
    }

    pub fn terminal_colors(self) -> TerminalColors {
        // Fixed terminal palette: changing the widget theme must never alter
        // terminal readability or output semantics.
        match self {
            Self::Dark => TerminalColors {
                foreground: rgb_color(0xe6e8eb),
                background: rgb_color(0x111318),
                cursor: rgb_color(0xe6e8eb),
                ansi: [
                    0x1b1d23, 0xe06c75, 0x98c379, 0xe5c07b, 0x61afef, 0xc678dd, 0x56b6c2, 0xabb2bf,
                    0x5c6370, 0xff7b86, 0xb3e98c, 0xffd68a, 0x76c7ff, 0xd99bff, 0x70e1ed, 0xf5f7fa,
                ]
                .map(rgb_color),
            },
            Self::Light => TerminalColors {
                foreground: rgb_color(0x202124),
                background: rgb_color(0xfafaf8),
                cursor: rgb_color(0x202124),
                ansi: [
                    0x202124, 0xb4232c, 0x2f7d32, 0x8a5a00, 0x005fb8, 0x7a3e9d, 0x007a7a, 0xdadce0,
                    0x5f6368, 0xd93025, 0x188038, 0xb06000, 0x1967d2, 0x9334e6, 0x008b8b, 0xffffff,
                ]
                .map(rgb_color),
            },
        }
    }
}

const fn rgb_color(hex: u32) -> RgbColor {
    RgbColor::new(
        ((hex >> 16) & 0xff) as u8,
        ((hex >> 8) & 0xff) as u8,
        (hex & 0xff) as u8,
    )
}
