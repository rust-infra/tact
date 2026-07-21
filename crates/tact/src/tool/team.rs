use anyhow::Result;
use schemars::JsonSchema;
use serde::Deserialize;
use tool_refactor_macros::tool;

use crate::tool::ToolContext;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SpawnTeammateInput {
    pub name: String,
    pub role: String,
}

#[tool(name = "spawn_teammate", description = "Create a named teammate.")]
pub async fn spawn_teammate(ctx: ToolContext, input: SpawnTeammateInput) -> Result<String> {
    ctx.teammate_manager.spawn_teammate(input.name, input.role)
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListTeammatesInput {}

#[tool(name = "list_teammates", description = "List teammates.")]
pub async fn list_teammates(ctx: ToolContext, _input: ListTeammatesInput) -> Result<String> {
    ctx.teammate_manager.list_teammates()
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SendMessageInput {
    pub from: String,
    pub to: String,
    pub body: String,
}

#[tool(name = "send_message", description = "Send a message to a teammate inbox.")]
pub async fn send_message(ctx: ToolContext, input: SendMessageInput) -> Result<String> {
    ctx.teammate_manager.send_message(input.from, input.to, input.body)
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct BroadcastInput {
    pub from: String,
    pub body: String,
}

#[tool(name = "broadcast", description = "Broadcast a message to all teammates.")]
pub async fn broadcast(ctx: ToolContext, input: BroadcastInput) -> Result<String> {
    ctx.teammate_manager.broadcast(input.from, input.body)
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ReadInboxInput {
    pub owner: String,
}

#[tool(name = "read_inbox", description = "Read a teammate inbox.")]
pub async fn read_inbox(ctx: ToolContext, input: ReadInboxInput) -> Result<String> {
    ctx.teammate_manager.read_inbox(&input.owner)
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ProtocolInput {
    pub from: String,
    pub to: String,
    pub body: String,
}

#[tool(name = "plan_approval", description = "Send a durable plan approval protocol message.")]
pub async fn plan_approval(ctx: ToolContext, input: ProtocolInput) -> Result<String> {
    ctx.teammate_manager.protocol_request(input.from, input.to, "plan_approval".to_string(), input.body)
}

#[tool(name = "shutdown_request", description = "Send a shutdown request protocol message.")]
pub async fn shutdown_request(ctx: ToolContext, input: ProtocolInput) -> Result<String> {
    ctx.teammate_manager.protocol_request(input.from, input.to, "shutdown_request".to_string(), input.body)
}

#[tool(name = "shutdown_response", description = "Send a shutdown response protocol message.")]
pub async fn shutdown_response(ctx: ToolContext, input: ProtocolInput) -> Result<String> {
    ctx.teammate_manager.protocol_request(input.from, input.to, "shutdown_response".to_string(), input.body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool::test_support::{run_tool, test_context};

    async fn spawn(context: &ToolContext, name: &str, role: &str) {
        run_tool(context, SpawnTeammateTool, "spawn_teammate", serde_json::json!({ "name": name, "role": role }))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn spawn_teammate_rejects_duplicate_name() {
        let context = test_context("spawn_teammate_rejects_duplicate_name");
        spawn(&context, "alice", "reviewer").await;

        let error = run_tool(
            &context,
            SpawnTeammateTool,
            "spawn_teammate",
            serde_json::json!({ "name": "alice", "role": "other" }),
        )
        .await
        .unwrap_err();

        assert!(error.to_string().contains("already exists"));
    }

    #[tokio::test]
    async fn broadcast_delivers_to_all_teammates() {
        let context = test_context("broadcast_delivers_to_all_teammates");
        spawn(&context, "alice", "reviewer").await;
        spawn(&context, "bob", "tester").await;

        run_tool(
            &context,
            BroadcastTool,
            "broadcast",
            serde_json::json!({
                "from": "lead",
                "body": "Standup in 5"
            }),
        )
        .await
        .unwrap();

        for teammate in ["alice", "bob"] {
            let inbox = run_tool(&context, ReadInboxTool, "read_inbox", serde_json::json!({ "owner": teammate }))
                .await
                .unwrap();
            assert!(inbox.contains("Standup in 5"));
        }
    }

    #[tokio::test]
    async fn plan_approval_sends_protocol_message() {
        let context = test_context("plan_approval_sends_protocol_message");
        spawn(&context, "alice", "reviewer").await;

        let output = run_tool(
            &context,
            PlanApprovalTool,
            "plan_approval",
            serde_json::json!({
                "from": "lead",
                "to": "alice",
                "body": "Approve plan v2"
            }),
        )
        .await
        .unwrap();

        assert!(output.contains("sent protocol request"));
        let inbox =
            run_tool(&context, ReadInboxTool, "read_inbox", serde_json::json!({ "owner": "alice" })).await.unwrap();
        assert!(inbox.contains("Approve plan v2"));
        assert!(inbox.contains("plan_approval"));
    }
}
