//! Anthropic LLM adapter.
//!
//! Uses direct HTTP + SSE instead of the SDK's streaming client so that we
//! can gracefully handle new stop_reason values (e.g. `pause_turn`) that the
//! upstream `anthropic-ai-sdk` crate has not yet added to its `StopReason`
//! enum.

use std::error::Error;
use std::time::Duration;

use anthropic_ai_sdk::types::message::{
    ContentBlock, ContentBlockDelta, CreateMessageParams, MessageError, StopReason, StreamUsage,
};
use futures_util::StreamExt;
use reqwest_eventsource::{Event, RequestBuilderExt};
use serde::Deserialize;
use tokio::sync::mpsc::UnboundedSender;

use tact_protocol::{AgentUpdate, ModelCallParams, TokenUsageInfo};

use super::{LlmClient, LlmError};

#[derive(Clone)]
pub struct AnthropicAdapter {
    api_key: String,
    base_url: String,
    api_version: String,
    /// Optional user identifier sent in the `metadata` body field.
    ///
    /// DeepSeek's Anthropic-compatible endpoint uses `metadata.user_id`
    /// for KV cache isolation: requests with the same `user_id` within a
    /// session share the KV cache, improving cache hit rate and reducing
    /// latency / cost.  Session IDs (UUIDs) are natural candidates.
    user_id: Option<String>,
}

