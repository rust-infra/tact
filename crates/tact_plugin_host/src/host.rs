//! The lifecycle boundary every plugin host implements.

use std::time::Duration;

use async_trait::async_trait;
use tact::{InvocationContext, KernelError, PluginState};
use tact_protocol::{
    CapabilityDeclaration, ErrorCategory, PluginId, PluginRequest, PluginResponse, ProtocolVersion,
};

use crate::runtime::DEFAULT_REQUEST_TIMEOUT;

/// Common lifecycle boundary implemented by external and in-process hosts.
///
/// The Kernel owns the *registry* of what is registered; this trait is the
/// runtime half — start, answer, stop — so the Node, WASM, and in-process Rust
/// hosts are interchangeable to anything that supervises them.
#[async_trait]
pub trait PluginHost: Send {
    fn plugin_id(&self) -> &PluginId;
    fn protocol(&self) -> ProtocolVersion;
    fn capabilities(&self) -> &[CapabilityDeclaration];
    fn state(&self) -> PluginState;
    async fn request(&mut self, request: PluginRequest) -> anyhow::Result<PluginResponse>;

    /// The bound for one router-mediated request, used to default the caller's
    /// deadline when it carries none. A host with its own negotiated timeout
    /// overrides it so the router and the host agree on how long a call may run.
    fn request_timeout(&self) -> Duration {
        DEFAULT_REQUEST_TIMEOUT
    }

    /// A request on behalf of a specific caller, carrying the caller's
    /// correlation and deadline.
    ///
    /// Distinct from [`Self::request`] deliberately: `request` is the raw host
    /// API and refuses a bare `Invoke`, so a caller cannot bypass the
    /// permissioned router by calling the host directly. This is the one entry
    /// the router-installed handler is allowed to take. The default forwards to
    /// `request`, which is correct for a host whose `request` already routes an
    /// invocation internally (in-process or a test double); a stdio host
    /// overrides it to put the context on the wire.
    async fn request_with_context(
        &mut self,
        _context: &InvocationContext,
        request: PluginRequest,
    ) -> Result<PluginResponse, KernelError> {
        let plugin_id = self.plugin_id().clone();
        self.request(request).await.map_err(|error| {
            KernelError::new(
                ErrorCategory::PluginCrashed,
                error.to_string(),
                "plugin_host",
                true,
            )
            .with_plugin_id(plugin_id)
        })
    }

    /// Abandons the current request: a stdio host terminates the process so a
    /// late response cannot answer the next call. The default is a no-op for a
    /// host with nothing to interrupt.
    async fn interrupt(&mut self) {}

    async fn shutdown(&mut self) -> anyhow::Result<()>;
}
