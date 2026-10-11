//! Built-in multi-step workflow extension manifest.

use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use tact_protocol::{
    CapabilityDeclaration, CapabilityKind, CapabilityRisk, ErrorCategory, PluginId,
    ProtocolVersion, RequestId, RunId,
};

use crate::plugin::RuntimePluginManifest;
use tact::{
    CapabilityHandler, CapabilityRegistration, InvocationContext, KernelError, RuntimeContext,
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

/// The production [`WorkflowExecutor`]: a workflow is a sequence of chat turns.
///
/// Each step submits its `prompt` through `chat.submit` on the session's
/// serving router, so a step gets the full turn semantics — message assembly,
/// the Stop-hook continuation loop, `TaskComplete` and the TaskCompleted hooks —
/// and every hop stays permission-checked and trajectory-recorded. Workflow is a
/// choreographer, not a second run entry.
///
/// The runtime is stored rather than passed per call because a registered
/// handler is called with only an [`InvocationContext`], which carries the
/// request but not the session's router. `ChatExtension` stores one for exactly
/// the same reason.
pub struct ChatTurnWorkflow {
    runtime: RuntimeContext,
}

impl ChatTurnWorkflow {
    #[must_use]
    pub fn new(runtime: RuntimeContext) -> Self {
        Self { runtime }
    }
}

/// The `workflow.run` request body.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkflowRunInput {
    /// The steps, in execution order. An empty list is a no-op.
    steps: Vec<WorkflowStep>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkflowStep {
    /// The raw prompt of this step's turn, exactly as `chat.submit` takes it.
    prompt: String,
    /// The run identity this step should use, when the caller has one. Absent
    /// means the executor generates one, so the output can always report the
    /// run each step produced.
    #[serde(default)]
    run_id: Option<RunId>,
}

#[async_trait]
impl WorkflowExecutor for ChatTurnWorkflow {
    async fn run(&self, context: InvocationContext, input: Value) -> Result<Value, KernelError> {
        let request: WorkflowRunInput = serde_json::from_value(input).map_err(|error| {
            KernelError::new(
                ErrorCategory::InvalidRequest,
                format!("invalid workflow.run request: {error}"),
                "workflow",
                false,
            )
        })?;

        let mut runs = Vec::with_capacity(request.steps.len());
        for (index, step) in request.steps.into_iter().enumerate() {
            let run_id = step
                .run_id
                .unwrap_or_else(|| RunId::from(uuid::Uuid::new_v4().to_string()));
            // A step is the chat turn's contract, invoked by name — Workflow
            // keeps no compile-time dependency on Chat. The fresh invocation
            // carries the caller's plugin identity and actor (so the inner
            // `chat.submit` is authorized as the host that asked for the
            // workflow) with its own request id and cancellation token: the same
            // hop `chat.submit` itself makes towards `runs.start`, and for the
            // same reason — reusing the caller's invocation would give every
            // step one request id and couple the caller's cancellation to a step
            // it never named.
            let invocation = self.runtime.invocation(
                RequestId::from(uuid::Uuid::new_v4().to_string()),
                context.plugin_id().clone(),
                context.actor().to_string(),
            );
            let step_input = json!({ "prompt": step.prompt, "run_id": run_id });
            self.runtime
                .router()
                .invoke("chat.submit", invocation, step_input)
                .await
                .map_err(|error| step_failed(index, error))?;
            runs.push(run_id);
        }

        Ok(json!({ "steps": runs.len(), "runs": runs }))
    }
}

/// Names the failing step without dropping the error's own fields.
///
/// The protocol error is mutated rather than rebuilt, so category,
/// retryability, request/plugin id and details survive the wrap.
fn step_failed(index: usize, error: KernelError) -> KernelError {
    let mut protocol = error.into_protocol_error();
    protocol.message = format!("workflow step {index} failed: {}", protocol.message);
    protocol.origin = "workflow".into();
    KernelError::from_protocol(protocol)
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
        dependencies: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    use tact::{CapabilityRouter, PermissionService, RuntimeServices};
    use tact_protocol::CapabilityDeclaration;

    struct AllowAll;

    #[async_trait]
    impl PermissionService for AllowAll {
        async fn check(
            &self,
            _declaration: &CapabilityDeclaration,
            _context: &InvocationContext,
            _input: &Value,
        ) -> Result<(), KernelError> {
            Ok(())
        }
    }

    /// A stub Chat: records every `chat.submit` call and can fail a chosen step.
    struct RecordingChat {
        calls: Mutex<Vec<(String, Option<RunId>)>>,
        fail_on: Option<usize>,
    }

    #[async_trait]
    impl CapabilityHandler for RecordingChat {
        async fn invoke(
            &self,
            _context: InvocationContext,
            input: Value,
        ) -> Result<Value, KernelError> {
            let prompt = input["prompt"]
                .as_str()
                .expect("chat.submit takes a prompt")
                .to_string();
            let run_id = input.get("run_id").and_then(Value::as_str).map(RunId::from);
            let index = {
                let mut calls = self.calls.lock().expect("calls lock");
                calls.push((prompt, run_id));
                calls.len() - 1
            };
            if self.fail_on == Some(index) {
                return Err(KernelError::new(
                    ErrorCategory::ToolError,
                    "step exploded",
                    "chat",
                    false,
                ));
            }
            Ok(json!({ "accepted": true }))
        }
    }

    fn chat_submit_declaration() -> CapabilityDeclaration {
        CapabilityDeclaration {
            name: "chat.submit".into(),
            kind: CapabilityKind::App,
            version: "1".into(),
            description: None,
            input_schema: None,
            output_schema: None,
            risk: CapabilityRisk::Medium,
        }
    }

    /// A runtime carrying the production Workflow executor over a stub Chat.
    fn workflow_runtime(recorder: Arc<RecordingChat>) -> RuntimeContext {
        let runtime = RuntimeContext::with_services(
            CapabilityRouter::new(),
            RuntimeServices::with_permission(Arc::new(AllowAll)),
        );
        runtime
            .router()
            .register(CapabilityRegistration::new(
                chat_submit_declaration(),
                recorder,
            ))
            .expect("chat.submit registers");
        WorkflowExtension::new(Arc::new(ChatTurnWorkflow::new(runtime.clone())))
            .register(&runtime)
            .expect("workflow.run registers");
        runtime
    }

    fn invocation(runtime: &RuntimeContext, name: &str) -> InvocationContext {
        runtime.invocation(RequestId::from(name), PluginId::from("tact.chat"), "test")
    }

    #[tokio::test]
    async fn workflow_runs_each_step_as_a_chat_turn_in_order() {
        let recorder = Arc::new(RecordingChat {
            calls: Mutex::new(Vec::new()),
            fail_on: None,
        });
        let runtime = workflow_runtime(Arc::clone(&recorder));

        let output = runtime
            .router()
            .invoke(
                "workflow.run",
                invocation(&runtime, "workflow-ok"),
                json!({
                    "steps": [
                        { "prompt": "one" },
                        { "prompt": "two", "run_id": "step-two" },
                    ]
                }),
            )
            .await
            .expect("workflow.run answers");

        let calls = recorder.calls.lock().expect("calls lock");
        assert_eq!(calls.len(), 2, "one chat.submit per step");
        assert_eq!(calls[0].0, "one");
        assert_eq!(calls[1].0, "two");
        assert_eq!(calls[1].1.as_ref().map(RunId::as_str), Some("step-two"));
        assert!(
            calls[0].1.is_some(),
            "a generated run id is passed so the output can report it"
        );
        assert_eq!(output["steps"], json!(2));
        assert_eq!(output["runs"][1], json!("step-two"));
        assert_eq!(
            output["runs"][0].as_str(),
            calls[0].1.as_ref().map(RunId::as_str)
        );
    }

    #[tokio::test]
    async fn workflow_stops_at_the_first_failing_step_and_names_it() {
        let recorder = Arc::new(RecordingChat {
            calls: Mutex::new(Vec::new()),
            fail_on: Some(1),
        });
        let runtime = workflow_runtime(Arc::clone(&recorder));

        let error = runtime
            .router()
            .invoke(
                "workflow.run",
                invocation(&runtime, "workflow-fail"),
                json!({
                    "steps": [
                        { "prompt": "ok" },
                        { "prompt": "boom" },
                        { "prompt": "never" },
                    ]
                }),
            )
            .await
            .expect_err("a failing step fails the workflow");

        assert_eq!(error.category(), ErrorCategory::ToolError);
        assert_eq!(error.origin(), "workflow");
        assert!(
            error.message().contains("workflow step 1 failed"),
            "the failing step is named: {}",
            error.message()
        );
        assert_eq!(
            recorder.calls.lock().expect("calls lock").len(),
            2,
            "the step after the failure must not run"
        );
    }

    #[tokio::test]
    async fn workflow_rejects_an_unknown_field() {
        let recorder = Arc::new(RecordingChat {
            calls: Mutex::new(Vec::new()),
            fail_on: None,
        });
        let runtime = workflow_runtime(recorder);

        let error = runtime
            .router()
            .invoke(
                "workflow.run",
                invocation(&runtime, "workflow-bad"),
                json!({ "steps": [], "extra": true }),
            )
            .await
            .expect_err("an unknown field is rejected, not ignored");

        assert_eq!(error.category(), ErrorCategory::InvalidRequest);
    }
}
