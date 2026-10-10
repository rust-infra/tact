use std::{path::PathBuf, sync::Arc, time::Duration};

use serde_json::json;
use tact::{
    EventService, InvocationContext, KernelError, PermissionService, RuntimeContext,
    RuntimeServices, StorageService, TrajectoryService,
};
use tact_plugin_node::NodePluginHost;
use tact_protocol::{
    CapabilityDeclaration, ErrorCategory, PluginId, PluginRequest, PluginResponse, ProtocolVersion,
    RequestId, RunId, RuntimeEvent, TrajectoryId,
};

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/chat-plugin/index.mjs")
}

async fn start(args: &[&str]) -> NodePluginHost {
    let args = std::iter::once(fixture().display().to_string())
        .chain(args.iter().map(|arg| arg.to_string()))
        .collect::<Vec<_>>();
    NodePluginHost::start(
        "node",
        &args,
        PluginId::from("fixture.chat"),
        ProtocolVersion::CURRENT,
    )
    .await
    .unwrap()
}

fn allow_runtime(router: tact::CapabilityRouter) -> RuntimeContext {
    RuntimeContext::with_services(
        router,
        RuntimeServices::with_permission(Arc::new(AllowPermissions)),
    )
}

#[tokio::test]
async fn registers_chat_capability_and_invokes_it() {
    let host = Arc::new(tokio::sync::Mutex::new(start(&[]).await));
    assert!(
        host.lock()
            .await
            .capabilities()
            .iter()
            .any(|capability| capability.name == "chat.echo")
    );
    let router = tact::CapabilityRouter::new();
    NodePluginHost::register_with_router(Arc::clone(&host), &router).unwrap();
    let runtime = allow_runtime(router);
    let context = runtime.invocation(
        RequestId::from("echo-call"),
        PluginId::from("fixture.chat"),
        "test",
    );
    let output = runtime
        .router()
        .invoke("chat.echo", context, json!({ "message": "hello" }))
        .await
        .unwrap();
    assert_eq!(output["message"], "hello");
    host.lock().await.stop().await.unwrap();
}

#[tokio::test]
async fn direct_invoke_requests_cannot_bypass_capability_router() {
    let mut host = start(&[]).await;
    let error = host
        .request(PluginRequest::Invoke {
            capability: "chat.echo".into(),
            input: json!({ "message": "bypass" }),
        })
        .await
        .unwrap_err();
    assert_eq!(error.category(), ErrorCategory::PermissionDenied);
    host.stop().await.unwrap();
}

#[tokio::test]
async fn chat_command_receives_run_identity_from_runtime() {
    let host = Arc::new(tokio::sync::Mutex::new(start(&[]).await));
    let router = tact::CapabilityRouter::new();
    NodePluginHost::register_with_router(Arc::clone(&host), &router).unwrap();
    let runtime = allow_runtime(router);
    let context = runtime
        .invocation(
            RequestId::from("start-chat"),
            PluginId::from("fixture.chat"),
            "test",
        )
        .with_run_id(RunId::from("run-chat"));
    let result = runtime
        .router()
        .invoke("chat.start", context, json!({ "message": "begin" }))
        .await
        .unwrap();
    assert_eq!(result["run_id"], "run-chat");
    host.lock().await.stop().await.unwrap();
}

#[tokio::test]
async fn replays_events_from_requested_sequence() {
    let mut host = start(&[]).await;
    let response = host
        .request(PluginRequest::Subscribe {
            from_sequence: Some(41),
        })
        .await
        .unwrap();
    let PluginResponse::Event {
        event: RuntimeEvent::Plugin { payload, .. },
    } = response
    else {
        panic!("expected replay event, got {response:?}");
    };
    assert_eq!(payload["from_sequence"], 41);
    host.stop().await.unwrap();
}

#[tokio::test]
async fn chat_plugin_can_answer_a_runtime_interaction() {
    let mut host = start(&[]).await;
    let event = host
        .request(PluginRequest::Subscribe {
            from_sequence: Some(42),
        })
        .await
        .unwrap();
    assert!(matches!(
        event,
        PluginResponse::Event {
            event: RuntimeEvent::Plugin { ref event_type, .. }
        } if event_type == "plugin.fixture.chat.interaction_ready"
    ));
    let response = host
        .request(PluginRequest::InteractionResponse {
            response: tact_protocol::InteractionResponse::Selected {
                request_id: RequestId::from("node-interaction"),
                values: vec!["hello".into()],
            },
        })
        .await
        .unwrap();
    assert!(matches!(
        response,
        PluginResponse::Result { output } if output["answered"] == true
    ));
    host.stop().await.unwrap();
}

