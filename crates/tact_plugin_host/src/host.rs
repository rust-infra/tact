//! The lifecycle boundary every plugin host implements.

use async_trait::async_trait;
use tact::PluginState;
use tact_protocol::{
    CapabilityDeclaration, PluginId, PluginRequest, PluginResponse, ProtocolVersion,
};

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
    async fn shutdown(&mut self) -> anyhow::Result<()>;
}
