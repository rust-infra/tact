//! Node.js plugin host.
//!
//! A Node plugin is a stdio child that speaks the versioned Plugin Protocol.
//! The supervision protocol — handshake, capability registration, request
//! correlation, timeouts, cancellation, crash detection, shutdown drain — is
//! shared in `tact_plugin_host`; this crate is the Node.js entry point, so a
//! Node host and a WASM host are the same machine with a different runner.

pub use tact_plugin_host::{
    HostCallService, PluginHost, PluginProcess, StdioPluginHost, make_envelope,
};

/// The Node.js plugin host. Alias rather than a wrapper: a Node plugin is a
/// plain stdio child, so wrapping would only duplicate the shared API.
pub use tact_plugin_host::StdioPluginHost as NodePluginHost;

/// The program a Node plugin is started with when the caller does not name one.
#[must_use]
pub fn node_program() -> std::path::PathBuf {
    std::path::PathBuf::from("node")
}
