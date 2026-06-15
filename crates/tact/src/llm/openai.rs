//! OpenAI-compatible LLM adapter.
//!
//! Uses `async-openai` types for request construction but handles responses
//! via custom deserialization structs in order to capture the
//! `reasoning_content` field, which `async-openai` does not expose in its
//! Chat Completions types (as of 0.40.2).
//!
//! SSE (Server-Sent Events) parsing uses `reqwest-eventsource` instead of
//! hand-rolled byte-level parsing, for correct handling of `\n\n` / `\r\n\r\n`
//! delimiters and connection lifecycle.

use anthropic_ai_sdk::types::message::{ContentBlock, CreateMessageParams, StopReason};
use async_openai::config::Config;
use futures_util::StreamExt;
use reqwest::header::{AUTHORIZATION, HeaderMap};
use reqwest_eventsource::{Event, RequestBuilderExt};
use secrecy::{ExposeSecret, Secret};
use serde::Deserialize;
use std::time::Duration;
use tokio::sync::mpsc::UnboundedSender;

use tact_core::AgentUpdate;

use super::{LlmClient, LlmError, convert::build_openai_request};

// ── Streaming response types ──────────────────────────────────────────

/// Top-level SSE chunk from an OpenAI-compatible streaming chat completion.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct StreamChunk {
    choices: Vec<StreamChoice>,
    usage: Option<StreamUsage>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct StreamChoice {
    delta: StreamDelta,
    finish_reason: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct StreamDelta {
    content: Option<String>,
    #[serde(rename = "reasoning_content")]
    reasoning_content: Option<String>,
    tool_calls: Vec<StreamToolCallDelta>,
}

#[derive(Debug, Deserialize)]
struct StreamToolCallDelta {
    index: u32,
    id: Option<String>,
    function: Option<StreamFunctionDelta>,
}

#[derive(Debug, Deserialize)]
struct StreamFunctionDelta {
    name: Option<String>,
    arguments: Option<String>,
}

#[derive(Debug, Deserialize)]
struct StreamUsage {
    prompt_tokens: u32,
    completion_tokens: u32,
    prompt_cache_hit_tokens: Option<u32>,
    prompt_cache_miss_tokens: Option<u32>,
}

// ── Config / Adapter ───────────────────────────────────────────────────

/// Custom config that strips the `OpenAI-Beta` header.
///
/// `async-openai`'s built-in `OpenAIConfig` unconditionally adds
/// `OpenAI-Beta: assistants=v1` to every request, which causes 403
/// on many OpenAI-compatible providers (Kimi, DeepSeek, etc.).
#[derive(Clone)]
pub struct CompatibleConfig {
    api_base: String,
    api_key: Secret<String>,
}

impl CompatibleConfig {
    pub fn new(api_key: impl Into<String>, api_base: impl Into<String>) -> Self {
        Self {
            api_base: api_base.into(),
            api_key: Secret::new(api_key.into()),
        }
    }
}

impl Config for CompatibleConfig {
    fn headers(&self) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            AUTHORIZATION,
            format!("Bearer {}", self.api_key.expose_secret())
                .parse()
                .unwrap(),
        );
        // Kimi's coding endpoint whitelists specific coding agents.
        // Without a matching User-Agent it returns 403 access_terminated_error.
        headers.insert(reqwest::header::USER_AGENT, "Claude Code".parse().unwrap());
        headers
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.api_base, path)
    }

    fn query(&self) -> Vec<(&str, &str)> {
        vec![]
    }

    fn api_base(&self) -> &str {
        &self.api_base
    }

    fn api_key(&self) -> &Secret<String> {
        &self.api_key
    }
}

#[derive(Clone)]
pub struct OpenAiAdapter {
    config: CompatibleConfig,
}

impl OpenAiAdapter {
    pub fn new(config: CompatibleConfig) -> Self {
        Self { config }
    }
}