impl AnthropicAdapter {
    pub fn new(api_key: impl Into<String>, base_url: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            base_url: base_url.into(),
            api_version: "2023-06-01".to_string(),
            user_id: None,
        }
    }

    /// Set the `user_id` that will be injected into every outgoing request
    /// body as `metadata.user_id`.
    pub fn set_user_id(&mut self, user_id: String) {
        self.user_id = Some(user_id);
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

/// Walk an HTTP error's `source()` chain to surface the real root cause.
///
/// `reqwest::Error::to_string()` often yields a generic "error sending request
/// for url (...)" that hides the underlying cause (DNS failure, TLS handshake
/// error, "connection refused", etc.).  Walking the source chain recovers the
/// originating `hyper` / `rustls` / `std::io::Error` message.
fn format_http_error(e: &(dyn Error + 'static)) -> String {
    let mut parts: Vec<String> = vec![e.to_string()];
    let mut source = e.source();
    while let Some(s) = source {
        parts.push(s.to_string());
        source = s.source();
    }
    parts.join(": ")
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
    ) -> Result<
        (
            Vec<ContentBlock>,
            Option<StopReason>,
            Option<TokenUsageInfo>,
            Option<crate::LlmRequestBody>,
        ),
        LlmError,
    > {
        let mut response_blocks: Vec<ContentBlock> = Vec::new();
        let mut tool_input_buffers: Vec<String> = Vec::new();
        let mut stop_reason: Option<StopReason> = None;
        let mut token_usage: Option<TokenUsageInfo> = None;

        let mut body = serde_json::to_value(request)
            .map_err(|e| LlmError::Anthropic(MessageError::ApiError(e.to_string())))?;
        body["stream"] = serde_json::json!(true);
        if let Some(ref uid) = self.user_id {
            if let Some(meta) = body["metadata"].as_object_mut() {
                meta.insert("user_id".to_string(), serde_json::json!(uid));
            } else {
                body["metadata"] = serde_json::json!({"user_id": uid});
            }
        }

        let client = reqwest::Client::builder()
            .read_timeout(Duration::from_secs(120))
            .build()
            .map_err(|e| LlmError::Anthropic(MessageError::ApiError(format_http_error(&e))))?;

        let json_body = serde_json::to_vec(&body).unwrap();
        let mut event_source = client
            .post(self.messages_url())
            .headers(self.headers())
            .json(&body)
            .eventsource()
            .map_err(|e| LlmError::Anthropic(MessageError::ApiError(format_http_error(&e))))?;
        event_source.set_retry_policy(Box::new(reqwest_eventsource::retry::Never));

        while let Some(event) = event_source.next().await {
            match event {
                Err(e) => {
                    return Err(LlmError::Anthropic(MessageError::ApiError(
                        format_http_error(&e),
                    )));
                }
                Ok(Event::Open) => continue,
                Ok(Event::Message(msg)) => {
                    if msg.data == "[DONE]" {
                        break;
                    }

                    let value: serde_json::Value =
                        serde_json::from_str(&msg.data).map_err(|e| {
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
                            let start: MessageStartEvent =
                                serde_json::from_value(value).map_err(|e| {
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
                                    reasoning_effort: request
                                        .thinking
                                        .as_ref()
                                        .map(|_| "high".to_string()),
                                    extra_body: request
                                        .thinking
                                        .as_ref()
                                        .map(|t| serde_json::json!({"thinking": t}).to_string()),
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
                                    if !text.is_empty()
                                        && let Some(ref tx) = ui_tx
                                    {
                                        let _ = tx.send(AgentUpdate::StreamChunk(text.clone()));
                                    }
                                }
                                ContentBlock::Thinking { thinking, .. } => {
                                    tool_input_buffers[index].clear();
                                    if !thinking.is_empty()
                                        && let Some(ref tx) = ui_tx
                                    {
                                        let _ =
                                            tx.send(AgentUpdate::ThinkingChunk(thinking.clone()));
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
                            let delta_event: ContentBlockDeltaEvent = serde_json::from_value(value)
                                .map_err(|e| {
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
                                    if let Some(ContentBlock::Thinking {
                                        thinking: existing, ..
                                    }) = response_blocks.get_mut(index)
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
                                && stop.index < tool_input_buffers.len()
                                && let Ok(value) =
                                    serde_json::from_str(&tool_input_buffers[stop.index])
                            {
                                *existing = value;
                            }
                        }
                        "message_delta" => {
                            let delta_event: MessageDeltaEvent =
                                serde_json::from_value(value.clone()).map_err(|e| {
                                    LlmError::Anthropic(MessageError::ApiError(format!(
                                        "Failed to parse message_delta: {e}"
                                    )))
                                })?;
                            stop_reason = parse_stop_reason(delta_event.delta.stop_reason);
                            if let Some(usage) = delta_event.usage {
                                // The SDK's StreamUsage only has input/output
                                // tokens.  DeepSeek's Anthropic-compatible
                                // endpoint also returns cache and reasoning
                                // tokens in the same usage object.  Parse them
                                // from the raw JSON value so they aren't lost.
                                let usage_json = &value["usage"];
                                let cache_hit = usage_json["prompt_cache_hit_tokens"]
                                    .as_u64()
                                    .map(|n| n as u32)
                                    .unwrap_or(0);
                                let cache_miss = usage_json["prompt_cache_miss_tokens"]
                                    .as_u64()
                                    .map(|n| n as u32)
                                    .unwrap_or(0);
                                let reasoning = usage_json["completion_tokens_details"]
                                    .get("reasoning_tokens")
                                    .and_then(|v| v.as_u64())
                                    .map(|n| n as u32)
                                    .unwrap_or(0);
                                token_usage = Some(TokenUsageInfo {
                                    prompt: usage.input_tokens,
                                    completion: usage.output_tokens,
                                    total: usage.input_tokens + usage.output_tokens,
                                    prompt_cache_hit_tokens: cache_hit,
                                    prompt_cache_miss_tokens: cache_miss,
                                    reasoning_tokens: reasoning,
                                });
                                if let Some(ref tx) = ui_tx {
                                    let _ = tx.send(AgentUpdate::TokenUsage {
                                        prompt: usage.input_tokens,
                                        completion: usage.output_tokens,
                                        total: usage.input_tokens + usage.output_tokens,
                                        prompt_cache_hit_tokens: cache_hit,
                                        prompt_cache_miss_tokens: cache_miss,
                                        reasoning_tokens: reasoning,
                                    });
                                }
                            }
                        }
                        "message_stop" => break,
                        "ping" => {}
                        "error" => {
                            let err: StreamErrorEvent =
                                serde_json::from_value(value).map_err(|e| {
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

        Ok((response_blocks, stop_reason, token_usage, Some(json_body)))
    }

    async fn create_message(
        &self,
        request: &CreateMessageParams,
    ) -> Result<
        (
            Vec<ContentBlock>,
            Option<StopReason>,
            Option<TokenUsageInfo>,
            Option<crate::LlmRequestBody>,
        ),
        LlmError,
    > {
        let mut body = serde_json::to_value(request)
            .map_err(|e| LlmError::Anthropic(MessageError::ApiError(e.to_string())))?;
        body["stream"] = serde_json::json!(false);
        if let Some(ref uid) = self.user_id {
            if let Some(meta) = body["metadata"].as_object_mut() {
                meta.insert("user_id".to_string(), serde_json::json!(uid));
            } else {
                body["metadata"] = serde_json::json!({"user_id": uid});
            }
        }

        let json_body = serde_json::to_vec(&body).unwrap();

        let client = reqwest::Client::builder()
            .read_timeout(Duration::from_secs(120))
            .build()
            .map_err(|e| LlmError::Anthropic(MessageError::ApiError(format_http_error(&e))))?;

        let response = client
            .post(self.messages_url())
            .headers(self.headers())
            .json(&body)
            .send()
            .await
            .map_err(|e| LlmError::Anthropic(MessageError::ApiError(format_http_error(&e))))?;

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
            usage: Option<serde_json::Value>,
        }

        let payload: CreateMessageResponse = response.json().await.map_err(|e| {
            LlmError::Anthropic(MessageError::ApiError(format!(
                "Failed to parse response: {e}"
            )))
        })?;

        let token_usage = payload.usage.as_ref().and_then(|raw| {
            let prompt = raw["input_tokens"].as_u64().map(|n| n as u32)?;
            let completion = raw["output_tokens"].as_u64().map(|n| n as u32)?;
            Some(TokenUsageInfo {
                prompt,
                completion,
                total: prompt + completion,
                prompt_cache_hit_tokens: raw["prompt_cache_hit_tokens"]
                    .as_u64()
                    .map(|n| n as u32)
                    .unwrap_or(0),
                prompt_cache_miss_tokens: raw["prompt_cache_miss_tokens"]
                    .as_u64()
                    .map(|n| n as u32)
                    .unwrap_or(0),
                reasoning_tokens: raw["completion_tokens_details"]
                    .get("reasoning_tokens")
                    .and_then(|v| v.as_u64())
                    .map(|n| n as u32)
                    .unwrap_or(0),
            })
        });

        Ok((
            payload.content,
            parse_stop_reason(payload.stop_reason),
            token_usage,
            Some(json_body),
        ))
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
