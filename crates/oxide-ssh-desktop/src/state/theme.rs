//! Theme resolution, the OxideSSH widget palette, and the terminal palette.

use gpui::{App, Hsla, Window, hsla, px};
use gpui_component::{Theme, ThemeColor, ThemeMode};
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
        // terminal readability or output semantics. Only foreground,
        // background and cursor follow the widget palette family; the 16 ANSI
        // colors stay untouched so CLI color semantics are preserved.
        match self {
            Self::Dark => TerminalColors {
                foreground: rgb_color(0xdfe2e9),
                background: rgb_color(0x16181d),
                cursor: rgb_color(0xdfe2e9),
                ansi: [
                    0x1b1d23, 0xe06c75, 0x98c379, 0xe5c07b, 0x61afef, 0xc678dd, 0x56b6c2, 0xabb2bf,
                    0x5c6370, 0xff7b86, 0xb3e98c, 0xffd68a, 0x76c7ff, 0xd99bff, 0x70e1ed, 0xf5f7fa,
                ]
                .map(rgb_color),
            },
            Self::Light => TerminalColors {
                foreground: rgb_color(0x202124),
                background: rgb_color(0xf3f4f6),
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

/// Switch gpui-component to the resolved mode, then lay the OxideSSH palette
/// over its default tokens. Cool-slate neutrals with a muted-blue accent;
/// neither mode bottoms out at pure black or pure white.
pub fn apply_theme(theme: ResolvedTheme, window: &mut Window, cx: &mut App) {
    Theme::change(
        match theme {
            ResolvedTheme::Light => ThemeMode::Light,
            ResolvedTheme::Dark => ThemeMode::Dark,
        },
        Some(window),
        cx,
    );
    let global = Theme::global_mut(cx);
    // Radius scale: 8px for controls, 12px for cards and dialogs.
    global.radius = px(8.);
    global.radius_lg = px(12.);
    match theme {
        ResolvedTheme::Dark => apply_dark_palette(&mut global.colors),
        ResolvedTheme::Light => apply_light_palette(&mut global.colors),
    }
}

fn hsl(h: f32, s: f32, l: f32) -> Hsla {
    hsla(h / 360., s / 100., l / 100., 1.)
}

fn apply_dark_palette(colors: &mut ThemeColor) {
    let background = hsl(222., 14., 12.);
    let raised = hsl(221., 12., 16.5);
    let hover = hsl(221., 11., 21.);
    let border = hsl(220., 9., 26.);
    let foreground = hsl(220., 13., 90.);
    let muted_foreground = hsl(219., 9., 62.);
    let primary = hsl(212., 33., 63.);
    let primary_hover = hsl(212., 36., 69.);
    let primary_active = hsl(212., 29., 56.);
    let primary_foreground = hsl(222., 28., 13.);
    let strip = hsl(222., 14., 13.5);

    colors.background = background;
    colors.foreground = foreground;
    colors.muted = hsl(221., 11., 19.);
    colors.muted_foreground = muted_foreground;
    colors.border = border;
    colors.input = border;
    colors.caret = foreground;
    colors.window_border = border;

    colors.primary = primary;
    colors.primary_hover = primary_hover;
    colors.primary_active = primary_active;
    colors.primary_foreground = primary_foreground;
    colors.ring = hsla(212. / 360., 0.33, 0.63, 0.55);
    colors.selection = hsla(212. / 360., 0.4, 0.6, 0.35);
    colors.link = primary;
    colors.link_hover = primary_hover;
    colors.link_active = primary_active;
    colors.progress_bar = primary;
    colors.drag_border = primary;
    colors.drop_target = hsla(212. / 360., 0.33, 0.63, 0.15);

    colors.secondary = raised;
    colors.secondary_hover = hover;
    colors.secondary_active = hover;
    colors.secondary_foreground = foreground;
    colors.accent = hover;
    colors.accent_foreground = foreground;

    colors.popover = hsl(221., 12., 18.);
    colors.popover_foreground = foreground;
    colors.group_box = raised;
    colors.group_box_foreground = foreground;
    colors.accordion = background;
    colors.accordion_hover = hover;
    colors.description_list_label = raised;
    colors.description_list_label_foreground = foreground;
    colors.skeleton = hsl(221., 11., 19.);

    colors.sidebar = strip;
    colors.sidebar_foreground = hsl(220., 10., 78.);
    colors.sidebar_border = hsl(220., 9., 22.);
    colors.sidebar_accent = hsl(221., 11., 20.);
    colors.sidebar_accent_foreground = foreground;
    colors.sidebar_primary = primary;
    colors.sidebar_primary_foreground = primary_foreground;

    colors.tab_bar = strip;
    colors.tab_bar_segmented = strip;
    colors.tab = Hsla::transparent_black();
    colors.tab_active = raised;
    colors.tab_active_foreground = foreground;
    colors.tab_foreground = muted_foreground;
    colors.title_bar = strip;
    colors.title_bar_border = border;

    colors.list = background;
    colors.list_hover = hover;
    colors.list_active = hsla(212. / 360., 0.33, 0.63, 0.16);
    colors.list_active_border = primary;
    colors.list_even = raised;
    colors.list_head = raised;

    colors.scrollbar = Hsla::transparent_black();
    colors.scrollbar_thumb = hsl(220., 8., 34.);
    colors.scrollbar_thumb_hover = hsl(220., 8., 40.);
    colors.slider_bar = hsl(221., 11., 19.);
    colors.slider_thumb = primary;
    colors.switch = hsl(220., 9., 30.);
    colors.switch_thumb = hsl(220., 20., 92.);

    colors.danger = hsl(3., 48., 50.);
    colors.danger_hover = hsl(3., 52., 56.);
    colors.danger_active = hsl(3., 45., 44.);
    colors.danger_foreground = hsl(0., 30., 96.);
}

fn apply_light_palette(colors: &mut ThemeColor) {
    let background = hsl(220., 16., 94.);
    let raised = hsl(220., 20., 97.);
    let hover = hsl(220., 13., 89.);
    let border = hsl(220., 10., 82.);
    let foreground = hsl(222., 18., 24.);
    let muted_foreground = hsl(220., 8., 45.);
    let primary = hsl(212., 45., 44.);
    let primary_hover = hsl(212., 48., 50.);
    let primary_active = hsl(212., 42., 38.);
    let primary_foreground = hsl(210., 40., 97.);
    let strip = hsl(220., 15., 92.);

    colors.background = background;
    colors.foreground = foreground;
    colors.muted = hsl(220., 13., 91.);
    colors.muted_foreground = muted_foreground;
    colors.border = border;
    colors.input = border;
    colors.caret = foreground;
    colors.window_border = border;

    colors.primary = primary;
    colors.primary_hover = primary_hover;
    colors.primary_active = primary_active;
    colors.primary_foreground = primary_foreground;
    colors.ring = hsla(212. / 360., 0.45, 0.44, 0.45);
    colors.selection = hsla(212. / 360., 0.5, 0.55, 0.28);
    colors.link = primary;
    colors.link_hover = primary_hover;
    colors.link_active = primary_active;
    colors.progress_bar = primary;
    colors.drag_border = primary;
    colors.drop_target = hsla(212. / 360., 0.45, 0.44, 0.12);

    colors.secondary = raised;
    colors.secondary_hover = hover;
    colors.secondary_active = hover;
    colors.secondary_foreground = foreground;
    colors.accent = hover;
    colors.accent_foreground = foreground;

    colors.popover = hsl(220., 20., 97.5);
    colors.popover_foreground = foreground;
    colors.group_box = raised;
    colors.group_box_foreground = foreground;
    colors.accordion = background;
    colors.accordion_hover = hover;
    colors.description_list_label = raised;
    colors.description_list_label_foreground = foreground;
    colors.skeleton = hsl(220., 13., 91.);

    colors.sidebar = strip;
    colors.sidebar_foreground = hsl(222., 14., 32.);
    colors.sidebar_border = hsl(220., 10., 84.);
    colors.sidebar_accent = hsl(220., 13., 87.);
    colors.sidebar_accent_foreground = foreground;
    colors.sidebar_primary = primary;
    colors.sidebar_primary_foreground = primary_foreground;

    colors.tab_bar = strip;
    colors.tab_bar_segmented = strip;
    colors.tab = Hsla::transparent_black();
    colors.tab_active = raised;
    colors.tab_active_foreground = foreground;
    colors.tab_foreground = muted_foreground;
    colors.title_bar = strip;
    colors.title_bar_border = border;

    colors.list = background;
    colors.list_hover = hover;
    colors.list_active = hsla(212. / 360., 0.45, 0.44, 0.12);
    colors.list_active_border = primary;
    colors.list_even = raised;
    colors.list_head = raised;

    colors.scrollbar = Hsla::transparent_black();
    colors.scrollbar_thumb = hsl(220., 8., 72.);
    colors.scrollbar_thumb_hover = hsl(220., 8., 64.);
    colors.slider_bar = hsl(220., 13., 88.);
    colors.slider_thumb = primary;
    colors.switch = hsl(220., 10., 80.);
    colors.switch_thumb = hsl(220., 20., 98.);

    colors.danger = hsl(3., 52., 52.);
    colors.danger_hover = hsl(3., 56., 57.);
    colors.danger_active = hsl(3., 48., 46.);
    colors.danger_foreground = hsl(0., 40., 97.);
}

const fn rgb_color(hex: u32) -> RgbColor {
    RgbColor::new(
        ((hex >> 16) & 0xff) as u8,
        ((hex >> 8) & 0xff) as u8,
        (hex & 0xff) as u8,
    )
}
