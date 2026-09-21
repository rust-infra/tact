//! Headless agent session runtime shared by Tact front ends.
//!
//! `tact-session` owns everything between the protocol and a user interface:
//! it builds the main [`tact::Agent`], drives [`UserCommand`]s through
//! [`driver::run_command_loop_with_account`], and exposes the resulting
//! [`AgentUpdate`] stream. It is deliberately free of ratatui and GPUI types so
//! the TUI (`tact-ui`) and the desktop client (`tact-gui`) can share one
//! implementation instead of forking the driver.
//!
//! [`UserCommand`]: tact_protocol::UserCommand
//! [`AgentUpdate`]: tact_protocol::AgentUpdate

pub mod account;
pub mod builder;
pub mod driver;
pub mod history;
pub mod mcp_listing;
pub mod runtime;
pub mod session_actions;
pub mod sessions;
pub mod test_support;
pub mod user_message;

pub use history::{HistoryBlock, HistoryMessage, HistoryRole};
pub use runtime::{SessionOptions, SessionRuntime};
pub use session_actions::{duplicate, rename, reveal, set_archived};
pub use sessions::RecentSession;
