use std::{sync::Arc, time::Duration};

use async_trait::async_trait;
use serde_json::json;
use tact_protocol::{
    CapabilityDeclaration, CapabilityKind, CapabilityRisk, ErrorCategory, PluginId, ProtocolError,
    RequestId,
};

use super::{
    CapabilityHandler, CapabilityRegistration, CapabilityRouter, EventService, FnCapabilityHandler,
    InvocationContext, KernelError, PermissionService, RuntimeContext, RuntimeServices,
    StorageService, TrajectoryService,
};

fn declaration(name: &str) -> CapabilityDeclaration {
    CapabilityDeclaration {
        name: name.to_string(),
        kind: CapabilityKind::Tool,
        version: "1.0".to_string(),
        description: Some("test capability".to_string()),
        input_schema: None,
        output_schema: None,
        risk: CapabilityRisk::ReadOnly,
    }
}

fn context() -> InvocationContext {
    RuntimeContext::with_services(
        CapabilityRouter::new(),
        RuntimeServices::with_permission(Arc::new(AllowPermission)),
    )
    .invocation(
        RequestId::from("request-1"),
        PluginId::from("plugin-1"),
        "test",
    )
}

#[tokio::test]
async fn missing_capability_preserves_invocation_identity() {
    let router = CapabilityRouter::new();
    let error = router
        .invoke("missing", context(), json!({}))
        .await
        .expect_err("missing capability should fail");

    assert_eq!(error.category(), ErrorCategory::CapabilityNotFound);
    assert_eq!(error.request_id(), Some(&RequestId::from("request-1")));
    assert_eq!(error.plugin_id(), Some(&PluginId::from("plugin-1")));
}

#[tokio::test]
async fn missing_permission_service_fails_closed() {
    let router = CapabilityRouter::new();
    let called = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let handler_called = Arc::clone(&called);
    router
        .register_handler(
            declaration("demo.protected"),
            FnCapabilityHandler::new(move |_context: InvocationContext, _input| {
                handler_called.store(true, std::sync::atomic::Ordering::SeqCst);
                async { Ok(json!("ran")) }
            }),
        )
        .unwrap();
    let runtime = RuntimeContext::new(router);
    let error = runtime
        .router()
        .invoke(
            "demo.protected",
            runtime.invocation(
                RequestId::from("no-permission-service"),
                PluginId::from("plugin"),
                "test",
            ),
            json!({}),
        )
        .await
        .expect_err("an unconfigured permission boundary must deny");
    assert_eq!(error.category(), ErrorCategory::PermissionDenied);
    assert!(!called.load(std::sync::atomic::Ordering::SeqCst));
}

#[test]
fn duplicate_registration_is_rejected_and_descriptions_are_stable() {
    let router = CapabilityRouter::new();
    let handler: Arc<dyn CapabilityHandler> = Arc::new(FnCapabilityHandler::new(
        |_context: InvocationContext, input| async move { Ok(input) },
    ));
    router
        .register(CapabilityRegistration::new(
            declaration("demo.echo"),
            Arc::clone(&handler),
        ))
        .unwrap();
    let duplicate = router
        .register(CapabilityRegistration::new(
            declaration("demo.echo"),
            handler,
        ))
        .expect_err("duplicate registration should fail");

    assert_eq!(duplicate.category(), ErrorCategory::InvalidRequest);
    assert_eq!(router.describe("demo.echo").unwrap().name, "demo.echo");
    assert_eq!(router.describe_all().len(), 1);
}

