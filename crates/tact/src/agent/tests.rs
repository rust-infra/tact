use sqlx::Row;
use tact_agent_api::{AgentContentBlock, AgentMessage, AgentRole, MessageOrigin};
use tact_llm::{
    ContentBlock, CreateMessageParams, LlmError, LlmProvider, Message, MessageContent, MessageKind,
    MockClient, ProviderConversationState, ProviderKind, Role, StopReason,
};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::*;
use crate::store::SessionStore;
use crate::tool::test_support::test_context;

fn ensure_config() {
    crate::config::test_support::install_default();
}

fn make_text_block(content: &str) -> ContentBlock {
    ContentBlock::Text {
        text: content.to_string(),
    }
}

/// Hash the canonical message shape the kernel sends to the provider.
fn wire_context_hash(messages: &[Message]) -> String {
    let canonical = messages
        .iter()
        .map(tact_llm::neutral::agent_message)
        .map(|message| tact_llm::neutral::llm_message(&message))
        .collect::<Vec<_>>();
    tact_llm::context_hash(&canonical).unwrap()
}

/// Blocks of a message (`MessageContent::Text` carries none).
fn message_blocks(message: &Message) -> &[ContentBlock] {
    match &message.content {
        MessageContent::Blocks { content } => content,
        MessageContent::Text { .. } => &[],
    }
}

/// `(tool_use ids, ids answered by a tool_result)` in message order.
fn tool_pairing(messages: &[Message]) -> (Vec<String>, Vec<String>) {
    let mut uses = Vec::new();
    let mut results = Vec::new();
    for block in messages.iter().flat_map(message_blocks) {
        match block {
            ContentBlock::ToolUse { id, .. } => uses.push(id.clone()),
            ContentBlock::ToolResult { tool_use_id, .. } => results.push(tool_use_id.clone()),
            _ => {}
        }
    }
    (uses, results)
}

/// Content of the `ToolResult` answering `tool_use_id`.
fn tool_result_for(messages: &[Message], tool_use_id: &str) -> Option<String> {
    messages
        .iter()
        .flat_map(message_blocks)
        .find_map(|block| match block {
            ContentBlock::ToolResult {
                tool_use_id: id,
                content,
                ..
            } if id == tool_use_id => Some(content.clone()),
            _ => None,
        })
}

fn mcp_client_with_instructions(server: &str, instructions: &str) -> crate::mcp::McpClient {
    use std::borrow::Cow;
    use std::sync::Arc;

    use rmcp::model::{Content, JsonObject, Tool as McpTool};
    use serde_json::json;

    let tool = McpTool {
        name: Cow::Borrowed("recall"),
        title: None,
        description: Some(Cow::Borrowed("Recall a note")),
        input_schema: Arc::new(JsonObject::from_iter([(
            "type".to_string(),
            json!("object"),
        )])),
        output_schema: None,
        annotations: None,
        execution: None,
        icons: None,
        meta: None,
    };
    let service = crate::mcp::MockMcpService::new(vec![tool.clone()], |_| {
        Ok(rmcp::model::CallToolResult::success(vec![Content::text(
            "ok",
        )]))
    })
    .with_instructions(instructions);
    crate::mcp::McpClient::with_service(server, vec![tool], Arc::new(service))
}

fn chat_completions_test_agent(context_name: &str) -> Agent {
    chat_completions_test_agent_for(context_name, test_context(context_name))
}

fn chat_completions_test_agent_for(context_name: &str, context: crate::tool::ToolContext) -> Agent {
    Agent::new(
        LlmProvider::Mock(MockClient::new(vec![])),
        context,
        crate::tool::toolset(),
        crate::mcp::MCPToolRouter::new(),
        crate::permission::PermissionManager::try_new(crate::permission::PermissionMode::Default)
            .unwrap(),
        AgentSystemPrompt::Static(context_name.to_string()),
    )
}

fn responses_test_agent(context_name: &str, base_url: &str) -> Agent {
    Agent::new(
        LlmProvider::OpenAiResponses(tact_llm::openai::responses::OpenAiResponsesAdapter::new(
            "test-key", base_url, None,
        )),
        test_context(context_name),
        crate::tool::toolset(),
        crate::mcp::MCPToolRouter::new(),
        crate::permission::PermissionManager::try_new(crate::permission::PermissionMode::Default)
            .unwrap(),
        AgentSystemPrompt::Static("test".to_string()),
    )
}

mod agent_loop;
mod compaction;
mod events;
mod facade;
mod hooks;
mod prompt;
mod session;
mod tools;
