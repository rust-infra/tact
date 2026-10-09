use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde_json::{Value, json};
use tact_protocol::{PluginId, RequestId, RunId};

use tact::{InvocationContext, KernelError, PermissionService, RuntimeContext, RuntimeServices};

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
        tact::CapabilityRouter::new(),
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
            "runs.start",
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
            "runs.start",
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

/// `runs.cancel` sets the executor's cancel flag and records a `Cancelled`
/// event for the requested run.
#[tokio::test]
async fn runs_cancel_capability_requests_cancellation() {
    struct CancellingExecutor(Mutex<Vec<String>>);

    #[async_trait]
    impl AgentExecutor for CancellingExecutor {
        async fn run(
            &self,
            _context: InvocationContext,
            _message: String,
        ) -> Result<RunId, KernelError> {
            Ok(RunId::from("run-cancel-test"))
        }

        fn cancel(&self, run_id: &RunId) -> Result<(), KernelError> {
            self.0.lock().unwrap().push(run_id.as_str().to_string());
            Ok(())
        }
    }

    let executor = Arc::new(CancellingExecutor(Mutex::new(Vec::new())));
    let runtime = runtime();
    AgentExtension::new(executor.clone())
        .register(&runtime)
        .unwrap();
    let context = runtime.invocation(
        RequestId::from("request-cancel-test"),
        PluginId::from("tact.agent"),
        "tester",
    );

    let output = runtime
        .router()
        .invoke("runs.cancel", context, json!({"run_id": "run-cancel-test"}))
        .await
        .unwrap();
    assert_eq!(output, json!({"cancelled": true}));
    assert_eq!(executor.0.lock().unwrap().as_slice(), ["run-cancel-test"]);
}