#[test]
fn batch_registration_is_atomic_when_a_capability_conflicts() {
    let router = CapabilityRouter::new();
    let handler: Arc<dyn CapabilityHandler> = Arc::new(FnCapabilityHandler::new(
        |_context: InvocationContext, input| async move { Ok(input) },
    ));
    router
        .register(CapabilityRegistration::new(
            declaration("demo.existing"),
            Arc::clone(&handler),
        ))
        .unwrap();

    let error = router
        .register_many(vec![
            CapabilityRegistration::new(declaration("demo.new"), Arc::clone(&handler)),
            CapabilityRegistration::new(declaration("demo.existing"), Arc::clone(&handler)),
        ])
        .expect_err("a batch with a conflict must fail");
    assert_eq!(error.category(), ErrorCategory::InvalidRequest);
    assert!(router.describe("demo.new").is_none());
    assert_eq!(router.describe_all().len(), 1);
}

#[tokio::test]
async fn capability_invocation_publishes_and_records_tool_lifecycle() {
    let router = CapabilityRouter::new();
    router
        .register_handler(
            declaration("demo.echo"),
            FnCapabilityHandler::new(|_context: InvocationContext, input| async move { Ok(input) }),
        )
        .unwrap();
    let events = Arc::new(CapturedEvents::default());
    let trajectory = Arc::new(CapturedTrajectory::default());
    let runtime = RuntimeContext::with_services(
        router.clone(),
        RuntimeServices::new(
            events.clone(),
            trajectory.clone(),
            Arc::new(AllowPermission),
            Arc::new(NoStorage),
        ),
    );
    let context = runtime
        .invocation(
            RequestId::from("evented-call"),
            PluginId::from("test-plugin"),
            "test",
        )
        .with_run_id(tact_protocol::RunId::from("run-7"))
        .with_trajectory_id(tact_protocol::TrajectoryId::from("trajectory-7"));

    assert_eq!(
        router
            .invoke("demo.echo", context, json!({"ok":true}))
            .await
            .unwrap(),
        json!({"ok":true})
    );
    assert!(matches!(
        events.0.lock().await.as_slice(),
        [
            tact_protocol::RuntimeEvent::ToolCallStarted { .. },
            tact_protocol::RuntimeEvent::ToolCallFinished { success: true, .. }
        ]
    ));
    assert_eq!(trajectory.0.lock().await.len(), 2);
}

#[tokio::test]
async fn cancellation_stops_an_in_flight_invocation() {
    let router = CapabilityRouter::new();
    let handler: Arc<dyn CapabilityHandler> = Arc::new(FnCapabilityHandler::new(
        |context: InvocationContext, _input| async move {
            context.cancellation_token().cancelled().await;
            std::future::pending::<()>().await;
            #[allow(unreachable_code)]
            Ok(json!("completed"))
        },
    ));
    router
        .register(CapabilityRegistration::new(
            declaration("demo.wait"),
            handler,
        ))
        .unwrap();

    let invocation = context();
    let cancel = invocation.cancellation_token();
    let call = tokio::spawn({
        let router = router.clone();
        let invocation = invocation.clone();
        async move { router.invoke("demo.wait", invocation, json!({})).await }
    });
    tokio::time::sleep(Duration::from_millis(10)).await;
    cancel.cancel();
    let result = tokio::time::timeout(Duration::from_secs(1), call)
        .await
        .expect("cancellation must finish within a bounded timeout")
        .expect("invocation task should not panic")
        .expect_err("cancelled invocation should fail");

    assert_eq!(result.category(), ErrorCategory::Cancelled);
    assert_eq!(result.request_id(), Some(&RequestId::from("request-1")));
}

#[tokio::test]
async fn expired_deadline_is_rejected_before_handler_runs() {
    let router = CapabilityRouter::new();
    let called = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let called_by_handler = Arc::clone(&called);
    let handler: Arc<dyn CapabilityHandler> = Arc::new(FnCapabilityHandler::new(
        move |_context: InvocationContext, _input| {
            called_by_handler.store(true, std::sync::atomic::Ordering::SeqCst);
            async { Ok(json!(null)) }
        },
    ));
    router
        .register(CapabilityRegistration::new(
            declaration("demo.expired"),
            handler,
        ))
        .unwrap();

    let invocation = context().with_timeout(Duration::ZERO);
    let error = router
        .invoke("demo.expired", invocation, json!({}))
        .await
        .expect_err("expired invocation should fail");

    assert_eq!(error.category(), ErrorCategory::Timeout);
    assert!(!called.load(std::sync::atomic::Ordering::SeqCst));
}

