//! NSH views of the desktop surfaces. Pure decisions live in `crate::ui`.

pub mod composer;
pub mod inspector;
pub mod overlays;
pub mod rail;
pub mod reader;
pub mod surface;
pub mod title_bar;
pub mod trajectory;
pub mod transcript;

use nsh_base::theme;
use nsh_core::element::AdaptiveColor;

/// A theme token of the app's (`theme.json`), falling back to the foreground
/// so a missing token is visible rather than transparent.
pub(crate) fn token(name: &str) -> AdaptiveColor {
    theme().color(name).unwrap_or_else(|| theme().foreground)
}

pub(crate) const RADIUS: f32 = 10.0;
