use std::{sync::Arc, time::Duration};

use tact_protocol::{InteractionRequest, InteractionResponse, PluginId, RequestId};
use tokio::time::timeout;

use super::{
    CapabilityRouter, InteractionBroker, InteractionService, KernelError, PermissionService,
    RuntimeContext, RuntimeServices,
};

struct AllowAll;

#[async_trait::async_trait]
impl PermissionService for AllowAll {
    async fn check(
        &self,
        _declaration: &tact_protocol::CapabilityDeclaration,
        _context: &super::InvocationContext,
        _input: &serde_json::Value,
    ) -> Result<(), KernelError> {
        Ok(())
    }
}

fn runtime() -> RuntimeContext {
    RuntimeContext::with_services(
        CapabilityRouter::new(),
        RuntimeServices::with_permission(Arc::new(AllowAll)),
    )
}

#[tokio::test]
async fn broker_preserves_interaction_order_and_correlates_response() {
    let broker = InteractionBroker::new(4);
    let mut subscription = broker.subscribe();
    let runtime = runtime();
    let request_id = RequestId::from("interaction-1");
    let context = runtime.invocation(request_id.clone(), PluginId::from("tact.chat"), "test");
    let request = InteractionRequest::Select {
        request_id: request_id.clone(),
        prompt: "Choose".into(),
        options: vec!["one".into(), "two".into()],
        log_confirm: true,
    };
    let request_task = tokio::spawn({
        let broker = broker.clone();
        async move { broker.request(request, &context).await }
    });

    let delivered = timeout(Duration::from_millis(200), subscription.recv())
        .await
        .expect("interaction request timed out")
        .unwrap();
    assert!(
        matches!(delivered, InteractionRequest::Select { ref request_id, .. } if request_id.as_str() == "interaction-1")
    );
    assert!(broker.respond(InteractionResponse::Selected {
        request_id: request_id.clone(),
        values: vec!["two".into()],
    }));

    let response = timeout(Duration::from_millis(200), request_task)
        .await
        .expect("interaction response timed out")
        .unwrap()
        .unwrap();
    assert!(matches!(response, InteractionResponse::Selected { values, .. } if values == ["two"]));
}

#[tokio::test]
async fn cancelled_interaction_waiter_is_removed_and_returns_cancelled() {
    let broker = InteractionBroker::new(4);
    let mut subscription = broker.subscribe();
    let runtime = runtime();
    let request_id = RequestId::from("interaction-cancel");
    let context = runtime.invocation(request_id.clone(), PluginId::from("tact.chat"), "test");
    let request_id_in_task = request_id.clone();
    let request_task = tokio::spawn({
        let broker = broker.clone();
        async move {
            broker
                .request(
                    InteractionRequest::Confirm {
                        request_id: request_id_in_task,
                        prompt: "Continue?".into(),
                    },
                    &context,
                )
                .await
        }
    });

    timeout(Duration::from_millis(200), subscription.recv())
        .await
        .expect("interaction request timed out")
        .unwrap();
    assert!(runtime.cancellation().cancel_request(&request_id));
    let error = timeout(Duration::from_millis(200), request_task)
        .await
        .expect("cancelled interaction did not finish")
        .unwrap()
        .unwrap_err();
    assert_eq!(error.category(), tact_protocol::ErrorCategory::Cancelled);
    assert!(!broker.respond(InteractionResponse::Approved { request_id }));
}

#[tokio::test]
async fn snapshot_exposes_pending_requests_until_answered() {
    let broker = InteractionBroker::new(4);
    let mut subscription = broker.subscribe();
    let runtime = runtime();
    let request_id = RequestId::from("interaction-snapshot");
    let context = runtime.invocation(request_id.clone(), PluginId::from("tact.chat"), "test");
    let request = InteractionRequest::Select {
        request_id: request_id.clone(),
        prompt: "Pick".into(),
        options: vec!["a".into(), "b".into()],
        log_confirm: false,
    };
    let request_task = tokio::spawn({
        let broker = broker.clone();
        async move { broker.request(request, &context).await }
    });

    // The broadcast is published after the request is registered, so once the
    // subscription delivers it the pending snapshot is authoritative.
    timeout(Duration::from_millis(200), subscription.recv())
        .await
        .expect("request timed out")
        .unwrap();
    let snapshot = broker.snapshot();
    assert_eq!(snapshot.len(), 1);
    assert!(
        matches!(&snapshot[0], InteractionRequest::Select { request_id, options, .. }
            if request_id.as_str() == "interaction-snapshot" && options == &vec!["a".to_string(), "b".to_string()])
    );

    assert!(broker.respond(InteractionResponse::Selected {
        request_id,
        values: vec!["a".into()],
    }));
    timeout(Duration::from_millis(200), request_task)
        .await
        .expect("response timed out")
        .unwrap()
        .unwrap();
    assert!(broker.snapshot().is_empty());
}
