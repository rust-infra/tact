//! Tact desktop client.
//!
//! The window shell is composed from `gpui-kit` components and driven by the
//! headless `tact-session` runtime. The agent loop, providers, and protocol live
//! in the headless crates; nothing here may pull GPUI into them.

mod commands;
mod composer;
mod layout;
pub mod pane;
mod session;
pub mod shell;
mod terminal;
pub mod theme;
mod transcript;

pub use shell::{TactApp, Workspace};
pub use tact_session::RecentSession;

/// Register the desktop client's global actions and key bindings.
pub fn commands_init(cx: &mut gpui_kit::App) {
    commands::init(cx);
}
