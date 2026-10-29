//! Ergonomic facade over the provider-neutral Tact agent kernel.
//!
//! The SDK supplies conservative defaults, a static tool composition layer,
//! session ownership, and optional tracing instrumentation. It is not the full
//! Tact product and does not re-export a model provider.

mod agent;
mod builder;
mod config;
mod control;
mod defaults;
mod error;
mod run_id;
mod session;
mod toolset;

#[cfg(feature = "telemetry")]
mod telemetry;

pub use agent::Agent;
pub use async_trait::async_trait;
pub use builder::AgentBuilder;
pub(crate) use config::AgentConfig;
pub use control::SessionControl;
pub use defaults::{
    AllowAllGate, BasicContextPolicy, CallbackEventPort, ContextCallbackEventPort, DenyAllGate,
    EmptyTools, NoopEventPort, NoopTranscriptStore, NoopUsageSink,
};
pub use error::{AgentBuildError, ToolSetError};
pub use futures_util;
pub use run_id::{RunIdSource, UuidRunIdSource};
pub use serde_json;
pub use session::AgentSession;
pub use tact_agent_api;
pub use tact_agent_core::KernelError;
pub use toolset::{ToolHandler, ToolProvider, ToolSet};

/// Common imports for hosts implementing model and tool adapters.
pub mod prelude {
    pub use async_trait::async_trait;
    pub use futures_util;
    pub use serde_json;
    pub use tact_agent_api::*;
    pub use tact_agent_core::KernelError;

    pub use crate::{
        Agent, AgentBuildError, AgentBuilder, AgentSession, AllowAllGate, BasicContextPolicy,
        CallbackEventPort, ContextCallbackEventPort, DenyAllGate, EmptyTools, NoopEventPort,
        NoopTranscriptStore, NoopUsageSink, RunIdSource, SessionControl, ToolHandler, ToolProvider,
        ToolSet, ToolSetError, UuidRunIdSource,
    };
}

#[cfg(feature = "telemetry")]
pub use telemetry::TelemetryConfig;

#[cfg(not(feature = "telemetry"))]
mod telemetry_config;
#[cfg(not(feature = "telemetry"))]
pub use telemetry_config::TelemetryConfig;
