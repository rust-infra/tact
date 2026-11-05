use gpui::{Hsla, px, rgba};
use gpui_component::{Theme, ThemeMode};
use tact_desktop_core::{Appearance, DesktopTheme};

use std::sync::OnceLock;

static REDUCED_TRANSPARENCY: OnceLock<bool> = OnceLock::new();
static HIGH_CONTRAST: OnceLock<bool> = OnceLock::new();

/// One complete desktop palette. The shell, the transcript, and every embedded
/// `gpui-component` surface read the same values, so a theme family changes the
/// whole window rather than repainting the chrome around fixed Apple colours.
#[derive(Clone, Copy)]
pub struct Palette {
    pub background: Hsla,
    pub foreground: Hsla,
    pub border: Hsla,
    pub input: Hsla,
    pub muted: Hsla,
    pub muted_foreground: Hsla,
    pub popover: Hsla,
    pub popover_foreground: Hsla,
    pub accent: Hsla,
    pub accent_foreground: Hsla,
    pub primary: Hsla,
    pub primary_hover: Hsla,
    pub primary_active: Hsla,
    pub primary_foreground: Hsla,
    pub secondary: Hsla,
    pub secondary_hover: Hsla,
    pub secondary_active: Hsla,
    pub secondary_foreground: Hsla,
    pub selection: Hsla,
    pub ring: Hsla,
    pub sidebar: Hsla,
    pub sidebar_foreground: Hsla,
    pub sidebar_border: Hsla,
    pub sidebar_accent: Hsla,
    pub sidebar_accent_foreground: Hsla,
    pub sidebar_primary: Hsla,
    pub sidebar_primary_foreground: Hsla,
    pub title_bar: Hsla,
    pub title_bar_border: Hsla,
    pub link: Hsla,
    pub link_hover: Hsla,
    pub link_active: Hsla,
    pub success: Hsla,
    pub success_foreground: Hsla,
    pub warning: Hsla,
    pub warning_foreground: Hsla,
    pub danger: Hsla,
    pub danger_foreground: Hsla,
    pub caret: Hsla,
    pub group_box: Hsla,
    pub group_box_foreground: Hsla,
}

pub fn palette(theme: DesktopTheme, appearance: Appearance) -> Palette {
    match (theme, appearance) {
        (DesktopTheme::Tact, Appearance::Light) => tact_light(),
        (DesktopTheme::Tact, Appearance::Dark) => tact_dark(),
        (DesktopTheme::Claude, Appearance::Light) => claude_light(),
        (DesktopTheme::Claude, Appearance::Dark) => claude_dark(),
    }
}

pub fn apply(
    theme_family: DesktopTheme,
    appearance: Appearance,
    window: Option<&mut gpui::Window>,
    cx: &mut gpui::App,
) {
    Theme::change(
        match appearance {
            Appearance::Light => ThemeMode::Light,
            Appearance::Dark => ThemeMode::Dark,
        },
        window,
        cx,
    );
    let theme = Theme::global_mut(cx);
    theme.font_family = "IBM Plex Sans".into();
    theme.mono_font_family = "Lilex".into();
    theme.font_size = px(14.);
    theme.mono_font_size = px(12.5);
    theme.radius = px(10.);
    theme.radius_lg = px(14.);
    theme.shadow = true;
    apply_palette(theme, &palette(theme_family, appearance));
    Theme::sync_base(cx);
}

