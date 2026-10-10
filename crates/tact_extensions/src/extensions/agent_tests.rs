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

struct CapturingExecutor(Mutex<Option<tact_llm::Message>>);

#[async_trait]
impl AgentExecutor for CapturingExecutor {
    async fn run(
        &self,
        _context: InvocationContext,
        message: tact_llm::Message,
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
    let captured = executor
        .0
        .lock()
        .unwrap()
        .clone()
        .expect("executor captured the message");
    assert_eq!(captured.role, tact_llm::Role::User);
    assert_eq!(
        crate::extract_text(&captured.content),
        "hello from extension"
    );
}

/// The full-content form is accepted too, so a client can send blocks rather
/// than a bare string without the boundary narrowing it to text.
#[tokio::test]
async fn agent_run_capability_accepts_a_full_message_content() {
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
                RequestId::from("request-agent-content"),
                PluginId::from("tact.agent"),
                "test",
            ),
            json!({"content": {"content": [{"type": "text", "text": "hello blocks"}]}}),
        )
        .await
        .unwrap();

    assert_eq!(result, json!({"run_id": "run-agent-test"}));
    let captured = executor
        .0
        .lock()
        .unwrap()
        .clone()
        .expect("executor captured the message");
    assert_eq!(captured.role, tact_llm::Role::User);
    assert_eq!(crate::extract_text(&captured.content), "hello blocks");
}

/// `message` and `content` are alternatives, not a pair: both together is a
/// malformed request, not a merge.
#[tokio::test]
async fn agent_run_capability_rejects_message_and_content_together() {
    let runtime = runtime();
    AgentExtension::new(Arc::new(CapturingExecutor(Mutex::new(None))))
        .register(&runtime)
        .unwrap();

    let error = runtime
        .router()
        .invoke(
            "runs.start",
            runtime.invocation(
                RequestId::from("request-agent-both"),
                PluginId::from("tact.agent"),
                "test",
            ),
            json!({"message": "hi", "content": {"content": "also hi"}}),
        )
        .await
        .unwrap_err();

    assert_eq!(
        error.category(),
        tact_protocol::ErrorCategory::InvalidRequest
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

/// `runs.cancel` sets the executor's cancel flag for the requested run.
/// The `Cancelled` fact is not this capability's to emit — see the handler.
#[tokio::test]
async fn runs_cancel_capability_requests_cancellation() {
    struct CancellingExecutor(Mutex<Vec<Option<String>>>);

    #[async_trait]
    impl AgentExecutor for CancellingExecutor {
        async fn run(
            &self,
            _context: InvocationContext,
            _message: tact_llm::Message,
        ) -> Result<RunId, KernelError> {
            Ok(RunId::from("run-cancel-test"))
        }

        fn cancel(&self, run_id: Option<&RunId>) -> Result<(), KernelError> {
            self.0
                .lock()
                .unwrap()
                .push(run_id.map(|id| id.as_str().to_string()));
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
    assert_eq!(
        executor.0.lock().unwrap().as_slice(),
        [Some("run-cancel-test".to_string())]
    );
}

/// `runs.cancel` does not require a run id: a host cancels the in-flight run
/// and cannot supply its identity. An empty body still cancels and reports
/// `cancelled: true`.
#[tokio::test]
async fn runs_cancel_without_a_run_id_still_cancels() {
    struct RecordingExecutor(Mutex<Vec<Option<String>>>);

    #[async_trait]
    impl AgentExecutor for RecordingExecutor {
        async fn run(
            &self,
            _context: InvocationContext,
            _message: tact_llm::Message,
        ) -> Result<RunId, KernelError> {
            Ok(RunId::from("run-ignored"))
        }

        fn cancel(&self, run_id: Option<&RunId>) -> Result<(), KernelError> {
            self.0
                .lock()
                .unwrap()
                .push(run_id.map(|id| id.as_str().to_string()));
            Ok(())
        }
    }

    let executor = Arc::new(RecordingExecutor(Mutex::new(Vec::new())));
    let runtime = runtime();
    AgentExtension::new(executor.clone())
        .register(&runtime)
        .unwrap();
    let context = runtime.invocation(
        RequestId::from("request-cancel-no-id"),
        PluginId::from("tact.agent"),
        "tester",
    );

    let output = runtime
        .router()
        .invoke("runs.cancel", context, json!({}))
        .await
        .unwrap();
    assert_eq!(output, json!({"cancelled": true}));
    assert_eq!(
        executor.0.lock().unwrap().as_slice(),
        [None],
        "the executor is asked to cancel without a run id"
    );
}