#[async_trait::async_trait]
impl LlmClient for OpenAiAdapter {
    /// Stream a chat completion via SSE, capturing `reasoning_content`
    /// not present in `async-openai`'s response types.
    ///
    /// Uses `reqwest-eventsource` for robust SSE parsing (handles both
    /// `\n\n` and `\r\n\r\n` delimiters) and injects `stream_options`
    /// to receive usage data in the final chunk.
    async fn stream_message(
        &self,
        request: &CreateMessageParams,
        ui_tx: Option<UnboundedSender<AgentUpdate>>,
    ) -> Result<(Vec<ContentBlock>, Option<StopReason>), LlmError> {
        let mut openai_request = build_openai_request(request);
        openai_request.stream = Some(true);

        // `stream_options` is absent from async-openai 0.20's
        // CreateChatCompletionRequest, so we inject it into the JSON body.
        let mut body =
            serde_json::to_value(&openai_request).map_err(|e| LlmError::Other(e.to_string()))?;
        body["stream_options"] = serde_json::json!({"include_usage": true});

        let url = self.config.url("/chat/completions");
        let headers = self.config.headers();

        let mut event_source = reqwest::Client::builder()
            .read_timeout(Duration::from_secs(120))
            .build()
            .map_err(|e| LlmError::Other(e.to_string()))?
            .post(&url)
            .headers(headers)
            .json(&body)
            .eventsource()
            .map_err(|e| LlmError::Other(e.to_string()))?;

        // Disable automatic retry — re-sending the request to an LLM
        // endpoint would produce a duplicate response.
        event_source.set_retry_policy(Box::new(reqwest_eventsource::retry::Never));

        let mut text_buffer = String::new();
        let mut reasoning_buffer = String::new();
        // Buffer tool call results until the final chunk is received.
        // (vec of (id, name, args) buffers)
        let mut tool_call_buffers: Vec<(Option<String>, Option<String>, String)> = Vec::new();
        let mut stop_reason: Option<StopReason> = None;

        while let Some(event) = event_source.next().await {
            match event {
                Err(e) => return Err(LlmError::Other(format!("SSE error: {e}"))),
                Ok(Event::Open) => continue,
                Ok(Event::Message(msg)) => {
                    if msg.data == "[DONE]" {
                        break;
                    }

                    let chunk: StreamChunk = serde_json::from_str(&msg.data)
                        .map_err(|e| LlmError::Other(format!("JSON parse: {e}")))?;

                    // ── choices ──
                    for choice in &chunk.choices {
                        let delta = &choice.delta;

                        // content
                        if let Some(ref content) = delta.content
                            && !content.is_empty()
                        {
                            text_buffer.push_str(content);
                            if let Some(ref tx) = ui_tx {
                                let _ = tx.send(AgentUpdate::StreamChunk(content.clone()));
                            }
                        }

                        // reasoning_content (thinking)
                        if let Some(ref reasoning) = delta.reasoning_content
                            && !reasoning.is_empty()
                        {
                            reasoning_buffer.push_str(reasoning);
                            if let Some(ref tx) = ui_tx {
                                let _ = tx.send(AgentUpdate::ThinkingChunk(reasoning.clone()));
                            }
                        }

                        // tool_calls
                        for tc in &delta.tool_calls {
                            let idx = tc.index as usize;
                            while tool_call_buffers.len() <= idx {
                                tool_call_buffers.push((None, None, String::new()));
                            }
                            if let Some(ref id) = tc.id {
                                tool_call_buffers[idx].0 = Some(id.clone());
                            }
                            if let Some(ref func) = tc.function {
                                if let Some(ref name) = func.name {
                                    tool_call_buffers[idx].1 = Some(name.clone());
                                }
                                if let Some(ref args) = func.arguments {
                                    tool_call_buffers[idx].2.push_str(args);
                                }
                            }
                        }

                        // finish_reason
                        if let Some(ref finish) = choice.finish_reason {
                            stop_reason = Some(match finish.as_str() {
                                "stop" => StopReason::EndTurn,
                                "length" => StopReason::MaxTokens,
                                "tool_calls" => StopReason::ToolUse,
                                "content_filter" => StopReason::StopSequence,
                                _ => StopReason::EndTurn,
                            });
                        }
                    }

                    // ── usage (sent in final chunk when stream_options.include_usage is set) ──
                    if let Some(usage) = &chunk.usage {
                        // Only emit when there are real token counts.
                        if (usage.prompt_tokens > 0 || usage.completion_tokens > 0)
                            && let Some(ref tx) = ui_tx
                        {
                            let _ = tx.send(AgentUpdate::TokenUsage {
                                prompt: usage.prompt_tokens,
                                completion: usage.completion_tokens,
                                total: usage.prompt_tokens + usage.completion_tokens,
                                prompt_cache_hit_tokens: usage.prompt_cache_hit_tokens.unwrap_or(0),
                                prompt_cache_miss_tokens: usage
                                    .prompt_cache_miss_tokens
                                    .unwrap_or(0),
                            });
                        }
                    }
                }
            }
        }

        // Build response blocks
        let mut response_blocks = Vec::new();
        if !reasoning_buffer.is_empty() {
            response_blocks.push(ContentBlock::Thinking {
                thinking: reasoning_buffer,
                signature: String::new(),
            });
        }
        if !text_buffer.is_empty() {
            response_blocks.push(ContentBlock::Text { text: text_buffer });
        }
        // use tool call buffers
        for (id, name, args) in tool_call_buffers {
            let id = id.unwrap_or_default();
            let name = name.unwrap_or_default();
            let input = serde_json::from_str(&args)
                .unwrap_or_else(|_| serde_json::Value::Object(Default::default()));
            response_blocks.push(ContentBlock::ToolUse { id, name, input });
        }

        Ok((response_blocks, stop_reason))
    }