#[tokio::test]
async fn plugin_cannot_forge_host_runtime_events() {
    let mut host = start(&["--forged"]).await;
    let error = host
        .request(PluginRequest::Subscribe {
            from_sequence: Some(1),
        })
        .await
        .unwrap_err();
    assert_eq!(error.category(), ErrorCategory::InvalidRequest);
    assert_eq!(host.state(), tact::PluginState::Failed);
}

#[tokio::test]
async fn denied_capability_never_reaches_node_process() {
    let host = Arc::new(tokio::sync::Mutex::new(start(&[]).await));
    let router = tact::CapabilityRouter::new();
    NodePluginHost::register_with_router(Arc::clone(&host), &router).unwrap();
    let services = RuntimeServices::new(
        Arc::new(NoopEvents),
        Arc::new(NoopTrajectory),
        Arc::new(DenyPermissions),
        Arc::new(NoopStorage),
    );
    let runtime = RuntimeContext::with_services(router, services);
    let context = runtime.invocation(
        RequestId::from("denied-call"),
        PluginId::from("fixture.chat"),
        "test",
    );
    let error = runtime
        .router()
        .invoke("chat.echo", context, json!({ "message": "blocked" }))
        .await
        .unwrap_err();
    assert_eq!(error.category(), ErrorCategory::PermissionDenied);

    let allowed_router = tact::CapabilityRouter::new();
    NodePluginHost::register_with_router(Arc::clone(&host), &allowed_router).unwrap();
    let allowed_runtime = allow_runtime(allowed_router);
    let context = allowed_runtime.invocation(
        RequestId::from("allowed-probe"),
        PluginId::from("fixture.chat"),
        "test",
    );
    let output = allowed_runtime
        .router()
        .invoke("chat.echo", context, json!({ "message": "probe" }))
        .await
        .unwrap();
    assert_eq!(output["calls"], 1, "only the explicit probe reached Node");
    host.lock().await.stop().await.unwrap();
}

#[tokio::test]
async fn invocation_publishes_events_and_persists_trajectory_facts() {
    let host = Arc::new(tokio::sync::Mutex::new(start(&[]).await));
    let router = tact::CapabilityRouter::new();
    NodePluginHost::register_with_router(Arc::clone(&host), &router).unwrap();
    let events = Arc::new(RecordedEvents::default());
    let trajectory = Arc::new(RecordedTrajectory::default());
    let services = RuntimeServices::new(
        events.clone(),
        trajectory.clone(),
        Arc::new(AllowPermissions),
        Arc::new(NoopStorage),
    );
    let runtime = RuntimeContext::with_services(router, services);
    let context = runtime
        .invocation(
            RequestId::from("recorded-call"),
            PluginId::from("fixture.chat"),
            "test",
        )
        .with_run_id(RunId::from("run-1"))
        .with_trajectory_id(TrajectoryId::from("trajectory-1"));
    runtime
        .router()
        .invoke("chat.echo", context, json!({ "message": "record me" }))
        .await
        .unwrap();

    let events = events.0.lock().await;
    assert_eq!(events.len(), 2);
    assert!(matches!(events[0], RuntimeEvent::ToolCallStarted { .. }));
    assert!(matches!(
        events[1],
        RuntimeEvent::ToolCallFinished { success: true, .. }
    ));
    let trajectory = trajectory.0.lock().await;
    assert_eq!(trajectory.len(), 2);
    assert!(matches!(
        trajectory[0],
        RuntimeEvent::ToolCallStarted { .. }
    ));
    assert!(matches!(
        trajectory[1],
        RuntimeEvent::ToolCallFinished { success: true, .. }
    ));
    host.lock().await.stop().await.unwrap();
}

