//! Anthropic LLM adapter.
//!
//! Uses direct HTTP + SSE instead of the SDK's streaming client so that we
//! can gracefully handle new stop_reason values (e.g. `pause_turn`) that the
//! upstream `anthropic-ai-sdk` crate has not yet added to its `StopReason`
//! enum.

use std::time::Duration;

use anthropic_ai_sdk::types::message::{
    ContentBlock, ContentBlockDelta, CreateMessageParams, MessageError, StopReason, StreamUsage,
};
use futures_util::StreamExt;
use reqwest_eventsource::{Event, RequestBuilderExt};
use serde::Deserialize;
use tokio::sync::mpsc::UnboundedSender;

use tact_core::{AgentUpdate, ModelCallParams};

use super::{LlmClient, LlmError};

#[derive(Clone)]
pub struct AnthropicAdapter {
    api_key: String,
    base_url: String,
    api_version: String,
}

impl AnthropicAdapter {
    pub fn new(api_key: impl Into<String>, base_url: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            base_url: base_url.into(),
            api_version: "2023-06-01".to_string(),
        }
    }

    fn messages_url(&self) -> String {
        format!("{}/messages", self.base_url.trim_end_matches('/'))
    }

    fn headers(&self) -> reqwest::header::HeaderMap {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert("x-api-key", self.api_key.parse().unwrap());
        headers.insert("anthropic-version", self.api_version.parse().unwrap());
        headers.insert(
            reqwest::header::CONTENT_TYPE,
            "application/json".parse().unwrap(),
        );
        headers
    }
}

fn parse_stop_reason(reason: Option<String>) -> Option<StopReason> {
    match reason.as_deref() {
        Some("end_turn") => Some(StopReason::EndTurn),
        Some("max_tokens") => Some(StopReason::MaxTokens),
        Some("stop_sequence") => Some(StopReason::StopSequence),
        Some("tool_use") => Some(StopReason::ToolUse),
        Some("refusal") => Some(StopReason::Refusal),
        // `pause_turn` is returned by newer Anthropic models when they want to
        // hand control back to the caller. Treat it the same as `end_turn` for
        // the agent loop.
        Some("pause_turn") => Some(StopReason::EndTurn),
        _ => None,
    }
}

#[derive(Debug, Deserialize)]
struct MessageStartEvent {
    message: MessageStartPayload,
}

#[derive(Debug, Deserialize)]
struct MessageStartPayload {
    model: String,
}

#[derive(Debug, Deserialize)]
struct ContentBlockStartEvent {
    index: usize,
    content_block: ContentBlock,
}

#[derive(Debug, Deserialize)]
struct ContentBlockDeltaEvent {
    index: usize,
    delta: ContentBlockDelta,
}

#[derive(Debug, Deserialize)]
struct ContentBlockStopEvent {
    index: usize,
}

#[derive(Debug, Deserialize)]
struct MessageDeltaEvent {
    delta: MessageDeltaPayload,
    usage: Option<StreamUsage>,
}

#[derive(Debug, Deserialize)]
struct MessageDeltaPayload {
    stop_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct StreamErrorEvent {
    error: StreamErrorPayload,
}

#[derive(Debug, Deserialize)]
struct StreamErrorPayload {
    #[serde(rename = "type")]
    type_: String,
    message: String,
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

        let mut body = serde_json::to_value(request)
            .map_err(|e| LlmError::Anthropic(MessageError::ApiError(e.to_string())))?;
        body["stream"] = serde_json::json!(true);

        let client = reqwest::Client::builder()
            .read_timeout(Duration::from_secs(120))
            .build()
            .map_err(|e| LlmError::Anthropic(MessageError::ApiError(e.to_string())))?;

        let mut event_source = client
            .post(&self.messages_url())
            .headers(self.headers())
            .json(&body)
            .eventsource()
            .map_err(|e| LlmError::Anthropic(MessageError::ApiError(e.to_string())))?;
        event_source.set_retry_policy(Box::new(reqwest_eventsource::retry::Never));

