mod capabilities;
mod convert;
mod history;
mod normalize;
#[cfg(feature = "legacy-tact")]
mod request_options;
mod stream;
mod wire;

#[cfg(feature = "legacy-tact")]
pub use capabilities::{ResponsesCapabilities, ResponsesToolKind};
#[cfg(not(feature = "legacy-tact"))]
pub(crate) use capabilities::{ResponsesCapabilities, ResponsesToolKind};
#[cfg(feature = "legacy-tact")]
pub use request_options::ResponsesRequestOptions;

use std::sync::Arc;

use self::{
    normalize::{NormalizedResponse, parse_compact_resource},
    stream::ResponsesStreamState,
    wire::parse_response_envelope,
};
use crate::auth::{ApiKeyProvider, CredentialProvider, LegacyCredential};
use crate::client::{LlmClient, LlmRequestBody, LlmResponse};
use crate::error::LlmError;
use crate::provider_state::{
    ProviderConversationState, ProviderStateUpdate, ResponsesConversationState, context_hash,
};
use crate::transport::SharedHttpClient13;
use crate::{CreateMessageParams, ProviderProgress, ProviderProgressSink};
use async_openai_responses::types::responses::ResponseStreamEvent;
#[cfg(all(test, feature = "legacy-tact"))]
use async_openai_responses::{config::Config, error::OpenAIError};
use eventsource_stream::Eventsource;
use futures_util::StreamExt;
use reqwest13::header::{AUTHORIZATION, CONTENT_TYPE, HeaderMap};
use secrecy::ExposeSecret as LegacyExposeSecret;
#[cfg(all(test, feature = "legacy-tact"))]
use secrecy10::ExposeSecret;
#[cfg(all(test, feature = "legacy-tact"))]
use secrecy10::SecretString;
use serde_json::Value;

/// Custom async-openai config for the Responses protocol.
///
/// Unlike the SDK's `OpenAIConfig`, this never injects an `OpenAI-Beta`
/// header and leaves authorization to the credential provider so expiring
/// tokens can be refreshed per request.
#[derive(Clone, Debug)]
#[cfg(all(test, feature = "legacy-tact"))]
struct ResponsesCompatConfig {
    api_base: String,
    api_key: Option<SecretString>,
    empty_api_key: SecretString,
    /// Tact session id → OpenCode `x-opencode-session` header. `None` omits
    /// the session header (requests without a conversation session, e.g. the
    /// `/v1/models` picker fetch, send only the identifying User-Agent).
    opencode_session: Option<String>,
}

#[cfg(all(test, feature = "legacy-tact"))]
impl ResponsesCompatConfig {
    fn new(api_base: String, api_key: Option<SecretString>) -> Self {
        Self {
            api_base,
            api_key,
            empty_api_key: SecretString::from(String::new()),
            opencode_session: None,
        }
    }
}

#[cfg(all(test, feature = "legacy-tact"))]
impl Config for ResponsesCompatConfig {
    fn headers(&self) -> HeaderMap {
        let mut headers = HeaderMap::new();
        if let Some(api_key) = &self.api_key {
            headers.insert(
                AUTHORIZATION,
                format!("Bearer {}", api_key.expose_secret())
                    .parse()
                    .expect("bearer header value is valid"),
            );
        }
        // OpenCode Go requires x-opencode-session on every request and wants
        // a recognizable User-Agent; both are added only for that endpoint.
        // The session id doubles as the cache-distinguishing key, so each
        // Tact session maps to exactly one OpenCode session. No session id
        // (e.g. the `/v1/models` picker fetch) → session header omitted.
        headers.extend(crate::opencode::endpoint_headers(
            &self.api_base,
            self.opencode_session.as_deref(),
        ));
        headers
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.api_base.trim_end_matches('/'), path)
    }

    fn query(&self) -> Vec<(&str, &str)> {
        vec![]
    }

    fn api_base(&self) -> &str {
        &self.api_base
    }

    fn api_key(&self) -> &SecretString {
        self.api_key.as_ref().unwrap_or(&self.empty_api_key)
    }
}

