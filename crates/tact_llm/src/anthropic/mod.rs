//! Anthropic LLM adapter.
//!
//! Uses direct HTTP + SSE instead of the SDK's streaming client so that we
//! can map new Anthropic `stop_reason` strings into [`crate::StopReason`]
//! without waiting on upstream SDK enum updates.

use std::{error::Error, time::Duration};

use futures_util::StreamExt;
use reqwest_eventsource::{Event, RequestBuilderExt};
use serde::Deserialize;
use tact_protocol::{AgentUpdate, ModelCallParams, ThinkingChunk, TokenUsageInfo};
use tokio::sync::mpsc::UnboundedSender;

use super::{
    ContentBlock, ContentBlockDelta, CreateMessageParams, LlmClient, LlmError, LlmResponse,
    MessageError, ProviderConversationState, ProviderStateUpdate, StopReason, StreamUsage,
};

/// Events emitted when an Anthropic thinking content block starts.
fn thinking_start_events(initial_thinking: &str) -> Vec<ThinkingChunk> {
    let mut events = vec![ThinkingChunk::Started];
    if !initial_thinking.is_empty() {
        events.push(ThinkingChunk::Delta(initial_thinking.to_string()));
    }
    events
}

fn is_thinking_content_block(block: Option<&ContentBlock>) -> bool {
    matches!(block, Some(ContentBlock::Thinking { .. }))
}

#[derive(Clone)]
pub struct AnthropicAdapter {
    api_key: String,
    base_url: String,
    api_version: String,
    client: reqwest::Client,
}

impl AnthropicAdapter {
    pub fn new(api_key: impl Into<String>, base_url: impl Into<String>) -> Self {
        let client = reqwest::Client::builder()
            .read_timeout(Duration::from_secs(120))
            .build()
            .expect("failed to build reqwest client");
        Self {
            api_key: api_key.into(),
            base_url: base_url.into(),
            api_version: "2023-06-01".to_string(),
            client,
        }
    }

    /// Serialize the request and set the `stream` flag.
    fn prepare_body(
        &self,
        request: &CreateMessageParams,
        stream: bool,
    ) -> Result<serde_json::Value, LlmError> {
        let mut body = serde_json::to_value(request)
            .map_err(|e| LlmError::Anthropic(MessageError::ApiError(e.to_string())))?;
        body["stream"] = serde_json::json!(stream);
        Ok(body)
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
    StopReason::from_anthropic(reason.as_deref())
}

// ── Token usage extraction ───────────────────────────────────────────────
//
// Anthropic's `input_tokens`/`output_tokens` are `u32` on the wire, but the
// DeepSeek-compatible extras (`prompt_cache_hit_tokens`,
// `completion_tokens_details.reasoning_tokens`) live in the untyped part of
// the usage object. All of these are clamped to `u32::MAX` rather than
// truncated with `as u32`: a bogus counter must not wrap into a
// plausible-looking small number.

/// Clamps a wire token count into `u32`, saturating instead of wrapping.
fn clamp_token_count(value: u64) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}

/// Reads one optional numeric field of a raw usage object, defaulting to 0.
fn usage_field(raw: &serde_json::Value, field: &str) -> u32 {
    raw.get(field)
        .and_then(|v| v.as_u64())
        .map(clamp_token_count)
        .unwrap_or(0)
}

/// Reads `completion_tokens_details.reasoning_tokens`, defaulting to 0.
fn usage_reasoning_tokens(raw: &serde_json::Value) -> u32 {
    raw.get("completion_tokens_details")
        .and_then(|details| details.get("reasoning_tokens"))
        .and_then(|v| v.as_u64())
        .map(clamp_token_count)
        .unwrap_or(0)
}