/// A router whose only capability sleeps far past any deadline a test sets,
/// plus the runtime that carries the given event service.
///
/// The kind matters to what a test can observe: a `Tool` invocation with a run
/// additionally publishes its own lifecycle, a `Service` one publishes nothing
/// but the deadline fact.
fn timing_router(
    events: Arc<dyn EventService>,
    kind: CapabilityKind,
) -> (CapabilityRouter, RuntimeContext) {
    let router = CapabilityRouter::new();
    router
        .register_handler(
            CapabilityDeclaration {
                kind,
                ..declaration("demo.slow")
            },
            FnCapabilityHandler::new(|_context: InvocationContext, _input| async {
                tokio::time::sleep(Duration::from_secs(30)).await;
                Ok(json!("late"))
            }),
        )
        .unwrap();
    let runtime = RuntimeContext::with_services(
        router.clone(),
        RuntimeServices::new(
            events,
            Arc::new(CapturedTrajectory::default()),
            Arc::new(AllowPermission),
            Arc::new(NoStorage),
        ),
    );
    (router, runtime)
}

/// A deadline the Kernel enforces is a fact about a run, not only an error: the
/// run it cut off receives `TimedOut`.
#[tokio::test]
async fn a_deadline_publishes_the_timeout_fact_for_the_run() {
    let events = Arc::new(CapturedEvents::default());
    let (router, runtime) = timing_router(
        Arc::clone(&events) as Arc<dyn EventService>,
        CapabilityKind::Tool,
    );
    let invocation = runtime
        .invocation(
            RequestId::from("timed-call"),
            PluginId::from("plugin-1"),
            "test",
        )
        .with_run_id(tact_protocol::RunId::from("run-9"))
        .with_timeout(Duration::from_millis(10));

    let error = tokio::time::timeout(
        Duration::from_secs(5),
        router.invoke("demo.slow", invocation, json!({})),
    )
    .await
    .expect("the deadline fires within a bounded wait")
    .expect_err("a handler past its deadline times out");

    assert_eq!(error.category(), ErrorCategory::Timeout);
    // The fact lands between the tool call's start and its terminal event: the
    // deadline cuts the handler, and the failed call is reported after it.
    assert!(matches!(
        events.0.lock().await.as_slice(),
        [
            tact_protocol::RuntimeEvent::ToolCallStarted { .. },
            tact_protocol::RuntimeEvent::TimedOut { run_id },
            tact_protocol::RuntimeEvent::ToolCallFinished { success: false, .. },
        ] if run_id == &tact_protocol::RunId::from("run-9")
    ));
}

/// The documented boundary: `TimedOut` carries a **required** run id, so an
/// invocation with no run records nothing rather than inventing an attribution.
#[tokio::test]
async fn a_deadline_without_a_run_publishes_nothing() {
    let events = Arc::new(CapturedEvents::default());
    let (router, runtime) = timing_router(
        Arc::clone(&events) as Arc<dyn EventService>,
        CapabilityKind::Tool,
    );
    let invocation = runtime
        .invocation(
            RequestId::from("run-less-call"),
            PluginId::from("plugin-1"),
            "test",
        )
        .with_timeout(Duration::from_millis(10));

    let error = tokio::time::timeout(
        Duration::from_secs(5),
        router.invoke("demo.slow", invocation, json!({})),
    )
    .await
    .expect("the deadline fires within a bounded wait")
    .expect_err("a handler past its deadline times out");

    assert_eq!(error.category(), ErrorCategory::Timeout);
    assert!(
        events.0.lock().await.is_empty(),
        "a run-less invocation has no run the fact could be attributed to"
    );
}