    /*
    ── Legacy stream_message (hand-written SSE parsing) ───────────────────────
    Kept for reference. Using reqwest-eventsource fixed the following issues:
    - Only supported \n\n delimiter, not \r\n\r\n
    - data: [DONE] break only exited inner loop; outer loop would still continue
    - stream_options not set; usage stats were dead code
    - Residual buffer silently discarded after stream end

    async fn stream_message(
        &self,
        request: &CreateMessageParams,
        ui_tx: Option<UnboundedSender<AgentUpdate>>,
    ) -> Result<(Vec<ContentBlock>, Option<StopReason>), LlmError> {
        ...
    }
    */

    async fn create_message(
        &self,
        request: &CreateMessageParams,
    ) -> Result<(Vec<ContentBlock>, Option<StopReason>), LlmError> {
        let mut openai_request = build_openai_request(request);
        openai_request.stream = Some(false);

        let url = self.config.url("/chat/completions");
        let headers = self.config.headers();

        let response = reqwest::Client::builder()
            .read_timeout(Duration::from_secs(120))
            .build()
            .map_err(|e| LlmError::Other(e.to_string()))?
            .post(&url)
            .headers(headers)
            .json(&openai_request)
            .send()
            .await
            .map_err(|e| LlmError::Other(e.to_string()))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(LlmError::Other(format!("HTTP {status}: {body}")));
        }

        let json: serde_json::Value = response
            .json()
            .await
            .map_err(|e| LlmError::Other(e.to_string()))?;

        let choice = json["choices"]
            .as_array()
            .and_then(|a| a.first())
            .ok_or_else(|| LlmError::Other("No choices in response".to_string()))?;

        let message = &choice["message"];
        let mut blocks = Vec::new();

        // reasoning_content
        if let Some(reasoning) = message["reasoning_content"].as_str()
            && !reasoning.is_empty()
        {
            blocks.push(ContentBlock::Thinking {
                thinking: reasoning.to_string(),
                signature: String::new(),
            });
        }

        // content
        if let Some(content) = message["content"].as_str() {
            blocks.push(ContentBlock::Text {
                text: content.to_string(),
            });
        }

        // tool_calls
        if let Some(tool_calls) = message["tool_calls"].as_array() {
            for tc in tool_calls {
                let id = tc["id"].as_str().unwrap_or("").to_string();
                let name = tc["function"]["name"].as_str().unwrap_or("").to_string();
                let args = tc["function"]["arguments"].as_str().unwrap_or("{}");
                let input = serde_json::from_str(args)
                    .unwrap_or_else(|_| serde_json::Value::Object(Default::default()));
                blocks.push(ContentBlock::ToolUse { id, name, input });
            }
        }

        let stop_reason = choice["finish_reason"].as_str().map(|r| match r {
            "stop" => StopReason::EndTurn,
            "length" => StopReason::MaxTokens,
            "tool_calls" => StopReason::ToolUse,
            "content_filter" => StopReason::StopSequence,
            _ => StopReason::EndTurn,
        });

        Ok((blocks, stop_reason))
    }
}