fn apply_palette(theme: &mut Theme, palette: &Palette) {
    theme.background = palette.background;
    theme.foreground = palette.foreground;
    theme.border = palette.border;
    theme.input = palette.input;
    theme.muted = palette.muted;
    theme.muted_foreground = palette.muted_foreground;
    theme.popover = palette.popover;
    theme.popover_foreground = palette.popover_foreground;
    theme.accent = palette.accent;
    theme.accent_foreground = palette.accent_foreground;
    theme.primary = palette.primary;
    theme.primary_hover = palette.primary_hover;
    theme.primary_active = palette.primary_active;
    theme.primary_foreground = palette.primary_foreground;
    theme.secondary = palette.secondary;
    theme.secondary_hover = palette.secondary_hover;
    theme.secondary_active = palette.secondary_active;
    theme.secondary_foreground = palette.secondary_foreground;
    theme.selection = palette.selection;
    theme.ring = palette.ring;
    theme.sidebar = palette.sidebar;
    theme.sidebar_foreground = palette.sidebar_foreground;
    theme.sidebar_border = palette.sidebar_border;
    theme.sidebar_accent = palette.sidebar_accent;
    theme.sidebar_accent_foreground = palette.sidebar_accent_foreground;
    theme.sidebar_primary = palette.sidebar_primary;
    theme.sidebar_primary_foreground = palette.sidebar_primary_foreground;
    theme.title_bar = palette.title_bar;
    theme.title_bar_border = palette.title_bar_border;
    theme.link = palette.link;
    theme.link_hover = palette.link_hover;
    theme.link_active = palette.link_active;
    theme.success = palette.success;
    theme.success_foreground = palette.success_foreground;
    theme.warning = palette.warning;
    theme.warning_foreground = palette.warning_foreground;
    theme.danger = palette.danger;
    theme.danger_foreground = palette.danger_foreground;
    theme.caret = palette.caret;
    theme.group_box = palette.group_box;
    theme.group_box_foreground = palette.group_box_foreground;
}

fn tact_light() -> Palette {
    Palette {
        background: color(0xf5f5f7),
        foreground: color(0x1d1d1f),
        border: alpha(0x3c3c43, 0.14),
        input: alpha(0x3c3c43, 0.20),
        muted: color(0xececf1),
        muted_foreground: color(0x6e6e73),
        popover: color(0xffffff),
        popover_foreground: color(0x1d1d1f),
        accent: alpha(0x007aff, 0.12),
        accent_foreground: color(0x1d1d1f),
        primary: color(0x007aff),
        primary_hover: color(0x3395ff),
        primary_active: color(0x0063ce),
        primary_foreground: color(0xffffff),
        secondary: color(0xececf1),
        secondary_hover: color(0xe2e2e8),
        secondary_active: color(0xd7d7de),
        secondary_foreground: color(0x1d1d1f),
        selection: alpha(0x007aff, 0.16),
        ring: alpha(0x007aff, 0.45),
        sidebar: color(0xf2f2f7),
        sidebar_foreground: color(0x1d1d1f),
        sidebar_border: alpha(0x3c3c43, 0.12),
        sidebar_accent: alpha(0x007aff, 0.12),
        sidebar_accent_foreground: color(0x1d1d1f),
        sidebar_primary: color(0x007aff),
        sidebar_primary_foreground: color(0xffffff),
        title_bar: color(0xffffff),
        title_bar_border: alpha(0x3c3c43, 0.12),
        link: color(0x007aff),
        link_hover: color(0x0063ce),
        link_active: color(0x004999),
        success: color(0x34c759),
        success_foreground: color(0xffffff),
        warning: color(0xff9f0a),
        warning_foreground: color(0x1d1d1f),
        danger: color(0xff453a),
        danger_foreground: color(0xffffff),
        caret: color(0x007aff),
        group_box: color(0xffffff),
        group_box_foreground: color(0x1d1d1f),
    }
}

