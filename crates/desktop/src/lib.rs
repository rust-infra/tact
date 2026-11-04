//! Tact Desktop on Native Shell: the app's Rust UI, its root component, and
//! the bridge to `tact_desktop_core`.

pub mod app;
mod assets;
pub mod i18n;
mod shell;
pub mod ui;
pub mod views;

use nsh_core::prelude::*;

/// The app's UI set (`nsh.json` `"rust"`): what a TSX entry may import from
/// `tact-desktop/native`.
#[native_module("tact-desktop/native")]
fn native(registry: &mut Registry) {
    registry.register::<App>();
    registry.assets(assets!());
}

/// The application's root component.
#[component]
pub fn App() -> impl IntoElement {
    app::install();
    shell::shell()
}
