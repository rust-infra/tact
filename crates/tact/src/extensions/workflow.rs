//! Built-in multi-step workflow extension manifest.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;
use tact_protocol::{
    CapabilityDeclaration, CapabilityKind, CapabilityRisk, PluginId, ProtocolVersion,
};

use crate::{
    kernel::{
        CapabilityHandler, CapabilityRegistration, InvocationContext, KernelError, RuntimeContext,
    },
    plugin::RuntimePluginManifest,
};

pub struct WorkflowExtension {
    executor: Arc<dyn WorkflowExecutor>,
}

impl WorkflowExtension {
    #[must_use]
    pub fn new(executor: Arc<dyn WorkflowExecutor>) -> Self {
        Self { executor }
    }

    pub fn register(&self, runtime: &RuntimeContext) -> Result<(), KernelError> {
        let declaration = manifest()
            .capabilities
            .into_iter()
            .next()
            .expect("Workflow declares its run command");
        let handler: Arc<dyn CapabilityHandler> = Arc::new(WorkflowRunHandler {
            executor: self.executor.clone(),
        });
        runtime
            .router()
            .register(CapabilityRegistration::new(declaration, handler))
    }
}

#[async_trait]
pub trait WorkflowExecutor: Send + Sync {
    async fn run(&self, context: InvocationContext, input: Value) -> Result<Value, KernelError>;
}

struct WorkflowRunHandler {
    executor: Arc<dyn WorkflowExecutor>,
}

#[async_trait]
impl CapabilityHandler for WorkflowRunHandler {
    async fn invoke(&self, context: InvocationContext, input: Value) -> Result<Value, KernelError> {
        self.executor.run(context, input).await
    }
}

pub fn manifest() -> RuntimePluginManifest {
    RuntimePluginManifest {
        id: PluginId::from("tact.workflow"),
        version: env!("CARGO_PKG_VERSION").into(),
        protocol: ProtocolVersion::CURRENT,
        capabilities: vec![CapabilityDeclaration {
            name: "workflow.run".into(),
            kind: CapabilityKind::Command,
            version: "1".into(),
            description: Some("Run a multi-step workflow".into()),
            input_schema: None,
            output_schema: None,
            risk: CapabilityRisk::Medium,
        }],
    }
}