fn tact_dark() -> Palette {
    Palette {
        background: color(0x1c1c1e),
        foreground: color(0xf5f5f7),
        border: alpha(0xffffff, 0.10),
        input: alpha(0xffffff, 0.16),
        muted: color(0x2c2c2e),
        muted_foreground: color(0x98989d),
        popover: color(0x2c2c2e),
        popover_foreground: color(0xf5f5f7),
        accent: alpha(0x0a84ff, 0.18),
        accent_foreground: color(0xf5f5f7),
        primary: color(0x0a84ff),
        primary_hover: color(0x409cff),
        primary_active: color(0x0063ce),
        primary_foreground: color(0xffffff),
        secondary: color(0x3a3a3c),
        secondary_hover: color(0x48484a),
        secondary_active: color(0x2c2c2e),
        secondary_foreground: color(0xf5f5f7),
        selection: alpha(0x0a84ff, 0.28),
        ring: alpha(0x0a84ff, 0.55),
        sidebar: alpha(0x1c1c1e, 0.82),
        sidebar_foreground: color(0xf5f5f7),
        sidebar_border: alpha(0xffffff, 0.08),
        sidebar_accent: alpha(0x0a84ff, 0.22),
        sidebar_accent_foreground: color(0xf5f5f7),
        sidebar_primary: color(0x0a84ff),
        sidebar_primary_foreground: color(0xffffff),
        title_bar: color(0x2c2c2e),
        title_bar_border: alpha(0xffffff, 0.08),
        link: color(0x409cff),
        link_hover: color(0x64b5ff),
        link_active: color(0x0a84ff),
        success: color(0x30d158),
        success_foreground: color(0xffffff),
        warning: color(0xff9f0a),
        warning_foreground: color(0x1c1c1e),
        danger: color(0xff453a),
        danger_foreground: color(0xffffff),
        caret: color(0x0a84ff),
        group_box: color(0x242426),
        group_box_foreground: color(0xf5f5f7),
    }
}

fn claude_light() -> Palette {
    Palette {
        background: color(0xfaf9f5),
        foreground: color(0x1f1e1d),
        border: color(0xd9d6cc),
        input: alpha(0x8a8579, 0.25),
        muted: color(0xf0eee6),
        muted_foreground: color(0x6e6a60),
        popover: color(0xffffff),
        popover_foreground: color(0x1f1e1d),
        accent: alpha(0xb85535, 0.12),
        accent_foreground: color(0x1f1e1d),
        primary: color(0xb85535),
        primary_hover: color(0xc96442),
        primary_active: color(0x9e4529),
        primary_foreground: color(0xffffff),
        secondary: color(0xf0eee6),
        secondary_hover: color(0xe8e4da),
        secondary_active: color(0xddd8cc),
        secondary_foreground: color(0x1f1e1d),
        selection: alpha(0xb85535, 0.16),
        ring: alpha(0xb85535, 0.45),
        sidebar: color(0xf0eee6),
        sidebar_foreground: color(0x1f1e1d),
        sidebar_border: color(0xe2ded3),
        sidebar_accent: alpha(0xb85535, 0.12),
        sidebar_accent_foreground: color(0x1f1e1d),
        sidebar_primary: color(0xb85535),
        sidebar_primary_foreground: color(0xffffff),
        title_bar: color(0xf0eee6),
        title_bar_border: color(0xe2ded3),
        link: color(0xb85535),
        link_hover: color(0xc96442),
        link_active: color(0x9e4529),
        success: color(0x3d7a55),
        success_foreground: color(0xffffff),
        warning: color(0xbe7814),
        warning_foreground: color(0x1f1e1d),
        danger: color(0xbf4d43),
        danger_foreground: color(0xffffff),
        caret: color(0xb85535),
        group_box: color(0xffffff),
        group_box_foreground: color(0x1f1e1d),
    }
}