fn set_default_id(value: &mut Value, default_id: String) {
    let Some(object) = value.as_object_mut() else {
        return;
    };
    if !object.get("id").is_some_and(Value::is_string) {
        object.insert("id".to_string(), Value::String(default_id));
    }
}

/// Fills the fields a compatible endpoint omits on a part or an item.
///
/// Some payloads are announced before their content exists, and some endpoints
/// never populate a field the vendored SDK's types mark required. Measured on
/// commandcode.ai (DeepSeek over `/responses`):
///
/// - `reasoning_summary_part.added` carries `"part": {"type": "summary_text"}`
///   with no `text` (the text arrives at `reasoning_summary_text.done`);
/// - the `content_part.*` events carry `{"type": "output_text", "text": …}`
///   with no `annotations`;
/// - `output_item.added` announces a `function_call` item with `call_id` and
///   `name` but **no `arguments`** — the arguments only stream in afterwards,
///   via `function_call_arguments.delta`.
///
/// Each of those fields is required by the SDK (`SummaryTextContent`,
/// `OutputTextContent`, `FunctionToolCall`), so the strict deserialize below
/// would kill the whole stream with `missing field …`. Only absent fields are
/// filled, with the type's empty value; a present value (including `""`) is
/// kept. An empty `arguments` on an announced call is exactly right — Tact
/// reads the field from `output_item.done` / the terminal response, where the
/// endpoint does populate it.
///
/// Runs over the whole event, so every carrier is covered: the `*.part.*` and
/// `content_part.*` events, a reasoning item's `summary` array, an announced
/// `function_call` item, and the terminal `response.output`.
fn fill_missing_wire_fields(value: &mut Value) {
    match value {
        Value::Object(object) => {
            match object.get("type").and_then(Value::as_str) {
                Some("summary_text") if !object.get("text").is_some_and(Value::is_string) => {
                    object.insert("text".to_string(), Value::String(String::new()));
                }
                Some("output_text") if !object.get("annotations").is_some_and(Value::is_array) => {
                    object.insert("annotations".to_string(), Value::Array(Vec::new()));
                }
                Some("function_call") if !object.get("arguments").is_some_and(Value::is_string) => {
                    object.insert("arguments".to_string(), Value::String(String::new()));
                }
                _ => {}
            }
            for nested in object.values_mut() {
                fill_missing_wire_fields(nested);
            }
        }
        Value::Array(items) => {
            for item in items {
                fill_missing_wire_fields(item);
            }
        }
        _ => {}
    }
}

/// Describes the shape of the request Tact just sent.
///
/// A compatible endpoint can answer a request it dislikes with a bare
/// `invalid_request_error` and no detail at all — measured on commandcode.ai,
/// whose whole body is `{"type":"invalid_request_error","code":"","message":
/// "invalid request error trace_id: …"}`. With nothing else to go on, the only
/// remaining evidence is the request itself, so its **shape** is attached to
/// the failure. Deliberately not the body: that carries the whole conversation.
#[cfg(all(test, feature = "legacy-tact"))]
fn describe_request_shape(body: &[u8]) -> String {
    let Ok(request) = serde_json::from_slice::<Value>(body) else {
        return format!("{} bytes, unparseable", body.len());
    };
    let mut parts = vec![format!("{} bytes", body.len())];
    if let Some(model) = request.get("model").and_then(Value::as_str) {
        parts.push(format!("model={model}"));
    }
    if let Some(input) = request.get("input").and_then(Value::as_array) {
        let mut counts: std::collections::BTreeMap<&str, usize> = std::collections::BTreeMap::new();
        for item in input {
            let kind = item.get("type").and_then(Value::as_str).unwrap_or("?");
            *counts.entry(kind).or_default() += 1;
        }
        let kinds = counts
            .iter()
            .map(|(kind, count)| format!("{kind} {count}"))
            .collect::<Vec<_>>()
            .join(", ");
        parts.push(format!("input={} items ({kinds})", input.len()));
    }
    if let Some(tools) = request.get("tools").and_then(Value::as_array) {
        parts.push(format!("tools={}", tools.len()));
    }
    if let Some(instructions) = request.get("instructions").and_then(Value::as_str) {
        parts.push(format!(
            "instructions={} chars",
            instructions.chars().count()
        ));
    }
    if let Some(max) = request.get("max_output_tokens").and_then(Value::as_u64) {
        parts.push(format!("max_output_tokens={max}"));
    }
    parts.join(", ")
}

