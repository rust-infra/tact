use std::{sync::Arc, time::Duration};

use serde_json::json;
use tact_protocol::{
    CapabilityDeclaration, CapabilityKind, CapabilityRisk, ErrorCategory, PluginId, ProtocolError,
    RequestId,
};

use super::{
    CapabilityHandler, CapabilityRegistration, CapabilityRouter, FnCapabilityHandler,
    InvocationContext, KernelError,
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
    InvocationContext::new(
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
