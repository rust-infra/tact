use std::{
    borrow::Cow,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use anyhow::Result;
use async_trait::async_trait;
use rmcp::model::{CallToolResult, Content, JsonObject, Tool as McpTool};
use serde_json::{Value, json};
use tact_protocol::{
    CapabilityDeclaration, ErrorCategory, PluginId, RequestId, RunId, RuntimeEvent, TrajectoryId,
};

use crate::{
    mcp::{MCPToolRouter, McpClient, MockMcpService},
    security::{RedactionConfig, sensitive::Scanner},
    tool::{OutputPolicy, Tool, ToolCallResult, ToolContext, ToolEffect, ToolMetadata, ToolRouter},
};
use tact::{
    CapabilityRouter, EventService, InvocationContext, KernelError, PermissionService,
    RuntimeContext, RuntimeServices, StorageService, TrajectoryService,
};

struct EchoTool {
    calls: Arc<AtomicUsize>,
}

const ECHO_METADATA: ToolMetadata = ToolMetadata::read_json("echo", "Echo text.", "echo");

#[async_trait]
impl Tool for EchoTool {
    fn metadata(&self) -> &'static ToolMetadata {
        &ECHO_METADATA
    }

    fn input_schema(&self) -> Value {
        json!({"type":"object"})
    }

    async fn call(&self, _context: ToolContext, input: Value) -> Result<ToolCallResult> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if input.get("fail").and_then(Value::as_bool) == Some(true) {
            anyhow::bail!("echo failed");
        }
        Ok(ToolCallResult {
            content: format!("native:{}", input["text"].as_str().unwrap_or_default()),
            effects: vec![ToolEffect::CompactHistory {
                focus: Some("keep this".into()),
            }],
            image: None,
        })
    }
}

fn mcp_echo_tool() -> McpTool {
    McpTool {
        name: Cow::Borrowed("echo"),
        title: None,
        description: Some(Cow::Borrowed("MCP echo")),
        input_schema: Arc::new(JsonObject::new()),
        output_schema: None,
        annotations: None,
        execution: None,
        icons: None,
        meta: None,
    }
}

struct LongOutputTool;

const LONG_OUTPUT_METADATA: ToolMetadata = {
    let mut metadata =
        ToolMetadata::read_json("long_output", "Return a long result.", "long output");
    metadata.output = OutputPolicy::PersistLargeOutput;
    metadata
};

#[async_trait]
impl Tool for LongOutputTool {
    fn metadata(&self) -> &'static ToolMetadata {
        &LONG_OUTPUT_METADATA
    }

    fn input_schema(&self) -> Value {
        json!({"type":"object"})
    }

    async fn call(&self, _context: ToolContext, _input: Value) -> Result<ToolCallResult> {
        Ok(ToolCallResult::text("x".repeat(30_001)))
    }
}

