//! Library surface for `tact-ui` (binary + integration tests).

pub mod driver;
pub mod session_lock;
pub mod sessions;
pub mod test_support;

mod headless;
mod interactive;
mod permission;
mod user_message;

pub use headless::run_headless;
pub use interactive::run_interactive;
