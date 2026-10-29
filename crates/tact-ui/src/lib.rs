//! Library surface for `tact-ui` (binary + integration tests).

pub mod headless_session;
pub mod hooks_cli;
pub mod mcp_cli;
pub mod plugin_cli;
pub mod sessions;

mod headless;
mod headless_log;
mod interactive;

pub(crate) use tact_host::{account, session_bootstrap, user_message};
pub use tact_host::{driver, goal_runner, session_lock, test_support};

pub use headless::run_headless;
pub use interactive::run_interactive;
