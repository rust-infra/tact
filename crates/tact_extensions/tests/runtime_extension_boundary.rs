use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde_json::{Value, json};
use tact::{
    EventService, InteractionBroker, InteractionService, InvocationContext, KernelError,
    PermissionService, RuntimeContext, RuntimeServices, StorageService, TrajectoryService,
};
use tact_extensions::extensions::{
    agent::{AgentExecutor, AgentExtension},
    workflow::{WorkflowExecutor, WorkflowExtension},
};
use tact_protocol::{
    CapabilityDeclaration, InteractionRequest, InteractionResponse, PluginId, RequestId, RunId,
    RuntimeEvent, TrajectoryId,
};
use tokio::time::{Duration, timeout};

#[derive(Default)]
struct CapturedEvents(Mutex<Vec<RuntimeEvent>>);

#[async_trait]
impl EventService for CapturedEvents {
    async fn publish(&self, event: RuntimeEvent) -> Result<(), KernelError> {
        self.0.lock().unwrap().push(event);
        Ok(())
    }
}

#[derive(Default)]
struct CapturedTrajectory(Mutex<Vec<RuntimeEvent>>);

#[async_trait]
impl TrajectoryService for CapturedTrajectory {
    async fn append(
        &self,
        _trajectory_id: Option<&TrajectoryId>,
        _run_id: Option<&RunId>,
        event: RuntimeEvent,
    ) -> Result<(), KernelError> {
        self.0.lock().unwrap().push(event);
        Ok(())
    }
}

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

struct EmptyStorage;

#[async_trait]
impl StorageService for EmptyStorage {
    async fn get(&self, _namespace: &str, _key: &str) -> Result<Option<Value>, KernelError> {
        Ok(None)
    }

    async fn set(&self, _namespace: &str, _key: &str, _value: Value) -> Result<(), KernelError> {
        Ok(())
    }
}

struct EventPublishingAgent;

#[async_trait]
impl AgentExecutor for EventPublishingAgent {
    async fn run(&self, context: InvocationContext, message: String) -> Result<RunId, KernelError> {
        context
            .events()
            .publish(RuntimeEvent::Text {
                run_id: context.run_id().cloned(),
                role: "assistant".into(),
                content: message,
            })
            .await?;
        Ok(RunId::from("run-without-chat"))
    }
}

struct EchoWorkflow;

#[async_trait]
impl WorkflowExecutor for EchoWorkflow {
    async fn run(&self, _context: InvocationContext, input: Value) -> Result<Value, KernelError> {
        Ok(json!({"steps": input["steps"].as_array().unwrap().len()}))
    }
}

fn runtime(
    events: Arc<dyn EventService>,
    trajectory: Arc<dyn TrajectoryService>,
) -> RuntimeContext {
    RuntimeContext::with_services(
        tact::CapabilityRouter::new(),
        RuntimeServices::new(
            events,
            trajectory,
            Arc::new(AllowAll),
            Arc::new(EmptyStorage),
        ),
    )
}

#[tokio::test]
async fn chat_extension_runs_with_a_host_supplied_view_service() {
    let events = Arc::new(CapturedEvents::default());
    let runtime = runtime(events.clone(), Arc::new(CapturedTrajectory::default()));
    AgentExtension::new(Arc::new(EventPublishingAgent))
        .register(&runtime)
        .unwrap();

    let output = runtime
        .router()
        .invoke(
            "chat.start_run",
            runtime
                .invocation(
                    RequestId::from("request-chat-integration"),
                    PluginId::from("tact.chat"),
                    "web-view-adapter",
                )
                .with_run_id(RunId::from("run-view-replacement")),
            json!({"message": "visible through the injected view"}),
        )
        .await
        .unwrap();

    assert_eq!(output, json!({"run_id": "run-without-chat"}));
    assert!(matches!(
        events.0.lock().unwrap().as_slice(),
        [RuntimeEvent::Text { content, .. }]
            if content == "visible through the injected view"
    ));
}

#[tokio::test]
async fn workflow_plugin_runs_without_registering_a_chat_extension() {
    let runtime = runtime(
        Arc::new(CapturedEvents::default()),
        Arc::new(CapturedTrajectory::default()),
    );
    WorkflowExtension::new(Arc::new(EchoWorkflow))
        .register(&runtime)
        .unwrap();

    let output = runtime
        .router()
        .invoke(
            "workflow.run",
            runtime.invocation(
                RequestId::from("request-workflow-integration"),
                PluginId::from("tact.workflow"),
                "headless-client",
            ),
            json!({"steps": [1, 2, 3]}),
        )
        .await
        .unwrap();

    assert_eq!(output, json!({"steps": 3}));
}

#[tokio::test]
async fn headless_interaction_requests_are_recorded_and_can_resume() {
    let events = Arc::new(CapturedEvents::default());
    let trajectory = Arc::new(CapturedTrajectory::default());
    let runtime = runtime(events, trajectory.clone());
    let broker = InteractionBroker::new(8);
    let mut client = broker.subscribe();
    let request_id = RequestId::from("request-external-approval");
    let context = runtime
        .invocation(
            request_id.clone(),
            PluginId::from("tact.workflow"),
            "headless-client",
        )
        .with_run_id(RunId::from("run-external-approval"));
    let request = InteractionRequest::Permission {
        request_id: request_id.clone(),
        capability: "files.write".into(),
        reason: "Write generated output".into(),
    };
    let pending = tokio::spawn({
        let broker = broker.clone();
        async move { broker.request(request, &context).await }
    });

    let request = timeout(Duration::from_millis(200), client.recv())
        .await
        .expect("permission request was not delivered")
        .unwrap();
    assert!(matches!(
        request,
        InteractionRequest::Permission { request_id: id, .. } if id == request_id
    ));
    assert!(broker.respond(InteractionResponse::Approved {
        request_id: request_id.clone(),
    }));
    assert!(matches!(
        timeout(Duration::from_millis(200), pending)
            .await
            .expect("interaction response was not delivered")
            .unwrap()
            .unwrap(),
        InteractionResponse::Approved { request_id: id } if id == request_id
    ));
    assert!(matches!(
        trajectory.0.lock().unwrap().as_slice(),
        [RuntimeEvent::InteractionRequested { .. }]
    ));
}
