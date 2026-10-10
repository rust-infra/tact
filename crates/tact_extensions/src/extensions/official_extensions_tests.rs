use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde_json::{Value, json};
use tact_protocol::{
    CapabilityDeclaration, CapabilityKind, CapabilityRisk, ErrorCategory, PluginId,
    ProtocolVersion, RequestId, RunId, RuntimeEvent,
};

use crate::{
    extensions::{
        agent::{AgentExecutor, AgentExtension},
        chat,
        workflow::{WorkflowExecutor, WorkflowExtension},
    },
    plugin::{PluginRegistry, PluginState, RuntimePluginManifest},
};
use tact::{
    CapabilityRegistration, CapabilityRouter, EventService, InvocationContext, KernelError,
    PermissionService, RuntimeContext, RuntimeServices,
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

#[derive(Default)]
struct CapturedEvents(Mutex<Vec<RuntimeEvent>>);

#[async_trait]
impl EventService for CapturedEvents {
    async fn publish(&self, event: RuntimeEvent) -> Result<(), KernelError> {
        self.0.lock().unwrap().push(event);
        Ok(())
    }
}

struct PublishingExecutor;

#[async_trait]
impl AgentExecutor for PublishingExecutor {
    async fn run(
        &self,
        context: InvocationContext,
        message: tact_llm::Message,
    ) -> Result<RunId, KernelError> {
        context
            .events()
            .publish(RuntimeEvent::Text {
                run_id: None,
                role: "assistant".into(),
                content: crate::extract_text(&message.content),
            })
            .await?;
        Ok(RunId::from("run-view-adapter"))
    }
}

struct EchoWorkflow;

#[async_trait]
impl WorkflowExecutor for EchoWorkflow {
    async fn run(&self, _context: InvocationContext, input: Value) -> Result<Value, KernelError> {
        Ok(json!({"accepted": input["steps"].as_array().unwrap().len()}))
    }
}

fn runtime(events: Arc<dyn EventService>) -> RuntimeContext {
    RuntimeContext::with_services(
        CapabilityRouter::new(),
        RuntimeServices::with_event_and_permission(events, Arc::new(AllowAll)),
    )
}

#[tokio::test]
async fn chat_capability_uses_the_host_event_adapter() {
    let events = Arc::new(CapturedEvents::default());
    let runtime = runtime(events.clone());
    AgentExtension::new(Arc::new(PublishingExecutor))
        .register(&runtime)
        .unwrap();

    let result = runtime
        .router()
        .invoke(
            "chat.start_run",
            runtime.invocation(
                RequestId::from("request-chat"),
                PluginId::from("tact.chat"),
                "alternate-view",
            ),
            json!({"message": "render this elsewhere"}),
        )
        .await
        .unwrap();

    assert_eq!(result, json!({"run_id": "run-view-adapter"}));
    assert!(matches!(
        events.0.lock().unwrap().as_slice(),
        [RuntimeEvent::Text { role, content, .. }]
            if role == "assistant" && content == "render this elsewhere"
    ));
    assert!(chat::manifest().capabilities.iter().any(|capability| {
        capability.name == "chat.start_run" && capability.kind == CapabilityKind::App
    }));
}

#[tokio::test]
async fn workflow_extension_invokes_through_the_shared_capability_router() {
    let runtime = runtime(Arc::new(CapturedEvents::default()));
    WorkflowExtension::new(Arc::new(EchoWorkflow))
        .register(&runtime)
        .unwrap();

    let result = runtime
        .router()
        .invoke(
            "workflow.run",
            runtime.invocation(
                RequestId::from("request-workflow"),
                PluginId::from("tact.workflow"),
                "test-view",
            ),
            json!({"steps": ["one", "two"]}),
        )
        .await
        .unwrap();

    assert_eq!(result, json!({"accepted": 2}));
}

#[tokio::test]
async fn replacing_a_stopped_capability_switches_its_handler() {
    let runtime = runtime(Arc::new(CapturedEvents::default()));
    let declaration = CapabilityDeclaration {
        name: "chat.replaceable".into(),
        kind: CapabilityKind::App,
        version: "1".into(),
        description: None,
        input_schema: None,
        output_schema: None,
        risk: CapabilityRisk::ReadOnly,
    };
    runtime
        .router()
        .register(CapabilityRegistration::new(
            declaration.clone(),
            Arc::new(tact::FnCapabilityHandler::new(|_, _| async {
                Ok(json!({"handler": "old"}))
            })),
        ))
        .unwrap();
    runtime
        .router()
        .replace(CapabilityRegistration::new(
            declaration,
            Arc::new(tact::FnCapabilityHandler::new(|_, _| async {
                Ok(json!({"handler": "replacement"}))
            })),
        ))
        .unwrap();

    let result = runtime
        .router()
        .invoke(
            "chat.replaceable",
            runtime.invocation(
                RequestId::from("request-replaced"),
                PluginId::from("tact.chat"),
                "test",
            ),
            json!({}),
        )
        .await
        .unwrap();
    assert_eq!(result, json!({"handler": "replacement"}));
}

#[test]
fn plugin_registry_can_disable_and_replace_an_extension() {
    let registry = PluginRegistry::new(ProtocolVersion::CURRENT);
    let mut manifest = chat::manifest();
    registry.register(manifest.clone()).unwrap();
    registry.disable(&manifest.id).unwrap();
    assert_eq!(registry.state(&manifest.id), Some(PluginState::Stopped));

    manifest.version = "replacement".into();
    registry.replace(manifest.clone()).unwrap();

    assert_eq!(registry.state(&manifest.id), Some(PluginState::Registered));
    assert_eq!(registry.manifests()[0].version, "replacement");
    registry.enable(&manifest.id).unwrap();
    assert_eq!(registry.state(&manifest.id), Some(PluginState::Running));
}

#[test]
fn official_chat_capability_registration_rejects_a_duplicate_name() {
    let runtime = runtime(Arc::new(CapturedEvents::default()));
    AgentExtension::new(Arc::new(PublishingExecutor))
        .register(&runtime)
        .unwrap();
    let duplicate = CapabilityDeclaration {
        name: "chat.start_run".into(),
        kind: CapabilityKind::App,
        version: "1".into(),
        description: None,
        input_schema: None,
        output_schema: None,
        risk: CapabilityRisk::Medium,
    };

    let error = runtime
        .router()
        .register(CapabilityRegistration::new(
            duplicate,
            Arc::new(tact::FnCapabilityHandler::new(|_, _| async {
                Ok(json!({}))
            })),
        ))
        .unwrap_err();

    assert_eq!(error.category(), ErrorCategory::InvalidRequest);
}

#[test]
fn plugin_registry_rejects_replacing_a_running_extension() {
    let registry = PluginRegistry::new(ProtocolVersion::CURRENT);
    let manifest = chat::manifest();
    registry.register(manifest.clone()).unwrap();
    registry
        .set_state(&manifest.id, PluginState::Running)
        .unwrap();
    let replacement = RuntimePluginManifest {
        version: "unsafe-replacement".into(),
        ..manifest.clone()
    };

    let error = registry.replace(replacement).unwrap_err();

    assert_eq!(error.category(), ErrorCategory::InvalidRequest);
    assert_eq!(registry.manifests()[0].version, manifest.version);
}
