use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde_json::{Value, json};
use tact_protocol::{PluginId, RequestId, RunId};

use crate::kernel::{
    InvocationContext, KernelError, PermissionService, RuntimeContext, RuntimeServices,
};

use super::agent::{AgentExecutor, AgentExtension};

struct AllowAll;

#[async_trait]
impl PermissionService for AllowAll {
    async fn check(
        &self,
        _declaration: &tact_protocol::CapabilityDeclaration,
        _context: &InvocationContext,
        _input: &Value,
    ) -> Result<(), KernelError> {
        Ok(())
    }
}

struct CapturingExecutor(Mutex<Option<String>>);

#[async_trait]
impl AgentExecutor for CapturingExecutor {
    async fn run(
        &self,
        _context: InvocationContext,
        message: String,
    ) -> Result<RunId, KernelError> {
        *self.0.lock().unwrap() = Some(message);
        Ok(RunId::from("run-agent-test"))
    }
}

fn runtime() -> RuntimeContext {
    RuntimeContext::with_services(
        crate::kernel::CapabilityRouter::new(),
        RuntimeServices::with_permission(Arc::new(AllowAll)),
    )
}

#[tokio::test]
async fn agent_run_capability_forwards_message_and_returns_run_id() {
    let runtime = runtime();
    let executor = Arc::new(CapturingExecutor(Mutex::new(None)));
    AgentExtension::new(executor.clone())
        .register(&runtime)
        .unwrap();

    let result = runtime
        .router()
        .invoke(
            "agent.run",
            runtime.invocation(
                RequestId::from("request-agent-test"),
                PluginId::from("tact.agent"),
                "test",
            ),
            json!({"message": "hello from extension"}),
        )
        .await
        .unwrap();

    assert_eq!(result, json!({"run_id": "run-agent-test"}));
    assert_eq!(
        executor.0.lock().unwrap().as_deref(),
        Some("hello from extension")
    );
}

#[tokio::test]
async fn agent_run_capability_rejects_missing_message() {
    let runtime = runtime();
    AgentExtension::new(Arc::new(CapturingExecutor(Mutex::new(None))))
        .register(&runtime)
        .unwrap();

    let error = runtime
        .router()
        .invoke(
            "agent.run",
            runtime.invocation(
                RequestId::from("request-agent-invalid"),
                PluginId::from("tact.agent"),
                "test",
            ),
            json!({}),
        )
        .await
        .unwrap_err();

    assert_eq!(
        error.category(),
        tact_protocol::ErrorCategory::InvalidRequest
    );
}