/// Attaches [`describe_request_shape`] to an HTTP-status failure.
///
/// Only the SDK's API-status error is rewritten; a transport or parse failure
/// already says what went wrong and gains nothing from the request shape.
#[cfg(all(test, feature = "legacy-tact"))]
fn with_request_shape(error: LlmError, body: &[u8]) -> LlmError {
    let LlmError::OpenAiResponses(OpenAIError::ApiError(mut response)) = error else {
        return error;
    };
    response.api_error.message = format!(
        "{} [request: {}]",
        response.api_error.message,
        describe_request_shape(body)
    );
    LlmError::from(OpenAIError::ApiError(response))
}

fn normalize_stream_event_json(mut event: Value) -> Value {
    let event_type = event.get("type").and_then(Value::as_str).map(str::to_owned);
    fill_missing_wire_fields(&mut event);
    // Compatible endpoints may emit a `web_search_call` search action with a
    // `queries` array instead of the singular `query`; normalize the item
    // before the typed stream parser deserializes it.
    if matches!(
        event_type.as_deref(),
        Some("response.output_item.added" | "response.output_item.done")
    ) && let Some(item) = event.get_mut("item")
    {
        wire::normalize_web_search_call_query(item);
    }
    let terminal_status = match event_type.as_deref() {
        Some("response.completed") => Some("completed"),
        Some("response.incomplete") => Some("incomplete"),
        Some("response.failed") => Some("failed"),
        _ => None,
    };
    if let Some(status) = terminal_status {
        if let Some(response) = event.get_mut("response") {
            set_default_id(response, "compat-response".to_string());
            if !response.get("status").is_some_and(Value::is_string) {
                response["status"] = Value::String(status.to_string());
            }
        }
        if let Some(output) = event
            .get_mut("response")
            .and_then(|response| response.get_mut("output"))
            .and_then(Value::as_array_mut)
        {
            for (index, item) in output.iter_mut().enumerate() {
                set_default_id(item, format!("compat-output-item-{index}"));
                let is_message = item.get("type").and_then(Value::as_str) == Some("message");
                let is_function_call =
                    item.get("type").and_then(Value::as_str) == Some("function_call");
                if (is_message || is_function_call)
                    && !item.get("status").is_some_and(Value::is_string)
                {
                    item["status"] = Value::String(status.to_string());
                }
            }
        }
    }
    event
}

