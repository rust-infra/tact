use crate::tool::ToolMetadata;
use anyhow::{Result, anyhow};
use schemars::JsonSchema;
use serde::Deserialize;
use tool_refactor_macros::tool;

use crate::tool::DomainHandles;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SpawnTeammateInput {
    pub name: String,
    pub role: String,
}

pub const SPAWN_TEAMMATE_METADATA: ToolMetadata = ToolMetadata::team_write(
    "spawn_teammate",
    "Create a named teammate.",
    "👥 Team Spawn",
);

#[tool]
/// # Errors
///
/// Returns an error if a teammate with the same name already exists.
pub async fn spawn_teammate(handles: DomainHandles, input: SpawnTeammateInput) -> Result<String> {
    let Some(teammates) = handles.teammate_manager else {
        return Err(anyhow!(
            "team tools are unavailable: this host has no teammate manager"
        ));
    };
    teammates.spawn_teammate(input.name, input.role).await
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListTeammatesInput {}

pub const LIST_TEAMMATES_METADATA: ToolMetadata =
    ToolMetadata::read_json("list_teammates", "List teammates.", "👥 Team List");

#[tool]
/// # Errors
///
/// Returns an error if the teammate manager fails to retrieve the list.
pub async fn list_teammates(handles: DomainHandles, _input: ListTeammatesInput) -> Result<String> {
    let Some(teammates) = handles.teammate_manager else {
        return Err(anyhow!(
            "team tools are unavailable: this host has no teammate manager"
        ));
    };
    teammates.list_teammates().await
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SendMessageInput {
    pub from: String,
    pub to: String,
    pub body: String,
}

pub const SEND_MESSAGE_METADATA: ToolMetadata = ToolMetadata::team_write(
    "send_message",
    "Send a message to a teammate inbox.",
    "✉️ Team Send",
);

#[tool]
/// # Errors
///
/// Returns an error if the recipient does not exist or the teammate
/// manager fails to send the message.
pub async fn send_message(handles: DomainHandles, input: SendMessageInput) -> Result<String> {
    let Some(teammates) = handles.teammate_manager else {
        return Err(anyhow!(
            "team tools are unavailable: this host has no teammate manager"
        ));
    };
    teammates
        .send_message(input.from, input.to, input.body)
        .await
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct BroadcastInput {
    pub from: String,
    pub body: String,
}

pub const BROADCAST_METADATA: ToolMetadata = ToolMetadata::team_write(
    "broadcast",
    "Broadcast a message to all teammates.",
    "📢 Team Broadcast",
);

#[tool]
/// # Errors
///
/// Returns an error if the teammate manager fails to deliver the broadcast.
pub async fn broadcast(handles: DomainHandles, input: BroadcastInput) -> Result<String> {
    let Some(teammates) = handles.teammate_manager else {
        return Err(anyhow!(
            "team tools are unavailable: this host has no teammate manager"
        ));
    };
    teammates.broadcast(input.from, input.body).await
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ReadInboxInput {
    pub owner: String,
}

pub const READ_INBOX_METADATA: ToolMetadata =
    ToolMetadata::read_json("read_inbox", "Read a teammate inbox.", "📬 Team Inbox");

#[tool]
/// # Errors
///
/// Returns an error if the owner does not exist or the teammate
/// manager fails to read the inbox.
pub async fn read_inbox(handles: DomainHandles, input: ReadInboxInput) -> Result<String> {
    let Some(teammates) = handles.teammate_manager else {
        return Err(anyhow!(
            "team tools are unavailable: this host has no teammate manager"
        ));
    };
    teammates.read_inbox(&input.owner).await
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ProtocolInput {
    pub from: String,
    pub to: String,
    pub body: String,
}

pub const PLAN_APPROVAL_METADATA: ToolMetadata = ToolMetadata::team_write(
    "plan_approval",
    "Send a durable plan approval protocol message.",
    "✅ Team Approve",
);

#[tool]
/// # Errors
///
/// Returns an error if the recipient does not exist or the teammate
/// manager fails to send the protocol request.
pub async fn plan_approval(handles: DomainHandles, input: ProtocolInput) -> Result<String> {
    let Some(teammates) = handles.teammate_manager else {
        return Err(anyhow!(
            "team tools are unavailable: this host has no teammate manager"
        ));
    };
    teammates
        .protocol_request(
            input.from,
            input.to,
            "plan_approval".to_string(),
            input.body,
        )
        .await
}

pub const SHUTDOWN_REQUEST_METADATA: ToolMetadata = ToolMetadata::team_write(
    "shutdown_request",
    "Send a shutdown request protocol message.",
    "🔌 Shutdown Request",
);

#[tool]
/// # Errors
///
/// Returns an error if the recipient does not exist or the teammate
/// manager fails to send the protocol request.
pub async fn shutdown_request(handles: DomainHandles, input: ProtocolInput) -> Result<String> {
    let Some(teammates) = handles.teammate_manager else {
        return Err(anyhow!(
            "team tools are unavailable: this host has no teammate manager"
        ));
    };
    teammates
        .protocol_request(
            input.from,
            input.to,
            "shutdown_request".to_string(),
            input.body,
        )
        .await
}

pub const SHUTDOWN_RESPONSE_METADATA: ToolMetadata = ToolMetadata::team_write(
    "shutdown_response",
    "Send a shutdown response protocol message.",
    "🔌 Shutdown Response",
);

#[tool]
/// # Errors
///
/// Returns an error if the recipient does not exist or the teammate
/// manager fails to send the protocol request.
pub async fn shutdown_response(handles: DomainHandles, input: ProtocolInput) -> Result<String> {
    let Some(teammates) = handles.teammate_manager else {
        return Err(anyhow!(
            "team tools are unavailable: this host has no teammate manager"
        ));
    };
    teammates
        .protocol_request(
            input.from,
            input.to,
            "shutdown_response".to_string(),
            input.body,
        )
        .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool::test_support::{run_tool, test_context};

    async fn spawn(context: &crate::tool::ToolContext, name: &str, role: &str) {
        run_tool(
            context,
            SpawnTeammateTool,
            "spawn_teammate",
            serde_json::json!({ "name": name, "role": role }),
        )
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
            let inbox = run_tool(
                &context,
                ReadInboxTool,
                "read_inbox",
                serde_json::json!({ "owner": teammate }),
            )
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
        let inbox = run_tool(
            &context,
            ReadInboxTool,
            "read_inbox",
            serde_json::json!({ "owner": "alice" }),
        )
        .await
        .unwrap();
        assert!(inbox.contains("Approve plan v2"));
        assert!(inbox.contains("plan_approval"));
    }
}