#[tokio::test]
async fn timeout_terminates_failed_host() {
    // The host-wide timeout also bounds startup (spawning Node and completing
    // the handshake), so it must not share the short bound this test exercises:
    // on a loaded machine the spawn alone can outlast a few tens of
    // milliseconds and fail the test before the request under test is even
    // reached. Keep the host generous and narrow the one invocation below.
    let host = Arc::new(tokio::sync::Mutex::new(
        NodePluginHost::start_with_timeout(
            "node",
            &[fixture().display().to_string()],
            PluginId::from("fixture.chat"),
            ProtocolVersion::CURRENT,
            Duration::from_secs(5),
        )
        .await
        .unwrap(),
    ));
    let router = tact::CapabilityRouter::new();
    NodePluginHost::register_with_router(Arc::clone(&host), &router).unwrap();
    let runtime = allow_runtime(router);
    // The short timeout that used to be the host-wide setting now applies to
    // exactly the request under test, so the timeout semantics are unchanged.
    let context = runtime
        .invocation(
            RequestId::from("timeout-call"),
            PluginId::from("fixture.chat"),
            "test",
        )
        .with_timeout(Duration::from_millis(40));
    let error = runtime
        .router()
        .invoke("chat.slow", context, json!({}))
        .await
        .unwrap_err();
    assert_eq!(error.category(), ErrorCategory::Timeout);
    // The Router enforces the invocation deadline itself, so it can return the
    // timeout a hair before the host's own expiry path marks it Failed. Wait
    // (bounded) for that observable transition instead of racing it; the
    // assertion below is unchanged.
    tokio::time::timeout(Duration::from_secs(1), async {
        while host.lock().await.state() != tact::PluginState::Failed {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .expect("the timed-out host must reach Failed");
    assert_eq!(host.lock().await.state(), tact::PluginState::Failed);
}

#[tokio::test]
async fn cancellation_terminates_only_the_plugin_host() {
    let host = Arc::new(tokio::sync::Mutex::new(start(&[]).await));
    let router = tact::CapabilityRouter::new();
    NodePluginHost::register_with_router(Arc::clone(&host), &router).unwrap();
    let runtime = allow_runtime(router);
    let request_id = RequestId::from("cancel-this-call");
    let context = runtime.invocation(request_id.clone(), PluginId::from("fixture.chat"), "test");
    let cancellation = runtime.cancellation().clone();
    let task = tokio::spawn(async move {
        runtime
            .router()
            .invoke("chat.slow", context, json!({}))
            .await
    });
    tokio::time::sleep(Duration::from_millis(40)).await;
    assert!(cancellation.cancel_request(&request_id));
    let error = task.await.unwrap().unwrap_err();
    assert_eq!(error.category(), ErrorCategory::Cancelled);
    assert_eq!(host.lock().await.state(), tact::PluginState::Failed);
}

#[tokio::test]
async fn malformed_response_fails_host_and_restart_recovers() {
    let host = Arc::new(tokio::sync::Mutex::new(start(&[]).await));
    let router = tact::CapabilityRouter::new();
    NodePluginHost::register_with_router(Arc::clone(&host), &router).unwrap();
    let runtime = allow_runtime(router);
    let context = runtime.invocation(
        RequestId::from("malformed-call"),
        PluginId::from("fixture.chat"),
        "test",
    );
    let error = runtime
        .router()
        .invoke("chat.malformed", context, json!({}))
        .await
        .unwrap_err();
    assert_eq!(error.category(), ErrorCategory::PluginCrashed);
    assert_eq!(host.lock().await.state(), tact::PluginState::Failed);
    host.lock().await.restart().await.unwrap();
    let context = runtime.invocation(
        RequestId::from("after-restart"),
        PluginId::from("fixture.chat"),
        "test",
    );
    let result = runtime
        .router()
        .invoke("chat.echo", context, json!({ "after_restart": true }))
        .await
        .unwrap();
    assert_eq!(result["after_restart"], true);
    host.lock().await.stop().await.unwrap();
}

#[tokio::test]
async fn plugin_crash_does_not_prevent_host_restart() {
    let crashed = Arc::new(tokio::sync::Mutex::new(start(&[]).await));
    let independent = Arc::new(tokio::sync::Mutex::new(start(&[]).await));
    let crashed_router = tact::CapabilityRouter::new();
    let independent_router = tact::CapabilityRouter::new();
    NodePluginHost::register_with_router(Arc::clone(&crashed), &crashed_router).unwrap();
    NodePluginHost::register_with_router(Arc::clone(&independent), &independent_router).unwrap();
    let crashed_runtime = allow_runtime(crashed_router);
    let context = crashed_runtime.invocation(
        RequestId::from("crash-call"),
        PluginId::from("fixture.chat"),
        "test",
    );
    let error = crashed_runtime
        .router()
        .invoke("chat.crash", context, json!({}))
        .await
        .unwrap_err();
    assert_eq!(error.category(), ErrorCategory::PluginCrashed);
    assert_eq!(crashed.lock().await.state(), tact::PluginState::Failed);
    assert_eq!(independent.lock().await.state(), tact::PluginState::Running);
    let independent_runtime = allow_runtime(independent_router);
    let context = independent_runtime.invocation(
        RequestId::from("independent-call"),
        PluginId::from("fixture.chat"),
        "test",
    );
    independent_runtime
        .router()
        .invoke("chat.echo", context, json!({}))
        .await
        .unwrap();
    crashed.lock().await.restart().await.unwrap();
    let context = crashed_runtime.invocation(
        RequestId::from("after-crash-restart"),
        PluginId::from("fixture.chat"),
        "test",
    );
    crashed_runtime
        .router()
        .invoke("chat.echo", context, json!({}))
        .await
        .unwrap();
    crashed.lock().await.stop().await.unwrap();
    independent.lock().await.stop().await.unwrap();
}

#[tokio::test]
async fn protocol_mismatch_is_rejected_during_startup() {
    let args = [fixture().display().to_string(), "--mismatch".into()];
    let result = NodePluginHost::start(
        "node",
        &args,
        PluginId::from("fixture.chat"),
        ProtocolVersion::CURRENT,
    )
    .await;
    assert!(result.is_err());
}

#[tokio::test]
async fn invalid_capability_manifest_is_rejected() {
    let args = [fixture().display().to_string(), "--invalid".into()];
    let result = NodePluginHost::start(
        "node",
        &args,
        PluginId::from("fixture.chat"),
        ProtocolVersion::CURRENT,
    )
    .await;
    assert!(result.is_err());
}

struct DenyPermissions;

#[async_trait::async_trait]
impl PermissionService for DenyPermissions {
    async fn check(
        &self,
        _declaration: &CapabilityDeclaration,
        _context: &InvocationContext,
        _input: &serde_json::Value,
    ) -> Result<(), KernelError> {
        Err(KernelError::permission_denied("denied for test"))
    }
}

struct AllowPermissions;

#[async_trait::async_trait]
impl PermissionService for AllowPermissions {
    async fn check(
        &self,
        _declaration: &CapabilityDeclaration,
        _context: &InvocationContext,
        _input: &serde_json::Value,
    ) -> Result<(), KernelError> {
        Ok(())
    }
}

#[derive(Default)]
struct RecordedEvents(tokio::sync::Mutex<Vec<RuntimeEvent>>);

#[async_trait::async_trait]
impl EventService for RecordedEvents {
    async fn publish(&self, event: RuntimeEvent) -> Result<(), KernelError> {
        self.0.lock().await.push(event);
        Ok(())
    }
}

#[derive(Default)]
struct RecordedTrajectory(tokio::sync::Mutex<Vec<RuntimeEvent>>);

#[async_trait::async_trait]
impl TrajectoryService for RecordedTrajectory {
    async fn append(
        &self,
        _trajectory_id: Option<&TrajectoryId>,
        _run_id: Option<&RunId>,
        event: RuntimeEvent,
    ) -> Result<(), KernelError> {
        self.0.lock().await.push(event);
        Ok(())
    }
}

struct NoopEvents;

#[async_trait::async_trait]
impl EventService for NoopEvents {
    async fn publish(&self, _event: RuntimeEvent) -> Result<(), KernelError> {
        Ok(())
    }
}

struct NoopTrajectory;

#[async_trait::async_trait]
impl TrajectoryService for NoopTrajectory {
    async fn append(
        &self,
        _trajectory_id: Option<&TrajectoryId>,
        _run_id: Option<&RunId>,
        _event: RuntimeEvent,
    ) -> Result<(), KernelError> {
        Ok(())
    }
}

struct NoopStorage;

#[async_trait::async_trait]
impl StorageService for NoopStorage {
    async fn get(
        &self,
        _namespace: &str,
        _key: &str,
    ) -> Result<Option<serde_json::Value>, KernelError> {
        Ok(None)
    }

    async fn set(
        &self,
        _namespace: &str,
        _key: &str,
        _value: serde_json::Value,
    ) -> Result<(), KernelError> {
        Ok(())
    }
}
