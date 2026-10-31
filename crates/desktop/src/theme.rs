use gpui_component::{Theme, ThemeMode};
use tact_desktop_core::Appearance;

use std::sync::OnceLock;

static REDUCED_TRANSPARENCY: OnceLock<bool> = OnceLock::new();
static HIGH_CONTRAST: OnceLock<bool> = OnceLock::new();

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
