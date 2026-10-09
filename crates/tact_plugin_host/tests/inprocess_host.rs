//! The in-process Rust host speaks the same protocol as the external hosts.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{Value, json};
use tact::{
    CapabilityHandler, CapabilityRegistration, CapabilityRouter, FnCapabilityHandler,
    InvocationContext, KernelError, PermissionService, RuntimeContext, RuntimePluginManifest,
    RuntimeServices,
};
use tact_plugin_host::{InProcessPluginHost, PluginHost};
use tact_protocol::{
    CapabilityDeclaration, CapabilityKind, CapabilityRisk, PluginId, PluginRequest, PluginResponse,
    ProtocolVersion,
};

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

fn declaration() -> CapabilityDeclaration {
    CapabilityDeclaration {
        name: "echo".into(),
        kind: CapabilityKind::Tool,
        version: "1".into(),
        description: Some("Echo the input".into()),
        input_schema: None,
        output_schema: None,
        risk: CapabilityRisk::ReadOnly,
    }
}

#[tokio::test]
async fn in_process_host_invokes_through_the_capability_router() {
    let router = CapabilityRouter::new();
    let handler: Arc<dyn CapabilityHandler> = Arc::new(FnCapabilityHandler::new(
        |_, input| async move { Ok(input) },
    ));
    router
        .register(CapabilityRegistration::new(declaration(), handler))
        .expect("register");
    let runtime = RuntimeContext::with_services(
        router.clone(),
        RuntimeServices::with_permission(Arc::new(AllowAll)),
    );
    let manifest = RuntimePluginManifest {
        id: PluginId::from("tact.test"),
        version: "1.0.0".into(),
        protocol: ProtocolVersion::CURRENT,
        capabilities: vec![declaration()],
    };
    let mut host = InProcessPluginHost::new(manifest, router, runtime);

    let handshake = host
        .request(PluginRequest::Handshake {
            protocol_version: ProtocolVersion::CURRENT,
            features: Vec::new(),
        })
        .await
        .expect("handshake");
    assert!(matches!(
        handshake,
        PluginResponse::HandshakeAccepted { .. }
    ));

    let response = host
        .request(PluginRequest::Invoke {
            capability: "echo".into(),
            input: json!({"text": "hello"}),
        })
        .await
        .expect("invoke");
    match response {
        PluginResponse::Result { output } => assert_eq!(output, json!({"text": "hello"})),
        other => panic!("expected a result, got {other:?}"),
    }

    host.shutdown().await.expect("shutdown");
}