fn build_routers(
    calls: Arc<AtomicUsize>,
    test_name: &str,
) -> (ToolRouter, MCPToolRouter, ToolContext, Arc<MockMcpService>) {
    let tools = ToolRouter::new().route(EchoTool { calls }).unwrap();
    let service = Arc::new(MockMcpService::new(vec![mcp_echo_tool()], |params| {
        let text = params
            .arguments
            .as_ref()
            .and_then(|arguments| arguments.get("text"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        Ok(CallToolResult::success(vec![Content::text(format!(
            "mcp:{text}"
        ))]))
    }));
    let mut mcp = MCPToolRouter::new();
    mcp.register_client(McpClient::with_service(
        "db",
        vec![mcp_echo_tool()],
        service.clone(),
    ));
    (
        tools,
        mcp,
        crate::tool::test_support::test_context(test_name),
        service,
    )
}

fn allow_runtime(router: CapabilityRouter) -> RuntimeContext {
    RuntimeContext::with_services(
        router,
        RuntimeServices::with_permission(Arc::new(AllowPermissions)),
    )
}

#[tokio::test]
async fn native_and_mcp_calls_share_capability_router_and_keep_metadata() {
    let calls = Arc::new(AtomicUsize::new(0));
    let (tools, mcp, context, service) = build_routers(calls.clone(), "capability-router-success");
    let router = super::register_tool_capabilities(
        &tools,
        &mcp,
        context,
        Scanner::disabled(),
        RedactionConfig::default(),
    )
    .unwrap();
    let native_declaration = router.describe("echo").unwrap();
    assert_eq!(native_declaration.name, "echo");
    assert_eq!(
        native_declaration.description.as_deref(),
        Some("Echo text.")
    );
    assert_eq!(
        native_declaration.risk,
        tact_protocol::CapabilityRisk::ReadOnly
    );
    assert!(native_declaration.input_schema.is_some());
    let runtime = allow_runtime(router.clone());

    let native_context = runtime.invocation(
        RequestId::from("native-call"),
        PluginId::from("agent.tools"),
        "agent",
    );
    let native = router
        .invoke("echo", native_context, json!({"text":"hello"}))
        .await
        .unwrap();
    assert_eq!(native["content"], "native:hello");
    assert_eq!(native["effects"][0]["type"], "compact_history");
    assert_eq!(native["effects"][0]["focus"], "keep this");
    assert_eq!(native["image"], Value::Null);

    let failed_context = runtime.invocation(
        RequestId::from("native-failure"),
        PluginId::from("agent.tools"),
        "agent",
    );
    let error = router
        .invoke("echo", failed_context, json!({"fail":true}))
        .await
        .unwrap_err();
    assert_eq!(error.category(), ErrorCategory::ToolError);

    let mcp_context = runtime.invocation(
        RequestId::from("mcp-call"),
        PluginId::from("agent.tools"),
        "agent",
    );
    let mcp_output = router
        .invoke("mcp__db__echo", mcp_context, json!({"text":"query"}))
        .await
        .unwrap();
    assert_eq!(mcp_output["content"], "mcp:query");
    let mcp_declaration = router.describe("mcp__db__echo").unwrap();
    assert_eq!(mcp_declaration.name, "mcp__db__echo");
    assert_eq!(mcp_declaration.risk, tact_protocol::CapabilityRisk::High);
    assert!(router.describe("mcp__echo").is_none());
    assert!(router.describe("list_mcp_resources").is_some());
    assert!(router.describe("list_mcp_prompts").is_some());
    for capability in ["list_mcp_resources", "list_mcp_prompts"] {
        let context = runtime.invocation(
            RequestId::from(format!("{capability}-call")),
            PluginId::from("agent.tools"),
            "agent",
        );
        let output = router.invoke(capability, context, json!({})).await.unwrap();
        assert!(output["content"].is_string());
    }
    for (capability, input) in [
        ("read_mcp_resource", json!({"uri":"file:///note"})),
        ("get_mcp_prompt", json!({"name":"template"})),
    ] {
        let context = runtime.invocation(
            RequestId::from(format!("{capability}-invalid")),
            PluginId::from("agent.tools"),
            "agent",
        );
        let error = router.invoke(capability, context, input).await.unwrap_err();
        assert_eq!(error.category(), ErrorCategory::ToolError);
        assert_eq!(
            error.message(),
            format!("Error invoking {capability}: `server` is required")
        );
    }
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert_eq!(service.calls().len(), 1);
}

#[tokio::test]
async fn capability_router_denies_native_and_mcp_before_handlers_run() {
    let calls = Arc::new(AtomicUsize::new(0));
    let (tools, mcp, context, service) = build_routers(calls.clone(), "capability-router-deny");
    let router = super::register_tool_capabilities(
        &tools,
        &mcp,
        context,
        Scanner::disabled(),
        RedactionConfig::default(),
    )
    .unwrap();
    let runtime = RuntimeContext::with_services(
        router.clone(),
        RuntimeServices::new(
            Arc::new(NoEvents),
            Arc::new(NoTrajectory),
            Arc::new(DenyPermissions),
            Arc::new(NoStorage),
        ),
    );

    for (request_id, capability) in [("deny-native", "echo"), ("deny-mcp", "mcp__db__echo")] {
        let context = runtime.invocation(
            RequestId::from(request_id),
            PluginId::from("agent.tools"),
            "agent",
        );
        let error = router
            .invoke(capability, context, json!({"text":"blocked"}))
            .await
            .unwrap_err();
        assert_eq!(error.category(), ErrorCategory::PermissionDenied);
    }
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert!(service.calls().is_empty());
}

#[tokio::test]
async fn unknown_native_or_mcp_names_fail_closed() {
    let calls = Arc::new(AtomicUsize::new(0));
    let (tools, mcp, context, _service) = build_routers(calls.clone(), "capability-unknown");
    let router = super::register_tool_capabilities(
        &tools,
        &mcp,
        context,
        Scanner::disabled(),
        RedactionConfig::default(),
    )
    .unwrap();
    let runtime = allow_runtime(router.clone());
    for name in ["missing_native", "mcp__missing__echo"] {
        let context = runtime.invocation(
            RequestId::from(format!("unknown-{name}")),
            PluginId::from("agent.tools"),
            "agent",
        );
        let error = router.invoke(name, context, json!({})).await.unwrap_err();
        assert_eq!(error.category(), ErrorCategory::CapabilityNotFound);
    }
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn native_output_policy_runs_inside_capability_handler() {
    let context = crate::tool::test_support::test_context("capability-native-output");
    let work_dir = context.work_dir.clone();
    let tools = ToolRouter::new().route(LongOutputTool).unwrap();
    let mcp = MCPToolRouter::new();
    let router = super::register_tool_capabilities(
        &tools,
        &mcp,
        context,
        Scanner::disabled(),
        RedactionConfig::default(),
    )
    .unwrap();
    let runtime = allow_runtime(router.clone());
    let context = runtime.invocation(
        RequestId::from("native-long-output"),
        PluginId::from("agent.tools"),
        "agent",
    );
    let output = router
        .invoke("long_output", context, json!({}))
        .await
        .unwrap();
    assert!(
        output["content"]
            .as_str()
            .unwrap()
            .contains("<persisted-output>")
    );
    assert!(
        work_dir
            .join(".tact/tool-results/native-long-output.txt")
            .is_file()
    );
    let _ = std::fs::remove_dir_all(work_dir);
}

#[tokio::test]
async fn mcp_output_policy_runs_inside_capability_handler() {
    let context = crate::tool::test_support::test_context("capability-mcp-output");
    let work_dir = context.work_dir.clone();
    let tool = mcp_echo_tool();
    let service = Arc::new(MockMcpService::new(vec![tool.clone()], |_| {
        Ok(CallToolResult::success(vec![Content::text(
            "x".repeat(30_001),
        )]))
    }));
    let mut mcp = MCPToolRouter::new();
    mcp.register_client(McpClient::with_service("db", vec![tool], service));
    let tools = ToolRouter::new();
    let router = super::register_tool_capabilities(
        &tools,
        &mcp,
        context,
        Scanner::disabled(),
        RedactionConfig::default(),
    )
    .unwrap();
    let runtime = allow_runtime(router.clone());
    let context = runtime.invocation(
        RequestId::from("mcp-long-output"),
        PluginId::from("agent.tools"),
        "agent",
    );
    let output = router
        .invoke("mcp__db__echo", context, json!({}))
        .await
        .unwrap();
    assert!(
        output["content"]
            .as_str()
            .unwrap()
            .contains("<persisted-output>")
    );
    assert!(
        work_dir
            .join(".tact/tool-results/mcp-long-output.txt")
            .is_file()
    );
    let _ = std::fs::remove_dir_all(work_dir);
}

struct DenyPermissions;

#[async_trait]
impl PermissionService for DenyPermissions {
    async fn check(
        &self,
        _declaration: &CapabilityDeclaration,
        _context: &InvocationContext,
        _input: &Value,
    ) -> std::result::Result<(), KernelError> {
        Err(KernelError::permission_denied("denied in test"))
    }
}

struct AllowPermissions;

#[async_trait]
impl PermissionService for AllowPermissions {
    async fn check(
        &self,
        _declaration: &CapabilityDeclaration,
        _context: &InvocationContext,
        _input: &Value,
    ) -> std::result::Result<(), KernelError> {
        Ok(())
    }
}

struct NoEvents;

#[async_trait]
impl EventService for NoEvents {
    async fn publish(&self, _event: RuntimeEvent) -> std::result::Result<(), KernelError> {
        Ok(())
    }
}

struct NoTrajectory;

#[async_trait]
impl TrajectoryService for NoTrajectory {
    async fn append(
        &self,
        _trajectory_id: Option<&TrajectoryId>,
        _run_id: Option<&RunId>,
        _event: RuntimeEvent,
    ) -> std::result::Result<(), KernelError> {
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
    ) -> std::result::Result<Option<Value>, KernelError> {
        Ok(None)
    }

    async fn set(
        &self,
        _namespace: &str,
        _key: &str,
        _value: Value,
    ) -> std::result::Result<(), KernelError> {
        Ok(())
    }
}
