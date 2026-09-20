//! Theme wiring for the Tact desktop shell.
//!
//! Two themes ship in `themes/tact-anthropic.json`: `Tact Anthropic Light` and
//! `Tact Anthropic Dark`. `docs/design/tact-desktop-theme.json` is the design
//! source of truth; the copy this crate watches carries the same content so a
//! running window and the reviewed reference cannot drift apart.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

use gpui_kit::SharedString;
use gpui_kit::component::{Edges, Theme, ThemeMode, ThemeRegistry};
use gpui_kit::{Anchor, App, Window, px};

/// Light theme name declared in `themes/tact-anthropic.json`.
pub const LIGHT_THEME_NAME: &str = "Tact Anthropic Light";

/// Dark theme name declared in `themes/tact-anthropic.json`.
pub const DARK_THEME_NAME: &str = "Tact Anthropic Dark";

/// Directory of theme files the registry watches.
///
/// `TACT_GUI_THEMES` overrides the location for packaged builds. Without it the
/// directory beside this crate's manifest is used, so the application does not
/// depend on the process working directory.
pub fn theme_dir() -> PathBuf {
    std::env::var_os("TACT_GUI_THEMES")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("themes"))
}

/// Adopt the Tact light and dark configs from the registry, then activate `mode`.
///
/// The registry reads theme files asynchronously, so this reports an error
/// until `themes/tact-anthropic.json` has been loaded; the built-in default
/// theme stays in place meanwhile.
pub fn activate(mode: ThemeMode, window: Option<&mut Window>, cx: &mut App) -> anyhow::Result<()> {
    let (light, dark) = {
        let registry = ThemeRegistry::global(cx);
        let light = registry
            .themes()
            .get(&SharedString::from(LIGHT_THEME_NAME))
            .cloned();
        let dark = registry
            .themes()
            .get(&SharedString::from(DARK_THEME_NAME))
            .cloned();

        match (light, dark) {
            (Some(light), Some(dark)) => (light, dark),
            _ => anyhow::bail!(
                "{LIGHT_THEME_NAME:?} and {DARK_THEME_NAME:?} are not loaded from {}",
                theme_dir().display()
            ),
        }
    };

    let theme = Theme::global_mut(cx);
    theme.light_theme = light;
    theme.dark_theme = dark;
    // The prototype's `.toast` is pinned 20px from the bottom-right corner;
    // the component default is the top-right, under the title bar.
    theme.notification.placement = Anchor::BottomRight;
    theme.notification.margins = Edges {
        top: px(20.),
        right: px(20.),
        bottom: px(20.),
        left: px(20.),
    };
    Theme::change(mode, window, cx);
    Ok(())
}

/// Whether the Tact themes have been adopted at least once.
///
/// The registry's first load races window creation, so `reapply` cannot trust
/// `Theme::global(cx).mode` yet: it would adopt whichever mode the built-in
/// default happened to set. The first successful activation pins the intended
/// default instead; later reloads preserve the user's live choice.
static ACTIVATED: AtomicBool = AtomicBool::new(false);

/// Re-activate the Tact themes without changing the current light/dark choice.
///
/// The registry calls this after a theme file changes on disk, so an edit
/// repaints open windows instead of being ignored until the next launch. On the
/// very first load it adopts [`default_mode`] so a fresh client always opens
/// dark regardless of how the built-in theme was configured.
pub fn reapply(cx: &mut App) {
    let first_load = !ACTIVATED.swap(true, Ordering::SeqCst);
    let mode = if first_load {
        default_mode()
    } else {
        Theme::global(cx).mode
    };

    match activate(mode, None, cx) {
        Ok(()) => {
            tracing::info!("Tact theme active: {}", Theme::global(cx).theme_name());
            cx.refresh_windows();
        }
        Err(err) => {
            ACTIVATED.store(false, Ordering::SeqCst);
            tracing::warn!("Tact theme is unavailable: {err:#}");
        }
    }
}

/// The mode a fresh window starts in.
///
/// The approved prototype is the dark Direction A board, so the desktop client
/// opens dark for a like-for-like first impression; the toggle still switches.
pub fn default_mode() -> ThemeMode {
    ThemeMode::Dark
}

/// Activate the default theme for a new window.
///
/// The registry loads theme files asynchronously, so the first attempt can
/// legitimately fail; the built-in theme stays in place and the registry's
/// reload callback activates Tact once the files arrive.
pub fn activate_default(window: &mut Window, cx: &mut App) {
    if let Err(err) = activate(default_mode(), Some(window), cx) {
        tracing::warn!("Tact theme is not ready yet: {err:#}");
    }
}

