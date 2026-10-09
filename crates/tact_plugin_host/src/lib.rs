//! Shared plugin-host machinery: lifecycle, transport, and supervision.
//!
//! A plugin host is a process (or an in-process registry) that speaks the
//! versioned Plugin Protocol over a line-oriented stdio transport. Everything
//! a host must do the same way — handshake, capability registration, request
//! correlation, timeouts, cancellation, crash detection, and shutdown drain —
//! lives here, so `tact_plugin_node` and `tact_plugin_wasm` are thin
//! language-specific layers rather than two copies of a supervision protocol.

mod host;
mod process;
mod runtime;
mod transport;

pub use host::PluginHost;
pub use process::PluginProcess;
pub use runtime::{HostCallService, StdioPluginHost};
pub use transport::make_envelope;

/// Default bound for a single host RPC, and the startup/shutdown budgets.
pub use runtime::{DEFAULT_REQUEST_TIMEOUT, SHUTDOWN_TIMEOUT, STARTUP_TIMEOUT};
