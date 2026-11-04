//! NSH views of the desktop surfaces. Pure decisions live in `crate::ui`.

pub mod composer;
pub mod gate_banner;
pub mod inspector;
pub mod menu;
pub mod overlays;
pub mod rail;
pub mod reader;
pub mod surface;
pub mod title_bar;
pub mod toast;
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

/// One of the app's own menu bar menus.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuKind {
    File,
    View,
    Session,
}

impl MenuKind {
    pub const fn label(self) -> &'static str {
        match self {
            Self::File => "File",
            Self::View => "View",
            Self::Session => "Session",
        }
    }
}