        while let Some(event) = event_source.next().await {
            match event {
                Err(e) => return Err(LlmError::Anthropic(MessageError::ApiError(e.to_string()))),
                Ok(Event::Open) => continue,
                Ok(Event::Message(msg)) => {
                    if msg.data == "[DONE]" {
                        break;
                    }

                    let value: serde_json::Value = serde_json::from_str(&msg.data).map_err(|e| {
                        LlmError::Anthropic(MessageError::ApiError(format!(
                            "Failed to parse SSE event JSON: {e}. Data: {}",
                            msg.data
                        )))
                    })?;

                    let event_type = value["type"].as_str().ok_or_else(|| {
                        LlmError::Anthropic(MessageError::ApiError(format!(
                            "SSE event missing type field: {}",
                            msg.data
                        )))
                    })?;

                    match event_type {
                        "message_start" => {
                            let start: MessageStartEvent = serde_json::from_value(value)
                                .map_err(|e| {
                                    LlmError::Anthropic(MessageError::ApiError(format!(
                                        "Failed to parse message_start: {e}"
                                    )))
                                })?;
                            if let Some(ref tx) = ui_tx {
                                let _ = tx.send(AgentUpdate::ModelInfo(ModelCallParams {
                                    model: start.message.model,
                                    max_tokens: request.max_tokens,
                                    thinking_budget: request
                                        .thinking
                                        .as_ref()
                                        .map(|t| t.budget_tokens as u32),
                                    reasoning_effort: request.thinking.as_ref().map(|_| {
                                        "high".to_string()
                                    }),
                                    extra_body: request.thinking.as_ref().map(|t| {
                                        serde_json::json!({"thinking": t}).to_string()
                                    }),
                                }));
                            }
                        }
                        "content_block_start" => {
                            let start: ContentBlockStartEvent = serde_json::from_value(value)
                                .map_err(|e| {
                                    LlmError::Anthropic(MessageError::ApiError(format!(
                                        "Failed to parse content_block_start: {e}"
                                    )))
                                })?;
                            let index = start.index;
                            if index >= response_blocks.len() {
                                response_blocks.resize(
                                    index + 1,
                                    ContentBlock::Text {
                                        text: String::new(),
                                    },
                                );
                                tool_input_buffers.resize(index + 1, String::new());
                            }
                            match &start.content_block {
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
                                            let _ = tx
                                                .send(AgentUpdate::ThinkingChunk(thinking.clone()));
                                        }
                                    }
                                }
                                ContentBlock::ToolUse { .. } => {
                                    tool_input_buffers[index].clear();
                                }
                                _ => {}
                            }
                            response_blocks[index] = start.content_block;
                        }
                        "content_block_delta" => {
                            let delta_event: ContentBlockDeltaEvent =
                                serde_json::from_value(value).map_err(|e| {
                                    LlmError::Anthropic(MessageError::ApiError(format!(
                                        "Failed to parse content_block_delta: {e}"
                                    )))
                                })?;
                            let index = delta_event.index;
                            match delta_event.delta {
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
                                    if let Some(ContentBlock::Thinking { thinking: existing, .. }) =
                                        response_blocks.get_mut(index)
                                    {
                                        existing.push_str(&thinking);
                                    }
                                    if let Some(ref tx) = ui_tx {
                                        let _ = tx.send(AgentUpdate::ThinkingChunk(thinking));
                                    }
                                }
                                ContentBlockDelta::InputJsonDelta { partial_json } => {
                                    if index < tool_input_buffers.len() {
                                        tool_input_buffers[index].push_str(&partial_json);
                                    }
                                }
                                ContentBlockDelta::SignatureDelta { signature } => {
                                    if let Some(ContentBlock::Thinking {
                                        signature: existing,
                                        ..
                                    }) = response_blocks.get_mut(index)
                                    {
                                        existing.push_str(&signature);
                                    }
                                }
                            }
                        }
                        "content_block_stop" => {
                            let stop: ContentBlockStopEvent = serde_json::from_value(value)
                                .map_err(|e| {
                                    LlmError::Anthropic(MessageError::ApiError(format!(
                                        "Failed to parse content_block_stop: {e}"
                                    )))
                                })?;
                            if let Some(ContentBlock::ToolUse {
                                input: existing, ..
                            }) = response_blocks.get_mut(stop.index)
                            {
                                if stop.index < tool_input_buffers.len() {
                                    if let Ok(value) =
                                        serde_json::from_str(&tool_input_buffers[stop.index])
                                    {
                                        *existing = value;
                                    }
                                }
                            }
                        }
                        "message_delta" => {
                            let delta_event: MessageDeltaEvent = serde_json::from_value(value)
                                .map_err(|e| {
                                    LlmError::Anthropic(MessageError::ApiError(format!(
                                        "Failed to parse message_delta: {e}"
                                    )))
                                })?;
                            stop_reason = parse_stop_reason(delta_event.delta.stop_reason);
                            if let Some(usage) = delta_event.usage {
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
                        "message_stop" => break,
                        "ping" => {}
                        "error" => {
                            let err: StreamErrorEvent = serde_json::from_value(value).map_err(|e| {
                                LlmError::Anthropic(MessageError::ApiError(format!(
                                    "Failed to parse error event: {e}"
                                )))
                            })?;
                            return Err(LlmError::Anthropic(MessageError::ApiError(format!(
                                "stream error: {} - {}",
                                err.error.type_, err.error.message
                            ))));
                        }
                        other => {
                            tracing::warn!("Unknown Anthropic SSE event type: {}", other);
                        }
                    }
                }
            }
        }

        Ok((response_blocks, stop_reason))
    }

    async fn create_message(
        &self,
        request: &CreateMessageParams,
    ) -> Result<(Vec<ContentBlock>, Option<StopReason>), LlmError> {
        let mut body = serde_json::to_value(request)
            .map_err(|e| LlmError::Anthropic(MessageError::ApiError(e.to_string())))?;
        body["stream"] = serde_json::json!(false);

        let client = reqwest::Client::builder()
            .read_timeout(Duration::from_secs(120))
            .build()
            .map_err(|e| LlmError::Anthropic(MessageError::ApiError(e.to_string())))?;

        let response = client
            .post(&self.messages_url())
            .headers(self.headers())
            .json(&body)
            .send()
            .await
            .map_err(|e| LlmError::Anthropic(MessageError::ApiError(e.to_string())))?;

        if !response.status().is_success() {
            let status = response.status();
            let body_text = response.text().await.unwrap_or_default();
            return Err(LlmError::Anthropic(MessageError::ApiError(format!(
                "HTTP {status}: {body_text}"
            ))));
        }

        #[derive(Deserialize)]
        struct CreateMessageResponse {
            content: Vec<ContentBlock>,
            #[serde(rename = "stop_reason")]
            stop_reason: Option<String>,
        }

        let payload: CreateMessageResponse = response.json().await.map_err(|e| {
            LlmError::Anthropic(MessageError::ApiError(format!(
                "Failed to parse response: {e}"
            )))
        })?;

        Ok((payload.content, parse_stop_reason(payload.stop_reason)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_stop_reason_handles_pause_turn() {
        assert!(matches!(
            parse_stop_reason(Some("pause_turn".to_string())),
            Some(StopReason::EndTurn)
        ));
        assert!(matches!(
            parse_stop_reason(Some("end_turn".to_string())),
            Some(StopReason::EndTurn)
        ));
        assert!(matches!(
            parse_stop_reason(Some("tool_use".to_string())),
            Some(StopReason::ToolUse)
        ));
        assert!(parse_stop_reason(Some("unknown".to_string())).is_none());
        assert!(parse_stop_reason(None).is_none());
    }
}