/// Every event `type` the vendored SDK's `ResponseStreamEvent` enum accepts.
///
/// Derived from serde itself rather than hand-maintained: for an internally
/// tagged enum, the unknown-variant error enumerates every valid tag, so this
/// set follows the enum automatically when the SDK is bumped. That matters
/// because the alternative — a hand-written list of the events Tact acts on —
/// silently drifts from the enum, and an event the SDK learned about but the
/// list did not is dropped without a trace.
///
/// This is only used to tell "a type the SDK does not model" (drop it: a
/// newer server may emit events this Tact build predates) apart from "a
/// malformed payload for a type the SDK *does* model" (a real error worth
/// surfacing). It deliberately does not decide which events Tact acts on —
/// that is `stream.rs`'s `ResponsesStreamState::apply`, the single source of
/// truth for stream semantics.
fn sdk_event_types() -> &'static std::collections::HashSet<String> {
    static TYPES: std::sync::LazyLock<std::collections::HashSet<String>> =
        std::sync::LazyLock::new(|| {
            let probe = serde_json::json!({ "type": EVENT_TYPE_PROBE });
            let error = serde_json::from_value::<ResponseStreamEvent>(probe)
                .expect_err("the probe type must not deserialize");
            let message = error.to_string();
            // "unknown variant `…`, expected one of `a`, `b`, …"
            message
                .split_once("expected one of ")
                .map(|(_, list)| {
                    list.split("`, `")
                        .map(|entry| entry.trim_matches(|c: char| c == '`' || c.is_whitespace()))
                        .filter(|entry| !entry.is_empty())
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default()
        });
    &TYPES
}

/// The bogus tag used to make serde enumerate the enum's variants. NUL is not
/// a valid event type and cannot collide with a real one.
const EVENT_TYPE_PROBE: &str = "\u{0}tact-event-type-probe";

/// Whether the SDK models this stream event type at all.
fn sdk_knows_event(event_type: &str) -> bool {
    sdk_event_types().contains(event_type)
}

struct ParsedStreamEvent {
    event: ResponseStreamEvent,
    raw_output_items: Option<Vec<Value>>,
}

fn parse_stream_event_with_raw(event: Value) -> Result<Option<ParsedStreamEvent>, LlmError> {
    let Some(event_type) = event.get("type").and_then(Value::as_str).map(str::to_owned) else {
        return Err(LlmError::StreamParse(
            "missing field `type` in OpenAI Responses stream event".to_string(),
        ));
    };
    // Forward compatibility: a newer server may emit an event type this build
    // does not model. Drop it rather than failing the whole stream — but only
    // when the SDK genuinely has no such type; a malformed payload for a type
    // the SDK *does* model is still a hard error, caught by the deserialize
    // below. Previously this was a hardcoded list of the 23 events Tact acts
    // on, which silently dropped any event the SDK learned about afterwards.
    if !sdk_knows_event(&event_type) {
        return Ok(None);
    }

    let mut event = normalize_stream_event_json(event);
    let raw_output_items = if matches!(
        event_type.as_str(),
        "response.completed" | "response.incomplete" | "response.failed"
    ) {
        let Some(response) = event.get("response").cloned() else {
            return Err(LlmError::StreamParse(
                "terminal OpenAI Responses event is missing response".to_string(),
            ));
        };
        let envelope = wire::parse_response_envelope(response)?;
        event["response"] = serde_json::to_value(envelope.typed)?;
        Some(envelope.output_items)
    } else if event_type == "response.output_item.done" {
        event.get("item").cloned().map(|item| vec![item])
    } else {
        None
    };

    serde_json::from_value(event)
        .map(|event| {
            Some(ParsedStreamEvent {
                event,
                raw_output_items,
            })
        })
        .map_err(|_| LlmError::StreamParse("deserialize OpenAI Responses stream event".to_string()))
}

#[cfg(test)]
#[cfg(all(test, feature = "legacy-tact"))]
fn parse_stream_event(event: Value) -> Result<Option<ResponseStreamEvent>, LlmError> {
    Ok(parse_stream_event_with_raw(event)?.map(|parsed| parsed.event))
}

/// Whether a `/responses` base URL is the official OpenAI API.
///
/// Official OpenAI is the one family whose turn-continuation semantics
/// expect prior `reasoning` items (with their encrypted payload) to be
/// replayed into the next request. Compatible endpoints (DeepSeek, Kimi,
/// OpenCode Go, arbitrary OpenAI-compatible proxies) regenerate reasoning
/// themselves and only waste input tokens when Tact replays it.
fn is_official_openai_base_url(base_url: &str) -> bool {
    crate::policy::is_official_openai_endpoint(base_url)
}

/// Whether a reasoning-capable model is being asked to think on this request.
///
/// DeepSeek (and similar OpenAI-compatible Reasoning models) require prior
/// `reasoning_text` to be passed back in thinking mode, even on a compatible
/// base URL that is not `api.openai.com`. The base-URL heuristic alone would
/// drop the reasoning replay for such endpoints and produce a 400
/// (`reasoning_text in the thinking mode must be passed back to the API`).
fn reasoning_replay_required(request: &CreateMessageParams) -> bool {
    let thinking = request.thinking.is_some() || request.reasoning_effort.is_some();
    crate::policy::reasoning_replay_required(&request.model, thinking)
}

/// OpenAI Responses API adapter backed by async-openai 0.41.x.
///
/// Hosted web search is a **Responses-protocol capability**, independent of
/// the endpoint/provider behind it: every ordinary `/responses` request
/// injects the hosted `Tool::WebSearch` alongside function tools, the provider
/// executes it server-side, and Tact only renders it. The
/// `/responses/compact` path never sends tools.
#[derive(Clone)]
pub struct OpenAiResponsesAdapter {
    credentials: Arc<dyn CredentialProvider>,
    http: SharedHttpClient13,
    base_url: String,
    /// Optional `context_management.compact_threshold` (tokens) sent on
    /// every ordinary `/responses` request. `None` omits `context_management`
    /// entirely; Tact never falls back to a local summary compaction for
    /// Responses providers.
    compact_threshold: Option<u32>,
    /// Tact session id, wired by [`Agent::with_session`](crate::Agent)
    /// through `LlmProvider::set_user_id`. On OpenCode Go endpoints this
    /// becomes the `x-opencode-session` header value, which the service uses
    /// to distinguish per-conversation caches: same session = same header,
    /// different sessions get different caches.
    session_id: Option<String>,
    /// Whether historical assistant `reasoning` items are replayed into the
    /// `/responses` input. Official OpenAI needs them for turn continuation;
    /// compatible endpoints (DeepSeek, OpenCode, custom OpenAI-compatible
    /// proxies) regenerate reasoning themselves, so dropping the replay cuts
    /// ~17% of input bytes. Defaults from the base URL (official OpenAI →
    /// replay; everything else → drop); override with
    /// [`Self::with_replay_prior_reasoning`].
    replay_prior_reasoning: bool,
}

impl OpenAiResponsesAdapter {
    pub fn new(
        api_key: impl Into<String>,
        base_url: impl Into<String>,
        compact_threshold: Option<u32>,
    ) -> Self {
        Self::new_with_auth(
            Arc::new(ApiKeyProvider::new(api_key)),
            base_url,
            compact_threshold,
            SharedHttpClient13::default(),
        )
    }

    /// Build the adapter with request-time credential resolution and a shared
    /// HTTP transport.
    pub fn new_with_auth(
        credentials: Arc<dyn CredentialProvider>,
        base_url: impl Into<String>,
        compact_threshold: Option<u32>,
        http: SharedHttpClient13,
    ) -> Self {
        let base_url = base_url.into().trim_end_matches('/').to_string();
        let replay_prior_reasoning = is_official_openai_base_url(&base_url);
        Self {
            credentials,
            http,
            base_url,
            compact_threshold,
            session_id: None,
            replay_prior_reasoning,
        }
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    fn config_url(&self, path: &str) -> String {
        format!("{}{}", self.base_url.trim_end_matches('/'), path)
    }

    /// Sets the Tact session id used as the OpenCode `x-opencode-session`
    /// value (cache-distinguishing session key) for this adapter's endpoint.
    pub fn set_session_id(&mut self, session_id: String) {
        self.session_id = Some(session_id);
    }

    /// Overrides whether historical assistant `reasoning` items are replayed
    /// into `/responses` input. Defaults from the base URL: official OpenAI
    /// replays; compatible endpoints drop the reasoning payload to save
    /// input tokens. Set this explicitly for an endpoint whose semantics
    /// differ from its host name.
    pub fn with_replay_prior_reasoning(mut self, replay: bool) -> Self {
        self.replay_prior_reasoning = replay;
        self
    }

    /// Effective reasoning-replay policy for this adapter (used by tests).
    #[cfg(all(test, feature = "legacy-tact"))]
    fn replay_prior_reasoning(&self) -> bool {
        self.replay_prior_reasoning
    }

    fn opencode_session(&self) -> Option<&str> {
        self.session_id.as_deref()
    }

    /// Builds the SDK client for the current request after resolving
    /// credentials, so OAuth-style flows can refresh tokens between calls.
    async fn direct_headers(&self) -> Result<HeaderMap, LlmError> {
        let credential = self.credentials.resolve_with_kind().await?;
        let mut headers = HeaderMap::new();
        headers.insert(
            CONTENT_TYPE,
            "application/json".parse().expect("static header"),
        );
        let key = match credential {
            LegacyCredential::ApiKey(secret) | LegacyCredential::Bearer(secret)
                if !secret.expose_secret().is_empty() =>
            {
                Some(LegacyExposeSecret::expose_secret(&secret).clone())
            }
            LegacyCredential::ApiKey(_) | LegacyCredential::Bearer(_) | LegacyCredential::None => {
                None
            }
        };
        if let Some(key) = key {
            headers.insert(
                AUTHORIZATION,
                format!("Bearer {key}")
                    .parse()
                    .map_err(|_| LlmError::Auth("invalid Responses bearer header value".into()))?,
            );
        }
        headers.extend(crate::opencode::endpoint_headers(
            &self.base_url,
            self.opencode_session(),
        ));
        Ok(headers)
    }

    /// Builds the ordinary `/responses` wire request for this adapter,
    /// including `context_management` when a compact threshold is
    /// configured. Shared by the streaming and non-streaming paths so the
    /// configured threshold can never be dropped by one of them. Hosted web
    /// search is always injected (`native_web_search = true`) because it is a
    /// Responses-protocol capability — this path is never `/responses/compact`.
    fn build_wire_request(
        &self,
        request: &CreateMessageParams,
        provider_state: Option<&ProviderConversationState>,
    ) -> Result<(serde_json::Value, Vec<serde_json::Value>), LlmError> {
        // A compatible base URL defaults to dropping the historical reasoning
        // replay to save input tokens — unless the request is thinking mode
        // on a reasoning model (DeepSeek-style) that requires `reasoning_text`
        // to be passed back on the next turn. Honor the explicit
        // `with_replay_prior_reasoning` override too.
        let replay_prior_reasoning =
            self.replay_prior_reasoning || reasoning_replay_required(request);
        convert::create_response_with_policy(
            request,
            provider_state,
            self.compact_threshold,
            convert::ResponsesRequestPolicy {
                native_web_search: true,
                replay_prior_reasoning,
            },
        )
    }

    /// Validates that a persisted Responses state is bound to this adapter's
    /// provider and base URL. Model mismatches are intentionally allowed for
    /// experimentation; the provider may reject incompatible opaque state at
    /// request time.
    fn validate_state_binding(&self, state: &ResponsesConversationState) -> Result<(), LlmError> {
        if state.provider != "openai_responses" {
            return Err(LlmError::Unsupported(format!(
                "provider state is bound to provider '{}', expected 'openai_responses'",
                state.provider
            )));
        }
        if state.base_url != self.base_url {
            return Err(LlmError::Unsupported(format!(
                "provider state is bound to base URL '{}', expected '{}'",
                state.base_url, self.base_url
            )));
        }
        Ok(())
    }

    fn state_update(
        &self,
        normalized: &NormalizedResponse,
        request: &CreateMessageParams,
        input_items: Vec<serde_json::Value>,
    ) -> Result<ProviderStateUpdate, LlmError> {
        normalized.provider_state_update(
            input_items,
            "openai_responses",
            &self.base_url,
            &request.model,
            &request.messages,
        )
    }

    fn into_result(
        normalized: NormalizedResponse,
        request_body: LlmRequestBody,
        state_update: ProviderStateUpdate,
    ) -> LlmResponse {
        LlmResponse {
            blocks: normalized.blocks,
            stop_reason: normalized.stop_reason,
            usage: normalized.usage,
            request_body: Some(request_body),
            state_update,
        }
    }
}

impl LlmClient for OpenAiResponsesAdapter {
    async fn stream_message(
        &self,
        request: &CreateMessageParams,
        provider_state: Option<&ProviderConversationState>,
        events: &dyn ProviderProgressSink,
    ) -> Result<LlmResponse, LlmError> {
        if let Some(ProviderConversationState::OpenAiResponses(state)) = provider_state {
            self.validate_state_binding(state)?;
        }
        let (mut wire_request, input_items) = self.build_wire_request(request, provider_state)?;
        wire_request["stream"] = serde_json::Value::Bool(true);
        let request_body = serde_json::to_vec(&wire_request)?;
        let headers = self.direct_headers().await?;
        let response = self
            .http
            .inner()
            .post(self.config_url("/responses"))
            .headers(headers)
            .body(request_body.clone())
            .send()
            .await
            .map_err(|error| LlmError::Request(format!("HTTP request failed: {error}")))?;
        if !response.status().is_success() {
            let status = response.status();
            let retry_after = crate::transport::retry_after_13(response.headers());
            let body = crate::transport::read_bounded_error_body_13(response).await;
            let class = crate::transport::classify_http_error(status.as_u16(), &body);
            return Err(LlmError::HttpError {
                status: status.as_u16(),
                class,
                retry_after,
            });
        }
        let mut response_stream = response.bytes_stream().eventsource();
        let mut state = ResponsesStreamState::default();

        while let Some(result) = response_stream.next().await {
            let event = match result {
                Ok(event) => {
                    if event.data == "[DONE]" {
                        continue;
                    }
                    let value: Value = serde_json::from_str(&event.data)
                        .map_err(|_| LlmError::StreamParse("invalid SSE event JSON".to_string()))?;
                    match parse_stream_event_with_raw(value)? {
                        Some(parsed) => (parsed.event, parsed.raw_output_items),
                        None => continue,
                    }
                }
                Err(_) => {
                    if let Some(update) = state.close_thinking() {
                        events.send(update);
                    }
                    return Err(LlmError::StreamParse("SSE stream error".to_string()));
                }
            };
            let updates = match state.apply_with_raw(event.0, event.1) {
                Ok(updates) => updates,
                Err(error) => {
                    if let Some(update) = state.close_thinking() {
                        events.send(update);
                    }
                    return Err(error);
                }
            };
            for update in updates {
                events.send(update);
            }
        }

        if let Some(update) = state.close_thinking() {
            events.send(update);
        }
        let normalized = state.finish()?;
        if let Some(usage) = &normalized.usage {
            events.send(ProviderProgress::Usage(usage.clone()));
        }
        let state_update = self.state_update(&normalized, request, input_items)?;
        Ok(Self::into_result(normalized, request_body, state_update))
    }

    async fn create_message(
        &self,
        request: &CreateMessageParams,
        provider_state: Option<&ProviderConversationState>,
    ) -> Result<LlmResponse, LlmError> {
        if let Some(ProviderConversationState::OpenAiResponses(state)) = provider_state {
            self.validate_state_binding(state)?;
        }
        let (mut wire_request, input_items) = self.build_wire_request(request, provider_state)?;
        wire_request["stream"] = serde_json::Value::Bool(false);
        let request_body = serde_json::to_vec(&wire_request)?;
        let headers = self.direct_headers().await?;
        let response = self
            .http
            .inner()
            .post(self.config_url("/responses"))
            .headers(headers)
            .body(request_body.clone())
            .send()
            .await
            .map_err(|error| LlmError::Request(format!("HTTP request failed: {error}")))?;
        if !response.status().is_success() {
            let status = response.status();
            let retry_after = crate::transport::retry_after_13(response.headers());
            let body = crate::transport::read_bounded_error_body_13(response).await;
            let class = crate::transport::classify_http_error(status.as_u16(), &body);
            return Err(LlmError::HttpError {
                status: status.as_u16(),
                class,
                retry_after,
            });
        }
        let bytes = response
            .bytes()
            .await
            .map_err(|error| LlmError::Request(format!("response read failed: {error}")))?;
        let value: Value = serde_json::from_slice(&bytes)?;
        let envelope = parse_response_envelope(value)?;
        let wire::RawResponseEnvelope {
            value,
            typed,
            output_items,
            unknown_output_items,
        } = envelope;
        drop((value, unknown_output_items));
        let mut normalized = normalize::normalize_response(typed)?;
        normalized.output_items = output_items;
        let state_update = self.state_update(&normalized, request, input_items)?;
        Ok(Self::into_result(normalized, request_body, state_update))
    }

    async fn compact(
        &self,
        request: &CreateMessageParams,
        provider_state: Option<&ProviderConversationState>,
    ) -> Result<LlmResponse, LlmError> {
        if let Some(ProviderConversationState::OpenAiResponses(state)) = provider_state {
            self.validate_state_binding(state)?;
        }
        // The compact request carries the current protocol baseline plus any
        // logical messages not yet represented in it. The exact JSON input
        // items (including unknown/future item types) are preserved by
        // sending the request through the byot JSON path; no local summary
        // prompt or `create_message()` call is used. The reasoning-replay
        // policy applies here too: compatible endpoints compact without
        // historical reasoning payloads — unless the request is thinking
        // mode on a reasoning model that must receive `reasoning_text` back.
        let replay_prior_reasoning =
            self.replay_prior_reasoning || reasoning_replay_required(request);
        let (body, _) = convert::create_response_with_policy(
            request,
            provider_state,
            None,
            convert::ResponsesRequestPolicy {
                native_web_search: false,
                replay_prior_reasoning,
            },
        )?;
        let compact_request = serde_json::json!({
            "model": request.model,
            "input": body["input"],
        });
        let request_body = serde_json::to_vec(&compact_request)?;
        let credential = self.credentials.resolve_with_kind().await?;
        let key = match credential {
            LegacyCredential::ApiKey(secret) | LegacyCredential::Bearer(secret)
                if !secret.expose_secret().is_empty() =>
            {
                Some(LegacyExposeSecret::expose_secret(&secret).clone())
            }
            LegacyCredential::ApiKey(_) | LegacyCredential::Bearer(_) | LegacyCredential::None => {
                None
            }
        };
        let url = format!("{}/responses/compact", self.base_url.trim_end_matches('/'));
        let mut http_request = self
            .http
            .inner()
            .post(&url)
            .headers(crate::opencode::endpoint_headers(
                &self.base_url,
                self.opencode_session(),
            ))
            .json(&compact_request);
        if let Some(key) = key {
            http_request = http_request.header(AUTHORIZATION, format!("Bearer {key}"));
        }
        let response = http_request
            .send()
            .await
            .map_err(|error| LlmError::Unsupported(format!("HTTP request failed: {error}")))?;
        let status = response.status();
        // Compatible endpoints (e.g. custom OpenAI-compatible proxies) often
        // do not implement POST /responses/compact at all and answer 404
        // (sometimes 405) with an HTML page. Report that clearly instead of
        // surfacing the SDK's JSON-deserialization error over the HTML body.
        if status == reqwest13::StatusCode::NOT_FOUND
            || status == reqwest13::StatusCode::METHOD_NOT_ALLOWED
        {
            return Err(LlmError::Unsupported(format!(
                "endpoint does not support POST /responses/compact (HTTP {status}): \
                 native Responses compaction is not implemented by base URL {}",
                self.base_url
            )));
        }
        if !status.is_success() {
            let retry_after = crate::transport::retry_after_13(response.headers());
            let body = crate::transport::read_bounded_error_body_13(response).await;
            let class = crate::transport::classify_http_error(status.as_u16(), &body);
            return Err(LlmError::HttpError {
                status: status.as_u16(),
                class,
                retry_after,
            });
        }
        let body_bytes = response
            .bytes()
            .await
            .map_err(|error| LlmError::Unsupported(format!("HTTP request failed: {error}")))?;
        let resource: Value = serde_json::from_slice(&body_bytes).map_err(LlmError::from)?;
        let parsed = parse_compact_resource(resource)?;
        let state = ResponsesConversationState {
            version: 1,
            provider: "openai_responses".to_string(),
            base_url: self.base_url.clone(),
            model: request.model.clone(),
            input_items: parsed.input_items,
            compaction_id: Some(parsed.compaction_id),
            is_compacted: true,
            logical_message_count: request.messages.len(),
            logical_context_hash: context_hash(&request.messages)?,
        };
        Ok(LlmResponse {
            blocks: Vec::new(),
            stop_reason: None,
            // Preserve the token accounting reported by the compaction pass
            // so the native compact call is observable like any other LLM
            // call (it does not drive `last_token_total`: the next request
            // input is the compacted baseline, not the pre-compact prompt).
            usage: parsed.usage,
            request_body: Some(request_body),
            state_update: ProviderStateUpdate::Replace(ProviderConversationState::OpenAiResponses(
                state,
            )),
        })
    }
}

#[cfg(all(test, feature = "legacy-tact"))]
mod tests;
