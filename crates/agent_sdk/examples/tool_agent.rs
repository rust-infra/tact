use std::sync::Arc;

use async_trait::async_trait;
use futures_util::stream;
use tact_agent_api::{
    AgentContentBlock, AgentMessage, AgentRole, CancelToken, CursorUpdate, MessageId,
    MessageOrigin, ModelCapabilities, ModelCursor, ModelError, ModelEvent, ModelPort, ModelRequest,
    ModelResponse, ModelStream, StopReason, ToolError, ToolExecutionContext, ToolExecutionResult,
    ToolOutcome, ToolSpec,
};
use tact_agent_sdk::{AgentBuilder, AllowAllGate, ToolHandler, ToolSet};

struct ToolModel {
    calls: std::sync::Mutex<u8>,
}

#[async_trait]
impl ModelPort for ToolModel {
    fn capabilities(&self) -> ModelCapabilities {
        ModelCapabilities {
            model: "tool-demo".into(),
            context_window: 8_192,
            max_output_tokens: 512,
            supports_streaming: true,
            supports_native_compaction: false,
        }
    }

    async fn stream(
        &self,
        _request: ModelRequest,
        _cursor: Option<&ModelCursor>,
        _cancel: CancelToken,
    ) -> Result<ModelStream, ModelError> {
        let mut calls = self.calls.lock().expect("calls lock");
        let response = if *calls == 0 {
            *calls += 1;
            ModelResponse {
                message_id: MessageId::new("tool-call"),
                message: AgentMessage::new(
                    AgentRole::Assistant,
                    MessageOrigin::Assistant,
                    vec![AgentContentBlock::ToolUse {
                        id: "call-1".into(),
                        name: "echo".into(),
                        input: serde_json::json!({"text": "hello"}),
                    }],
                ),
                stop_reason: StopReason::ToolUse,
                usage: None,
                request_body: None,
                cursor: CursorUpdate::Unchanged,
            }
        } else {
            ModelResponse {
                message_id: MessageId::new("done"),
                message: AgentMessage::text(
                    AgentRole::Assistant,
                    MessageOrigin::Assistant,
                    "tool result received",
                ),
                stop_reason: StopReason::EndTurn,
                usage: None,
                request_body: None,
                cursor: CursorUpdate::Unchanged,
            }
        };
        let events = vec![
            Ok(ModelEvent::MessageStarted {
                message_id: response.message_id.clone(),
                role: AgentRole::Assistant,
            }),
            Ok(ModelEvent::Finished(response)),
        ];
        Ok(Box::pin(stream::iter(events)))
    }

    async fn complete(
        &self,
        _request: ModelRequest,
        _cursor: Option<&ModelCursor>,
        _cancel: CancelToken,
    ) -> Result<ModelResponse, ModelError> {
        Err(ModelError::unsupported("complete"))
    }
}

struct EchoTool;

#[async_trait]
impl ToolHandler for EchoTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "echo".into(),
            description: "Echo text".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {"text": {"type": "string"}},
                "required": ["text"]
            }),
        }
    }

    async fn execute(
        &self,
        input: serde_json::Value,
        _context: ToolExecutionContext,
    ) -> Result<ToolExecutionResult, ToolError> {
        let text = input
            .get("text")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| ToolError::new(tact_agent_api::ToolErrorKind::InvalidInput, "text"))?;
        Ok(ToolExecutionResult::success(ToolOutcome::text(text)))
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let tools = ToolSet::new().add(EchoTool)?;
    let mut session = AgentBuilder::new(Arc::new(ToolModel {
        calls: std::sync::Mutex::new(0),
    }))
    .tools(Arc::new(tools))
    .gate(Arc::new(AllowAllGate))
    .build_session("tool-chat")?;

    let outcome = session.run_text("use echo").await?;
    println!("stopped: {:?}", outcome.stop);
    Ok(())
}