/// Builds [`TokenUsageInfo`] from a raw usage object.
///
/// `None` when either required counter is missing or not a number — some
/// Anthropic-compatible endpoints omit `usage` fields entirely.
fn usage_from_json(raw: &serde_json::Value) -> Option<TokenUsageInfo> {
    let prompt = clamp_token_count(raw.get("input_tokens")?.as_u64()?);
    let completion = clamp_token_count(raw.get("output_tokens")?.as_u64()?);
    Some(TokenUsageInfo {
        prompt,
        completion,
        // Saturating: `prompt + completion` can exceed `u32::MAX` for a
        // hostile or buggy provider, which would panic in debug builds.
        total: prompt.saturating_add(completion),
        prompt_cache_hit_tokens: usage_field(raw, "prompt_cache_hit_tokens"),
        prompt_cache_miss_tokens: usage_field(raw, "prompt_cache_miss_tokens"),
        reasoning_tokens: usage_reasoning_tokens(raw),
    })
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

impl LlmClient for AnthropicAdapter {
    async fn stream_message(
        &self,
        request: &CreateMessageParams,
        _provider_state: Option<&ProviderConversationState>,
        ui_tx: Option<UnboundedSender<AgentUpdate>>,
    ) -> Result<LlmResponse, LlmError> {
        let mut response_blocks: Vec<ContentBlock> = Vec::new();
        let mut tool_input_buffers: Vec<String> = Vec::new();
        let mut stop_reason: Option<StopReason> = None;
        let mut token_usage: Option<TokenUsageInfo> = None;

        let body = self.prepare_body(request, true)?;

        let json_body = serde_json::to_vec(&body).unwrap();
        let mut event_source = self
            .client
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
                                        .map(|t| clamp_token_count(t.budget_tokens as u64)),
                                    reasoning_effort: request
                                        .reasoning_effort
                                        .map(|effort| effort.as_str().to_string()),
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
                                    if let Some(ref tx) = ui_tx {
                                        for chunk in thinking_start_events(thinking) {
                                            let _ = tx.send(AgentUpdate::ThinkingChunk(chunk));
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
                                        let _ = tx.send(AgentUpdate::ThinkingChunk(
                                            ThinkingChunk::Delta(thinking),
                                        ));
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
                            if is_thinking_content_block(response_blocks.get(stop.index))
                                && let Some(ref tx) = ui_tx
                            {
                                let _ =
                                    tx.send(AgentUpdate::ThinkingChunk(ThinkingChunk::Finished));
                            }
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
                                // The typed `StreamUsage` carries the required
                                // counters; the DeepSeek-compatible extras come
                                // from the same object's raw JSON.
                                let usage_json = &value["usage"];
                                let info = TokenUsageInfo {
                                    prompt: usage.input_tokens,
                                    completion: usage.output_tokens,
                                    total: usage.input_tokens.saturating_add(usage.output_tokens),
                                    prompt_cache_hit_tokens: usage_field(
                                        usage_json,
                                        "prompt_cache_hit_tokens",
                                    ),
                                    prompt_cache_miss_tokens: usage_field(
                                        usage_json,
                                        "prompt_cache_miss_tokens",
                                    ),
                                    reasoning_tokens: usage_reasoning_tokens(usage_json),
                                };
                                if let Some(ref tx) = ui_tx {
                                    let _ = tx.send(AgentUpdate::TokenUsage(info.clone()));
                                }
                                token_usage = Some(info);
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

        Ok(LlmResponse {
            blocks: response_blocks,
            stop_reason,
            usage: token_usage,
            request_body: Some(json_body),
            state_update: ProviderStateUpdate::Unchanged,
        })
    }

    async fn create_message(
        &self,
        request: &CreateMessageParams,
        _provider_state: Option<&ProviderConversationState>,
    ) -> Result<LlmResponse, LlmError> {
        let body = self.prepare_body(request, false)?;

        let json_body = serde_json::to_vec(&body).unwrap();

        let response = self
            .client
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

        let token_usage = payload.usage.as_ref().and_then(usage_from_json);

        Ok(LlmResponse {
            blocks: payload.content,
            stop_reason: parse_stop_reason(payload.stop_reason),
            usage: token_usage,
            request_body: Some(json_body),
            state_update: ProviderStateUpdate::Unchanged,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usage_from_json_clamps_instead_of_truncating() {
        let usage = usage_from_json(&serde_json::json!({
            "input_tokens": u32::MAX as u64 + 5,
            "output_tokens": 3,
            "prompt_cache_hit_tokens": 7,
            "prompt_cache_miss_tokens": 9,
            "completion_tokens_details": { "reasoning_tokens": 2 },
        }))
        .expect("usage with both required counters parses");
        assert_eq!(usage.prompt, u32::MAX, "must clamp, not wrap to 4");
        assert_eq!(usage.completion, 3);
        assert_eq!(usage.total, u32::MAX, "total must saturate, not overflow");
        assert_eq!(usage.prompt_cache_hit_tokens, 7);
        assert_eq!(usage.prompt_cache_miss_tokens, 9);
        assert_eq!(usage.reasoning_tokens, 2);
    }

    #[test]
    fn usage_from_json_requires_both_required_counters() {
        assert!(usage_from_json(&serde_json::json!({"input_tokens": 1})).is_none());
        assert!(usage_from_json(&serde_json::json!({"output_tokens": 1})).is_none());
        assert!(usage_from_json(&serde_json::json!({"input_tokens": "x"})).is_none());
    }

    #[test]
    fn usage_from_json_defaults_missing_extras_to_zero() {
        let usage = usage_from_json(&serde_json::json!({
            "input_tokens": 10,
            "output_tokens": 20,
        }))
        .expect("required counters present");
        assert_eq!(usage.total, 30);
        assert_eq!(usage.prompt_cache_hit_tokens, 0);
        assert_eq!(usage.prompt_cache_miss_tokens, 0);
        assert_eq!(usage.reasoning_tokens, 0);
    }

    #[test]
    fn parse_stop_reason_handles_known_values() {
        assert_eq!(
            parse_stop_reason(Some("pause_turn".to_string())),
            Some(StopReason::PauseTurn)
        );
        assert_eq!(
            parse_stop_reason(Some("end_turn".to_string())),
            Some(StopReason::EndTurn)
        );
        assert_eq!(
            parse_stop_reason(Some("tool_use".to_string())),
            Some(StopReason::ToolUse)
        );
        assert_eq!(
            parse_stop_reason(Some("refusal".to_string())),
            Some(StopReason::Refusal)
        );
        assert_eq!(
            parse_stop_reason(Some("model_context_window_exceeded".to_string())),
            Some(StopReason::MaxTokens)
        );
        assert_eq!(
            parse_stop_reason(Some("brand_new".to_string())),
            Some(StopReason::Unknown("brand_new".into()))
        );
        assert_eq!(parse_stop_reason(None), None);
    }

    #[test]
    fn thinking_start_events_empty_initial() {
        assert!(matches!(
            thinking_start_events("").as_slice(),
            [ThinkingChunk::Started]
        ));
    }

    #[test]
    fn thinking_start_events_with_initial_text() {
        let events = thinking_start_events("seed");
        assert!(matches!(
            events.as_slice(),
            [ThinkingChunk::Started, ThinkingChunk::Delta(t)] if t == "seed"
        ));
    }

    #[test]
    fn is_thinking_content_block_detects_thinking() {
        assert!(is_thinking_content_block(Some(&ContentBlock::Thinking {
            thinking: String::new(),
            signature: String::new(),
        })));
        assert!(!is_thinking_content_block(Some(&ContentBlock::Text {
            text: "hi".into()
        })));
        assert!(!is_thinking_content_block(None));
    }
}
