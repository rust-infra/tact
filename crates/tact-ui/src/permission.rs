//! Process-wide permission mode, re-exported from the shared session crate so
//! the TUI and the desktop client resolve the same value.

pub(crate) use tact_session::builder::permission_mode_from_config;