fn claude_dark() -> Palette {
    Palette {
        background: color(0x262624),
        foreground: color(0xf0eee6),
        border: color(0x444340),
        input: alpha(0xf0eee6, 0.16),
        muted: color(0x3a3937),
        muted_foreground: color(0xb7b5ac),
        popover: color(0x30302e),
        popover_foreground: color(0xf0eee6),
        accent: alpha(0xd97757, 0.18),
        accent_foreground: color(0xf0eee6),
        primary: color(0xd97757),
        primary_hover: color(0xe08a6a),
        primary_active: color(0xc15f3c),
        primary_foreground: color(0x1f1e1d),
        secondary: color(0x3a3937),
        secondary_hover: color(0x444340),
        secondary_active: color(0x30302e),
        secondary_foreground: color(0xf0eee6),
        selection: alpha(0xd97757, 0.28),
        ring: alpha(0xd97757, 0.55),
        sidebar: color(0x1f1e1d),
        sidebar_foreground: color(0xf0eee6),
        sidebar_border: color(0x3a3937),
        sidebar_accent: alpha(0xd97757, 0.18),
        sidebar_accent_foreground: color(0xf0eee6),
        sidebar_primary: color(0xd97757),
        sidebar_primary_foreground: color(0x1f1e1d),
        title_bar: color(0x1f1e1d),
        title_bar_border: color(0x3a3937),
        link: color(0xe08a6a),
        link_hover: color(0xe8a184),
        link_active: color(0xd97757),
        success: color(0x7faf8b),
        success_foreground: color(0x1f1e1d),
        warning: color(0xe0a458),
        warning_foreground: color(0x1f1e1d),
        danger: color(0xe57368),
        danger_foreground: color(0x1f1e1d),
        caret: color(0xd97757),
        group_box: color(0x30302e),
        group_box_foreground: color(0xf0eee6),
    }
}

fn color(hex: u32) -> Hsla {
    rgba(hex << 8 | 0xff).into()
}

fn alpha(hex: u32, opacity: f32) -> Hsla {
    rgba(hex << 8 | ((opacity * 255.0) as u32)).into()
}

pub const fn native_font_family() -> &'static str {
    #[cfg(target_os = "macos")]
    {
        "SF Pro Text"
    }
    #[cfg(target_os = "windows")]
    {
        "Segoe UI"
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        "Noto Sans"
    }
}

pub fn reduced_transparency() -> bool {
    *REDUCED_TRANSPARENCY.get_or_init(|| {
        std::env::var_os("TACT_DESKTOP_REDUCED_TRANSPARENCY").is_some()
            || crate::platform::system_reduced_transparency()
    })
}

pub fn high_contrast() -> bool {
    *HIGH_CONTRAST.get_or_init(|| {
        std::env::var_os("TACT_DESKTOP_HIGH_CONTRAST").is_some()
            || crate::platform::system_high_contrast()
    })
}

pub fn scrim_color() -> Hsla {
    alpha(0x000000, scrim_opacity())
}

pub fn scrim_opacity() -> f32 {
    if reduced_transparency() { 0.62 } else { 0.35 }
}

pub fn border_width() -> f32 {
    if high_contrast() { 2.0 } else { 1.0 }
}

#[cfg(test)]
mod tests {
    use super::{Appearance, DesktopTheme, color, palette};

    #[test]
    fn contrast_and_transparency_helpers_have_solid_fallbacks() {
        assert!(super::scrim_opacity() >= 0.35);
        assert!(super::border_width() >= 1.0);
    }

    #[test]
    fn tact_palette_keeps_the_existing_apple_primary() {
        let light = palette(DesktopTheme::Tact, Appearance::Light);
        assert_eq!(light.primary, color(0x007aff));
        assert_eq!(light.background, color(0xf5f5f7));

        let dark = palette(DesktopTheme::Tact, Appearance::Dark);
        assert_eq!(dark.primary, color(0x0a84ff));
        assert_eq!(dark.background, color(0x1c1c1e));
    }

    #[test]
    fn claude_palette_uses_the_warm_cream_and_orange_pair() {
        let light = palette(DesktopTheme::Claude, Appearance::Light);
        assert_eq!(light.background, color(0xfaf9f5));
        assert_eq!(light.primary, color(0xb85535));

        let dark = palette(DesktopTheme::Claude, Appearance::Dark);
        assert_eq!(dark.background, color(0x262624));
        assert_eq!(dark.primary, color(0xd97757));
    }
}
