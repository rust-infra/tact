//! Compile-time and small behavioral checks for the public `tact` facade.
//!
//! These tests intentionally use only public constructors and value types. They
//! provide a stable inventory while the runtime is split into owner modules;
//! they do not exercise a real provider, filesystem, MCP process, or session.

use anyhow::Result;
use async_trait::async_trait;
use serde_json::{Value, json};
use tact::{
    Agent, AgentRuntime, AgentSystemPrompt, LoopState,
    hook::{self, HookControl, HookContextChunk, SessionStartContext, ToolUse},
    mcp::MCPToolRouter,
    permission::{PermissionManager, PermissionMode},
    tool::{Tool, ToolCallResult, ToolContext, ToolMetadata, ToolRouter},
};
use tact_llm::{LlmProvider, ProviderKind, Tool as ToolSpec};

struct EchoTool;

#[async_trait]
impl Tool for EchoTool {
    fn metadata(&self) -> &'static ToolMetadata {
        static METADATA: ToolMetadata = ToolMetadata::read_json(
            "architecture_echo",
            "Returns a fixed compatibility value.",
            "Architecture Echo",
        );
        &METADATA
    }

    fn input_schema(&self) -> Value {
        json!({"type": "object"})
    }

    async fn call(&self, _context: ToolContext, _input: Value) -> Result<ToolCallResult> {
        Ok(ToolCallResult::text("ok"))
    }
}

#[test]
fn architecture_public_constructor_and_builder_signatures_remain_available() {
    let _new: fn(
        LlmProvider,
        ToolContext,
        ToolRouter,
        MCPToolRouter,
        PermissionManager,
        AgentSystemPrompt,
    ) -> Agent = Agent::new;
    let _with_provider_kind: fn(Agent, ProviderKind) -> Agent = Agent::with_provider_kind;
    let _with_max_turns: fn(Agent, Option<u32>) -> Agent = Agent::with_max_turns;
    let _with_ui_channel: fn(
        Agent,
        tokio::sync::mpsc::UnboundedSender<tact_protocol::AgentUpdate>,
    ) -> Agent = Agent::with_ui_channel;
    fn assert_agent_methods(agent: &mut Agent, content: &[tact_llm::ContentBlock]) {
        let _ = agent.agent_loop(None);
        let _ = agent.execute_tool_call(content);
    }
    let _ = assert_agent_methods as fn(&mut Agent, &[tact_llm::ContentBlock]);

    // These aliases are part of the facade contract and are used by legacy
    // hook implementations, even though the underlying type is `Agent`.
    fn accepts_loop_state(_: &LoopState) {}
    fn accepts_runtime(_: &AgentRuntime) {}
    accepts_loop_state as fn(&LoopState);
    accepts_runtime as fn(&AgentRuntime);
}

#[tokio::test]
async fn architecture_tool_trait_and_router_keep_registration_contract() {
    let router = ToolRouter::new()
        .route(EchoTool)
        .expect("unique tool names remain accepted");
    let specs: Vec<ToolSpec> = router.tool_specs();
    assert_eq!(specs.len(), 1);
    assert_eq!(specs[0].name, "architecture_echo");
    assert_eq!(
        specs[0].description.as_deref(),
        Some("Returns a fixed compatibility value.")
    );

    let resolved = router.resolve("architecture_echo").expect("tool resolves");
    assert_eq!(resolved.metadata().name, "architecture_echo");
}

#[test]
fn architecture_hook_context_and_control_contracts_remain_usable() {
    let chunk = HookContextChunk::new(Some("compat-test"), "context");
    assert_eq!(chunk.source.as_deref(), Some("compat-test"));
    assert_eq!(chunk.text, "context");

    let framed = hook::frame_hook_context(Some("compat-test"), "context");
    assert!(hook::is_hook_context_text(&framed));
    assert_eq!(hook::hook_context_source(&framed), Some("compat-test"));
    assert_eq!(hook::hook_context_body(&framed), "context");

    let mut session = SessionStartContext::default();
    session.push_additional_context(Some("compat-test"), "  injected  ");
    session.push_additional_context(None, "   ");
    assert_eq!(session.additional_contexts.len(), 1);
    assert_eq!(HookControl::default(), HookControl::Continue);

    let mut use_payload = ToolUse {
        id: "call-1".into(),
        name: "architecture_echo".into(),
        input: json!({"value": 1}),
    };
    use_payload.input["value"] = json!(2);
    assert_eq!(use_payload.name, "architecture_echo");
}

#[test]
fn architecture_permission_and_mcp_facades_keep_current_value_contracts() {
    let permissions = PermissionManager::try_new(PermissionMode::Default)
        .expect("isolated permission manager remains constructible");
    assert_eq!(permissions.mode(), PermissionMode::Default);
    assert_eq!(PermissionMode::Auto.hook_name(), "acceptEdits");

    let router = MCPToolRouter::new();
    assert!(MCPToolRouter::is_mcp_tool("mcp__server__tool"));
    assert!(!MCPToolRouter::is_mcp_tool("architecture_echo"));
    assert_eq!(
        router.server_name_for("mcp__server__tool").as_deref(),
        Some("server")
    );
    assert_eq!(router.server_name_for("architecture_echo"), None);
}