/// Switch between the Tact light and dark themes.
pub fn toggle(window: &mut Window, cx: &mut App) {
    let mode = if Theme::global(cx).mode.is_dark() {
        ThemeMode::Light
    } else {
        ThemeMode::Dark
    };

    if let Err(err) = activate(mode, Some(window), cx) {
        tracing::warn!("Cannot switch the Tact theme: {err:#}");
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::fs;

    use super::{DARK_THEME_NAME, LIGHT_THEME_NAME, theme_dir};

    /// The reviewed design source. `themes/tact-anthropic.json` is a copy of it
    /// so a running window cannot drift from the reference that was signed off.
    const DESIGN_THEME: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../docs/design/tact-desktop-theme.json"
    );

    /// The HTML prototype the theme's colours are lifted from.
    const DESIGN_PROTOTYPE: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../docs/design/tact-desktop-prototype.html"
    );

    /// The prototype's CSS custom properties for one `:root` block.
    fn prototype_vars(html: &str, selector: &str) -> BTreeMap<String, String> {
        let start = html
            .find(selector)
            .unwrap_or_else(|| panic!("{selector} is missing from the prototype"));
        let open = start + html[start..].find('{').expect("open brace");
        let close = open + html[open..].find('}').expect("close brace");
        html[open + 1..close]
            .split(';')
            .filter_map(|declaration| declaration.split_once(':'))
            .map(|(name, value)| (name.trim().to_string(), value.trim().to_lowercase()))
            .filter(|(name, _)| name.starts_with("--"))
            .collect()
    }

    /// One named theme's `colors` table, lower-cased for hex comparison.
    fn theme_colors(name: &str) -> BTreeMap<String, String> {
        let raw = fs::read_to_string(theme_dir().join("tact-anthropic.json"))
            .expect("the shipped theme file");
        let json: serde_json::Value = serde_json::from_str(&raw).expect("theme JSON");
        let theme = json["themes"]
            .as_array()
            .expect("themes array")
            .iter()
            .find(|theme| theme["name"] == name)
            .unwrap_or_else(|| panic!("{name} is missing from the theme file"));
        theme["colors"]
            .as_object()
            .expect("colors table")
            .iter()
            .map(|(key, value)| {
                (
                    key.clone(),
                    value.as_str().expect("colour string").to_lowercase(),
                )
            })
            .collect()
    }

    #[test]
    fn the_shipped_theme_copy_matches_the_design_source() {
        let shipped = fs::read_to_string(theme_dir().join("tact-anthropic.json"))
            .expect("the shipped theme file");
        let design = fs::read_to_string(DESIGN_THEME).expect("the design theme file");
        assert_eq!(
            shipped, design,
            "themes/tact-anthropic.json must stay a byte-for-byte copy of \
             docs/design/tact-desktop-theme.json"
        );
    }

    /// The prototype names its colours by role; the theme names them by widget.
    /// Pinning the translation here keeps every surface on the prototype's
    /// palette instead of on a lookalike picked by eye.
    #[test]
    fn the_theme_roles_carry_the_prototype_variables() {
        let html = fs::read_to_string(DESIGN_PROTOTYPE).expect("the prototype");
        let light = prototype_vars(&html, ":root{");
        let dark = prototype_vars(&html, ":root[data-theme=dark]{");

        let roles = [
            // The colour behind the window.
            ("background", "--page"),
            // The chrome columns: sidebar, title bar, status bar, work pane.
            ("sidebar.background", "--canvas"),
            // The main column: transcript, composer, and every card.
            ("popover.background", "--surface"),
            // The inset wells: search fields, chips, diff file headers.
            ("muted.background", "--surface2"),
            // Hover and selection fills.
            ("accent.background", "--hover"),
        ];

        for (name, vars) in [(LIGHT_THEME_NAME, &light), (DARK_THEME_NAME, &dark)] {
            let colors = theme_colors(name);
            for (token, var) in roles {
                let token_value = colors
                    .get(token)
                    .unwrap_or_else(|| panic!("{name} is missing the {token} token"));
                let var_value = vars
                    .get(var)
                    .unwrap_or_else(|| panic!("the prototype is missing {var}"));
                assert_eq!(
                    token_value, var_value,
                    "{name}: {token} must carry the prototype's {var}"
                );
            }
        }
    }
}
