use gpui::{Hsla, px, rgba};
use gpui_component::{Theme, ThemeMode};
use tact_desktop_core::Appearance;

use std::sync::OnceLock;

static REDUCED_TRANSPARENCY: OnceLock<bool> = OnceLock::new();
static HIGH_CONTRAST: OnceLock<bool> = OnceLock::new();

/// Tact Desktop uses the macOS system palette from the design prototype rather
/// than the neutral grayscale defaults of the component library. The shell and
/// the embedded components share one palette so a Tact row and a `gpui-ai`
/// surface cannot drift apart between Light and Dark.
pub fn apply(appearance: Appearance, window: Option<&mut gpui::Window>, cx: &mut gpui::App) {
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
    apply_palette(theme, matches!(appearance, Appearance::Dark));
    Theme::sync_base(cx);
}

fn apply_palette(theme: &mut Theme, dark: bool) {
    if dark {
        theme.background = color(0x1c1c1e);
        theme.foreground = color(0xf5f5f7);
        theme.border = alpha(0xffffff, 0.10);
        theme.input = alpha(0xffffff, 0.16);
        theme.muted = color(0x2c2c2e);
        theme.muted_foreground = color(0x98989d);
        theme.popover = color(0x2c2c2e);
        theme.popover_foreground = color(0xf5f5f7);
        theme.accent = alpha(0x0a84ff, 0.18);
        theme.accent_foreground = color(0xf5f5f7);
        theme.primary = color(0x0a84ff);
        theme.primary_hover = color(0x409cff);
        theme.primary_active = color(0x0063ce);
        theme.primary_foreground = color(0xffffff);
        theme.secondary = color(0x3a3a3c);
        theme.secondary_hover = color(0x48484a);
        theme.secondary_active = color(0x2c2c2e);
        theme.secondary_foreground = color(0xf5f5f7);
        theme.selection = alpha(0x0a84ff, 0.28);
        theme.ring = alpha(0x0a84ff, 0.55);
        theme.sidebar = color(0x242426);
        theme.sidebar_foreground = color(0xf5f5f7);
        theme.sidebar_border = alpha(0xffffff, 0.08);
        theme.sidebar_accent = alpha(0x0a84ff, 0.22);
        theme.sidebar_accent_foreground = color(0xf5f5f7);
        theme.sidebar_primary = color(0x0a84ff);
        theme.sidebar_primary_foreground = color(0xffffff);
        theme.title_bar = color(0x2c2c2e);
        theme.title_bar_border = alpha(0xffffff, 0.08);
        theme.link = color(0x409cff);
        theme.link_hover = color(0x64b5ff);
        theme.link_active = color(0x0a84ff);
        theme.success = color(0x30d158);
        theme.success_foreground = color(0xffffff);
        theme.warning = color(0xff9f0a);
        theme.warning_foreground = color(0x1c1c1e);
        theme.danger = color(0xff453a);
        theme.danger_foreground = color(0xffffff);
        theme.caret = color(0x0a84ff);
        theme.group_box = color(0x242426);
        theme.group_box_foreground = color(0xf5f5f7);
    } else {
        theme.background = color(0xf5f5f7);
        theme.foreground = color(0x1d1d1f);
        theme.border = alpha(0x3c3c43, 0.14);
        theme.input = alpha(0x3c3c43, 0.20);
        theme.muted = color(0xececf1);
        theme.muted_foreground = color(0x6e6e73);
        theme.popover = color(0xffffff);
        theme.popover_foreground = color(0x1d1d1f);
        theme.accent = alpha(0x007aff, 0.12);
        theme.accent_foreground = color(0x1d1d1f);
        theme.primary = color(0x007aff);
        theme.primary_hover = color(0x3395ff);
        theme.primary_active = color(0x0063ce);
        theme.primary_foreground = color(0xffffff);
        theme.secondary = color(0xececf1);
        theme.secondary_hover = color(0xe2e2e8);
        theme.secondary_active = color(0xd7d7de);
        theme.secondary_foreground = color(0x1d1d1f);
        theme.selection = alpha(0x007aff, 0.16);
        theme.ring = alpha(0x007aff, 0.45);
        theme.sidebar = color(0xf2f2f7);
        theme.sidebar_foreground = color(0x1d1d1f);
        theme.sidebar_border = alpha(0x3c3c43, 0.12);
        theme.sidebar_accent = alpha(0x007aff, 0.12);
        theme.sidebar_accent_foreground = color(0x1d1d1f);
        theme.sidebar_primary = color(0x007aff);
        theme.sidebar_primary_foreground = color(0xffffff);
        theme.title_bar = color(0xffffff);
        theme.title_bar_border = alpha(0x3c3c43, 0.12);
        theme.link = color(0x007aff);
        theme.link_hover = color(0x0063ce);
        theme.link_active = color(0x004999);
        theme.success = color(0x34c759);
        theme.success_foreground = color(0xffffff);
        theme.warning = color(0xff9f0a);
        theme.warning_foreground = color(0x1d1d1f);
        theme.danger = color(0xff453a);
        theme.danger_foreground = color(0xffffff);
        theme.caret = color(0x007aff);
        theme.group_box = color(0xffffff);
        theme.group_box_foreground = color(0x1d1d1f);
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
    #[test]
    fn contrast_and_transparency_helpers_have_solid_fallbacks() {
        assert!(super::scrim_opacity() >= 0.35);
        assert!(super::border_width() >= 1.0);
    }
}
