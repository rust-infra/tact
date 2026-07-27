//! Library surface for `tact-ui` (binary + integration tests).

pub mod driver;
pub mod headless_session;
pub mod plugin_cli;
pub mod session_lock;
pub mod sessions;
pub mod test_support;

mod account;
mod headless;
mod image_attach;
mod interactive;
mod permission;
mod user_message;

pub use headless::run_headless;
pub use interactive::run_interactive;
