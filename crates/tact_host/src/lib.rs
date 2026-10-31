//! Reusable host services for frontends.
//!
//! This crate owns the process/session orchestration that is independent of a
//! terminal renderer: command dispatch, session bootstrap and locking, Goal
//! execution, account polling, permission mapping, and input normalization.
//! `tact-ui` is one adapter over these services; desktop and embedded hosts can
//! provide another without depending on Ratatui or Crossterm.

use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

pub mod account;
pub mod driver;
pub mod extensions;
pub mod goal_runner;
pub mod host_session;
pub mod permission;
pub mod rendering;
pub mod session_bootstrap;
pub mod session_lock;
pub mod sessions;
#[cfg(any(test, feature = "test-support"))]
pub mod test_support;
pub mod user_message;

pub use anyhow::{self, Result};
pub use driver::{run_command_loop, run_command_loop_with_account};
pub use extensions::{
    ExtensionSnapshot, HostExtensionError, HostExtensionService, MarketplaceSnapshot, PluginAction,
    PluginSnapshot, SkillSnapshot,
};
pub use goal_runner::{GoalLimits, GoalRunner};
pub use host_session::{
    HostCloseError, HostEvent, HostExit, HostFailure, HostFrontendChannels, HostSendError,
    HostSession, HostSessionConfig, HostStartError, TryHostEvent,
};
pub use session_bootstrap::{
    Frontend, Notices, SessionSelection, UiWiring, bootstrap_session, open_session,
    open_session_selected,
};
pub use session_lock::{SessionLockGuard, SessionLockRegistry};
pub use tact::{
    Agent,
    config::{CliArgs, init_from_args as init_config},
    consts::TactPath,
    skill::{SharedSkillRegistry, shared_skill_registry},
    store::DynSessionStore,
    ui_responder::UiResponder,
};
pub use tact_protocol::{
    AccountUpdate, AgentUpdate, UpdateReceiver, UpdateSender, UserCommand, update_channel,
};

/// Create the command channel a frontend uses to drive one host session.
pub fn command_channel() -> (UnboundedSender<UserCommand>, UnboundedReceiver<UserCommand>) {
    tokio::sync::mpsc::unbounded_channel()
}
