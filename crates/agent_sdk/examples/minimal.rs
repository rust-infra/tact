use std::sync::Arc;

use async_trait::async_trait;
use futures_util::stream;
use tact_agent_api::{
    AgentMessage, AgentRole, CancelToken, CursorUpdate, MessageId, MessageOrigin,
    ModelCapabilities, ModelCursor, ModelError, ModelEvent, ModelPort, ModelRequest, ModelResponse,
    ModelStream, StopReason,
};
use tact_agent_sdk::AgentBuilder;

struct EchoModel;

#[async_trait]
impl ModelPort for EchoModel {
    fn capabilities(&self) -> ModelCapabilities {
        ModelCapabilities {
            model: "echo".into(),
            context_window: Some(8_192),
            max_output_tokens: 512,
            supports_streaming: true,
            supports_native_compaction: false,
        }
    }

    async fn stream(
        &self,
        request: ModelRequest,
        _cursor: Option<&ModelCursor>,
        _cancel: CancelToken,
    ) -> Result<ModelStream, ModelError> {
        let text = format!(
            "echo: {}",
            request
                .messages
                .last()
                .map(message_text)
                .unwrap_or_default()
        );
        let response = ModelResponse {
            message_id: MessageId::new("echo-response"),
            message: AgentMessage::text(AgentRole::Assistant, MessageOrigin::Assistant, text),
            stop_reason: StopReason::EndTurn,
            usage: None,
            request_body: None,
            cursor: CursorUpdate::Unchanged,
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

fn message_text(message: &tact_agent_api::ModelMessage) -> String {
    message
        .content
        .iter()
        .filter_map(|block| match block {
            tact_agent_api::AgentContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut session = AgentBuilder::new(Arc::new(EchoModel))
        .system_prompt("You are terse.")
        .max_total_turns(8)
        .build_session("minimal-chat")?;

    let outcome = session.run_text("hello").await?;
    println!("stopped: {:?}", outcome.stop);
    let answer = session
        .last_assistant_message()
        .map(|message| message.text_content())
        .unwrap_or_default();
    println!("answer: {answer}");
    Ok(())
}
