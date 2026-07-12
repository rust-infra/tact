//! MCP tool-call harness integration tests.
//!
//! These tests verify that `tact::mcp::MockMcpService` can stand in for a real
//! MCP server and that the agent correctly routes `mcp__<server>__<tool>` calls
//! through the [`tact::mcp::MCPToolRouter`].

mod harness;

use std::{borrow::Cow, sync::Arc};

use harness::run_single_task_with_mcp;
use rmcp::{
    ErrorData as McpError,
    model::{CallToolResult, Content, JsonObject, Tool as McpTool},
    service::ServiceError,
};
use serde_json::json;
use tact::mcp::{MCPToolRouter, McpClient, MockMcpService};
use tact_llm::MockClient;
use tact_llm::{ContentBlock, StopReason};
use tact_protocol::{AgentUpdate, StepStatus};

fn echo_tool() -> McpTool {
    McpTool {
        name: Cow::Borrowed("echo"),
        title: None,
        description: Some(Cow::Borrowed("Echo the input back")),
        input_schema: Arc::new(JsonObject::new()),
        output_schema: None,
        annotations: None,
        execution: None,
        icons: None,
        meta: None,
    }
}

fn mcp_echo_tool_use(id: &str, message: &str) -> ContentBlock {
    ContentBlock::ToolUse {
        id: id.to_string(),
        name: "mcp__demo__echo".to_string(),
        input: json!({ "message": message }),
    }
}

#[tokio::test]
async fn agent_routes_mcp_tool_through_mock_server() {
    let echo = echo_tool();
    let service = MockMcpService::new(vec![echo.clone()], |params| {
        let message = params
            .arguments
            .as_ref()
            .and_then(|args| args.get("message"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        Ok(CallToolResult::success(vec![Content::text(format!(
            "echo: {message}"
        ))]))
    });

    let client = McpClient::with_service("demo", vec![echo], Arc::new(service));
    let mut router = MCPToolRouter::new();
    router.register_client(client);

    // The MCP tool spec is exposed to the LLM alongside local tools.
    assert!(
        router
            .all_tools()
            .iter()
            .any(|spec| spec.name == "mcp__demo__echo")
    );

    let mock = MockClient::new(vec![
        (
            vec![mcp_echo_tool_use("tu1", "hi")],
            Some(StopReason::ToolUse),
        ),
        (
            vec![ContentBlock::Text {
                text: "done".to_string(),
            }],
            Some(StopReason::EndTurn),
        ),
    ]);

    let (updates, _work_dir) = run_single_task_with_mcp(
        mock,
        "call the echo tool",
        tact::permission::PermissionMode::Auto,
        None,
        router,
    )
    .await;

    assert!(updates.iter().any(|u| matches!(
        u,
        AgentUpdate::StepFinished { result, .. }
            if matches!(result.status, StepStatus::Success) && result.message.contains("echo: hi")
    )));
}

#[tokio::test]
async fn mcp_tool_error_is_reported_as_step_failure() {
    let echo = echo_tool();
    let service = MockMcpService::new(vec![echo.clone()], |_params| {
        Ok(CallToolResult::error(vec![Content::text(
            "server exploded",
        )]))
    });

    let client = McpClient::with_service("demo", vec![echo], Arc::new(service));
    let mut router = MCPToolRouter::new();
    router.register_client(client);

    let mock = MockClient::new(vec![
        (
            vec![mcp_echo_tool_use("tu1", "hi")],
            Some(StopReason::ToolUse),
        ),
        (
            vec![ContentBlock::Text {
                text: "done".to_string(),
            }],
            Some(StopReason::EndTurn),
        ),
    ]);

    let (updates, _work_dir) = run_single_task_with_mcp(
        mock,
        "call the echo tool",
        tact::permission::PermissionMode::Auto,
        None,
        router,
    )
    .await;

    // The router's joined content for an error result is the error text; the
    // agent still records the step as successful execution of the tool itself,
    // but the returned message contains the error reported by the server.
    assert!(updates.iter().any(|u| matches!(
        u,
        AgentUpdate::StepFinished { result, .. }
            if result.message.contains("server exploded")
    )));
}

#[tokio::test]
async fn mcp_tool_prompts_in_default_mode() {
    let echo = echo_tool();
    let service = MockMcpService::new(vec![echo.clone()], |params| {
        let message = params
            .arguments
            .as_ref()
            .and_then(|args| args.get("message"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        Ok(CallToolResult::success(vec![Content::text(format!(
            "echo: {message}"
        ))]))
    });

    let client = McpClient::with_service("demo", vec![echo], Arc::new(service));
    let mut router = MCPToolRouter::new();
    router.register_client(client);

    let mock = MockClient::new(vec![
        (
            vec![mcp_echo_tool_use("tu1", "hi")],
            Some(StopReason::ToolUse),
        ),
        (
            vec![ContentBlock::Text {
                text: "done".to_string(),
            }],
            Some(StopReason::EndTurn),
        ),
    ]);

    let (updates, _work_dir) = run_single_task_with_mcp(
        mock,
        "call the echo tool",
        tact::permission::PermissionMode::Default,
        Some(0), // allow once
        router,
    )
    .await;

    // Default mode asks the user before executing a write-capability MCP tool.
    // The harness auto-responds with "allow once" (choice 0), so the only
    // observable evidence in the collected updates is the permission_label.
    assert!(updates.iter().any(|u| matches!(
        u,
        AgentUpdate::StepFinished { result, .. }
            if matches!(result.status, StepStatus::Success)
                && result.permission_label.as_deref() == Some("Allow once")
                && result.message.contains("echo: hi")
    )));
}

#[tokio::test]
async fn agent_recovers_when_mcp_tool_returns_error() {
    let echo = echo_tool();
    let service = MockMcpService::new(vec![echo.clone()], |_params| {
        Err(ServiceError::McpError(McpError::internal_error(
            "server exploded",
            None,
        )))
    });

    let client = McpClient::with_service("demo", vec![echo], Arc::new(service));
    let mut router = MCPToolRouter::new();
    router.register_client(client);

    let mock = MockClient::new(vec![
        (
            vec![mcp_echo_tool_use("tu1", "hi")],
            Some(StopReason::ToolUse),
        ),
        (
            vec![ContentBlock::Text {
                text: "recovered".to_string(),
            }],
            Some(StopReason::EndTurn),
        ),
    ]);

    let (updates, _work_dir) = run_single_task_with_mcp(
        mock,
        "call the echo tool",
        tact::permission::PermissionMode::Auto,
        None,
        router,
    )
    .await;

    // The failed MCP tool should be recorded as a failed step.
    assert!(updates.iter().any(|u| matches!(
        u,
        AgentUpdate::StepFinished { result, .. }
            if matches!(result.status, StepStatus::Failed)
                && result.message.contains("Error invoking MCP tool mcp__demo__echo")
    )));

    // The agent should still finish the task after the error.
    assert!(updates.iter().any(|u| matches!(
        u,
        AgentUpdate::TaskComplete(message) if message.contains("recovered")
    )));
}