/// Recording is best effort: an event service that refuses the fact must not
/// replace the timeout error the caller is about to receive.
///
/// The capability is a `Service` so the deadline fact is the only publish this
/// invocation attempts — a `Tool` call would also try to publish its own
/// lifecycle, and that terminal fact has its own (pre-existing) say in the
/// caller's error.
#[tokio::test]
async fn a_failed_timeout_publish_keeps_the_timeout_error() {
    let (router, runtime) = timing_router(Arc::new(FailingEvents), CapabilityKind::Service);
    let invocation = runtime
        .invocation(
            RequestId::from("timed-call"),
            PluginId::from("plugin-1"),
            "test",
        )
        .with_run_id(tact_protocol::RunId::from("run-9"))
        .with_timeout(Duration::from_millis(10));

    let error = tokio::time::timeout(
        Duration::from_secs(5),
        router.invoke("demo.slow", invocation, json!({})),
    )
    .await
    .expect("the deadline fires within a bounded wait")
    .expect_err("a handler past its deadline times out");

    assert_eq!(error.category(), ErrorCategory::Timeout);
    assert_eq!(error.origin(), "kernel");
    assert_eq!(error.message(), "invocation deadline exceeded");
}

#[test]
fn protocol_error_conversion_preserves_metadata() {
    let mut protocol = ProtocolError::new(ErrorCategory::PluginCrashed, "crashed", "node", true);
    protocol.request_id = Some(RequestId::from("request-9"));
    protocol.plugin_id = Some(PluginId::from("plugin-9"));
    let kernel = KernelError::from(protocol.clone());

    assert_eq!(kernel.category(), ErrorCategory::PluginCrashed);
    assert_eq!(kernel.origin(), "node");
    assert!(kernel.retryable());
    let converted = ProtocolError::from(kernel);
    assert_eq!(converted.category, protocol.category);
    assert_eq!(converted.message, protocol.message);
    assert_eq!(converted.origin, protocol.origin);
    assert_eq!(converted.retryable, protocol.retryable);
    assert_eq!(converted.request_id, protocol.request_id);
    assert_eq!(converted.plugin_id, protocol.plugin_id);
}

struct AllowPermission;

#[async_trait]
impl PermissionService for AllowPermission {
    async fn check(
        &self,
        _declaration: &CapabilityDeclaration,
        _context: &InvocationContext,
        _input: &serde_json::Value,
    ) -> Result<(), KernelError> {
        Ok(())
    }
}

struct NoStorage;

#[async_trait]
impl StorageService for NoStorage {
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

#[derive(Default)]
struct CapturedEvents(tokio::sync::Mutex<Vec<tact_protocol::RuntimeEvent>>);

#[async_trait]
impl EventService for CapturedEvents {
    async fn publish(&self, event: tact_protocol::RuntimeEvent) -> Result<(), KernelError> {
        self.0.lock().await.push(event);
        Ok(())
    }
}

/// An event service that refuses every fact, standing in for a broken
/// transport when the question is what the caller's error should be.
struct FailingEvents;

#[async_trait]
impl EventService for FailingEvents {
    async fn publish(&self, _event: tact_protocol::RuntimeEvent) -> Result<(), KernelError> {
        Err(KernelError::new(
            ErrorCategory::InternalError,
            "event transport is down",
            "test",
            true,
        ))
    }
}

#[derive(Default)]
struct CapturedTrajectory(tokio::sync::Mutex<Vec<tact_protocol::RuntimeEvent>>);

#[async_trait]
impl TrajectoryService for CapturedTrajectory {
    async fn append(
        &self,
        _trajectory_id: Option<&tact_protocol::TrajectoryId>,
        _run_id: Option<&tact_protocol::RunId>,
        event: tact_protocol::RuntimeEvent,
    ) -> Result<(), KernelError> {
        self.0.lock().await.push(event);
        Ok(())
    }
}
