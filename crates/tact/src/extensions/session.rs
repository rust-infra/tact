//! Session extension entry points over the Runtime capability protocol.

use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use tact_protocol::{
    CapabilityDeclaration, CapabilityKind, CapabilityRisk, PluginId, ProtocolVersion,
};

use crate::{
    kernel::{
        CapabilityHandler, CapabilityRegistration, InvocationContext, KernelError, RuntimeContext,
    },
    plugin::RuntimePluginManifest,
    store::DynSessionStore,
};

#[derive(Clone)]
pub struct SessionExtension {
    store: DynSessionStore,
}

impl SessionExtension {
    #[must_use]
    pub fn new(store: DynSessionStore) -> Self {
        Self { store }
    }

    pub fn register(&self, runtime: &RuntimeContext) -> Result<(), KernelError> {
        runtime.router().register(CapabilityRegistration::new(
            read_capability(),
            Arc::new(SessionReadHandler {
                store: self.store.clone(),
            }),
        ))
    }
}

struct SessionReadHandler {
    store: DynSessionStore,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SessionReadInput {
    session_id: String,
}

#[async_trait]
impl CapabilityHandler for SessionReadHandler {
    async fn invoke(&self, context: InvocationContext, input: Value) -> Result<Value, KernelError> {
        let request: SessionReadInput = serde_json::from_value(input).map_err(|error| {
            KernelError::new(
                tact_protocol::ErrorCategory::InvalidRequest,
                format!("invalid sessions.read request: {error}"),
                "session",
                false,
            )
        })?;
        if request.session_id.trim().is_empty() {
            return Err(KernelError::new(
                tact_protocol::ErrorCategory::InvalidRequest,
                "session_id cannot be empty",
                "session",
                false,
            ));
        }
        if context
            .session_id()
            .is_none_or(|session_id| session_id.as_str() != request.session_id)
        {
            return Err(KernelError::permission_denied(
                "sessions.read may only read the invocation's session",
            ));
        }
        let messages = self
            .store
            .load_session(&request.session_id)
            .await
            .map_err(|error| {
                KernelError::new(
                    tact_protocol::ErrorCategory::StorageError,
                    error.to_string(),
                    "session",
                    true,
                )
            })?;
        serde_json::to_value(messages)
            .map(|messages| json!({"messages": messages}))
            .map_err(|error| {
                KernelError::new(
                    tact_protocol::ErrorCategory::InternalError,
                    error.to_string(),
                    "session",
                    false,
                )
            })
    }
}

fn read_capability() -> CapabilityDeclaration {
    CapabilityDeclaration {
        name: "sessions.read".into(),
        kind: CapabilityKind::Service,
        version: "1".into(),
        description: Some("Read persisted session messages".into()),
        input_schema: None,
        output_schema: None,
        risk: CapabilityRisk::ReadOnly,
    }
}

pub fn manifest() -> RuntimePluginManifest {
    RuntimePluginManifest {
        id: PluginId::from("tact.session"),
        version: env!("CARGO_PKG_VERSION").into(),
        protocol: ProtocolVersion::CURRENT,
        capabilities: vec![read_capability()],
    }
}
