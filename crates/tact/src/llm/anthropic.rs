//! Anthropic LLM adapter.

use anthropic_ai_sdk::types::message::{
    ContentBlock, ContentBlockDelta, CreateMessageParams, MessageClient, MessageError, StopReason,
    StreamEvent, ThinkingType,
};
use futures_util::StreamExt;
use tokio::sync::mpsc::UnboundedSender;

use tact_core::{AgentUpdate, ModelCallParams};

use super::{LlmClient, LlmError};

#[derive(Clone)]
pub struct AnthropicAdapter {
    client: anthropic_ai_sdk::client::AnthropicClient,
}

impl AnthropicAdapter {
    pub fn new(client: anthropic_ai_sdk::client::AnthropicClient) -> Self {
        Self { client }
    }
}

#[async_trait::async_trait]
impl LlmClient for AnthropicAdapter {
    async fn stream_message(
        &self,
        request: &CreateMessageParams,
        ui_tx: Option<UnboundedSender<AgentUpdate>>,
    ) -> Result<(Vec<ContentBlock>, Option<StopReason>), LlmError> {
        let mut response_blocks: Vec<ContentBlock> = Vec::new();
        let mut tool_input_buffers: Vec<String> = Vec::new();
        let mut stop_reason: Option<StopReason> = None;

        let stream = self.client.create_message_streaming(request).await?;
        tokio::pin!(stream);

        while let Some(event_result) = stream.next().await {
            let event = event_result?;
            match event {
                StreamEvent::MessageStart { message } => {
                    if let Some(ref tx) = ui_tx {
                        let _ = tx.send(AgentUpdate::ModelInfo(ModelCallParams {
                            model: message.model,
                            max_tokens: request.max_tokens,
                            thinking_budget: request
                                .thinking
                                .as_ref()
                                .map(|t| t.budget_tokens as u32),
                            reasoning_effort: request.thinking.as_ref().map(|t| match t.type_ {
                                ThinkingType::Enabled => "high".to_string(),
                            }),
                            extra_body: request
                                .thinking
                                .as_ref()
                                .map(|t| serde_json::json!({"thinking": t}).to_string()),
                        }));
                    }
                }
                StreamEvent::ContentBlockStart {
                    index,
                    content_block,
                } => {
                    if index >= response_blocks.len() {
                        response_blocks.resize(
                            index + 1,
                            ContentBlock::Text {
                                text: String::new(),
                            },
                        );
                        tool_input_buffers.resize(index + 1, String::new());
                    }
                    match &content_block {
                        ContentBlock::Text { text } => {
                            tool_input_buffers[index].clear();
                            if !text.is_empty() {
                                if let Some(ref tx) = ui_tx {
                                    let _ = tx.send(AgentUpdate::StreamChunk(text.clone()));
                                }
                            }
                        }
                        ContentBlock::Thinking { thinking, .. } => {
                            tool_input_buffers[index].clear();
                            if !thinking.is_empty() {
                                if let Some(ref tx) = ui_tx {
                                    let _ = tx.send(AgentUpdate::ThinkingChunk(thinking.clone()));
                                }
                            }
                        }
                        ContentBlock::ToolUse { .. } => {
                            tool_input_buffers[index].clear();
                        }
                        _ => {}
                    }
                    response_blocks[index] = content_block;
                }
                StreamEvent::ContentBlockDelta { index, delta } => match delta {
                    ContentBlockDelta::TextDelta { text } => {
                        if let Some(ContentBlock::Text { text: existing }) =
                            response_blocks.get_mut(index)
                        {
                            existing.push_str(&text);
                            if let Some(ref tx) = ui_tx {
                                let _ = tx.send(AgentUpdate::StreamChunk(text));
                            }
                        }
                    }
                    ContentBlockDelta::ThinkingDelta { thinking } => {
                        if let Some(ref tx) = ui_tx {
                            let _ = tx.send(AgentUpdate::ThinkingChunk(thinking));
                        }
                    }
                    ContentBlockDelta::InputJsonDelta { partial_json } => {
                        if index < tool_input_buffers.len() {
                            tool_input_buffers[index].push_str(&partial_json);
                        }
                    }
                    _ => {}
                },
                StreamEvent::ContentBlockStop { index } => {
                    if let Some(ContentBlock::ToolUse {
                        input: existing, ..
                    }) = response_blocks.get_mut(index)
                    {
                        if index < tool_input_buffers.len() {
                            if let Ok(value) = serde_json::from_str(&tool_input_buffers[index]) {
                                *existing = value;
                            }
                        }
                    }
                }
                StreamEvent::MessageDelta { delta, usage } => {
                    stop_reason = delta.stop_reason;
                    if let Some(usage) = usage {
                        if let Some(ref tx) = ui_tx {
                            let _ = tx.send(AgentUpdate::TokenUsage {
                                prompt: usage.input_tokens,
                                completion: usage.output_tokens,
                                total: usage.input_tokens + usage.output_tokens,
                                prompt_cache_hit_tokens: 0,
                                prompt_cache_miss_tokens: 0,
                            });
                        }
                    }
                }
                StreamEvent::MessageStop => break,
                StreamEvent::Ping => {}
                StreamEvent::Error { error } => {
                    return Err(LlmError::Anthropic(MessageError::ApiError(format!(
                        "stream error: {:?}",
                        error
                    ))));
                }
            }
        }

        Ok((response_blocks, stop_reason))
    }

    async fn create_message(
        &self,
        request: &CreateMessageParams,
    ) -> Result<(Vec<ContentBlock>, Option<StopReason>), LlmError> {
        let response = self.client.create_message(Some(request)).await?;
        Ok((response.content, response.stop_reason))
    }
}
