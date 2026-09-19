//! Library surface for `tact-ui` (binary + integration tests).
//!
//! The agent session runtime (driver, account service, agent construction) now
//! lives in the headless `tact-session` crate so the GPUI desktop client can
//! share it without depending on ratatui. The items below stay re-exported at
//! their historical `tact_ui::…` paths.

pub use tact_session::{driver, test_support};

pub mod headless_session;
pub mod mcp_cli;
pub mod plugin_cli;
pub mod session_lock;
pub mod sessions;

mod headless;
mod interactive;
mod permission;

pub use headless::run_headless;
pub use interactive::run_interactive;
