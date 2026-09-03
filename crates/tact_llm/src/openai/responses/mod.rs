mod capabilities;
mod convert;
mod history;
mod normalize;
mod request_options;
mod stream;
mod wire;

pub use capabilities::{ResponsesCapabilities, ResponsesToolKind};
pub use request_options::ResponsesRequestOptions;

use std::sync::Arc;

use async_openai_responses::{Client, config::Config, types::responses::ResponseStreamEvent};
use futures_util::StreamExt;
use reqwest13::header::{AUTHORIZATION, HeaderMap};
use secrecy::ExposeSecret as LegacyExposeSecret;
use secrecy10::{ExposeSecret, SecretString};
use serde_json::Value;
use tact_protocol::AgentUpdate;
use tokio::sync::mpsc::UnboundedSender;

use self::{
    convert::create_response,
    normalize::{NormalizedResponse, parse_compact_resource},
    stream::ResponsesStreamState,
    wire::parse_response_envelope,
};
use crate::{
    ApiKeyProvider, CreateMessageParams, CredentialProvider, LlmClient, LlmError, LlmRequestBody,
    LlmResponse, ProviderConversationState, ProviderStateUpdate, ResponsesConversationState,
    SharedHttpClient, context_hash,
};

/// Custom async-openai config for the Responses protocol.
///
/// Unlike the SDK's `OpenAIConfig`, this never injects an `OpenAI-Beta`
/// header and leaves authorization to the credential provider so expiring
/// tokens can be refreshed per request.
#[derive(Clone, Debug)]
struct ResponsesCompatConfig {
    api_base: String,
    api_key: Option<SecretString>,
    empty_api_key: SecretString,
    /// Tact session id → OpenCode `x-opencode-session` header. `None` falls
    /// back to a per-`base_url` token for non-conversation requests.
    opencode_session: Option<String>,
}

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
        // Tact session maps to exactly one OpenCode session.
        headers.extend(crate::opencode::endpoint_headers(
            &self.api_base,
            self.opencode_session.as_deref(),
        ));
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

fn normalize_stream_event_json(mut event: Value) -> Value {
    let event_type = event.get("type").and_then(Value::as_str).map(str::to_owned);
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
                if !is_message {
                    continue;
                }
                let Some(content) = item.get_mut("content").and_then(Value::as_array_mut) else {
                    continue;
                };
                for part in content {
                    if part.get("type").and_then(Value::as_str) == Some("output_text")
                        && part.get("annotations").is_none()
                    {
                        part["annotations"] = Value::Array(Vec::new());
                    }
                }
            }
        }
    }
    event
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
    let consumed = matches!(
        event_type.as_str(),
        "error"
            | "response.created"
            | "response.queued"
            | "response.in_progress"
            | "response.content_part.added"
            | "response.content_part.done"
            | "response.output_text.done"
            | "response.refusal.done"
            | "response.reasoning_summary_part.added"
            | "response.reasoning_summary_part.done"
            | "response.reasoning_summary_text.done"
            | "response.reasoning_text.done"
            | "response.function_call_arguments.delta"
            | "response.function_call_arguments.done"
            | "response.reasoning_summary_text.delta"
            | "response.reasoning_text.delta"
            | "response.output_text.delta"
            | "response.refusal.delta"
            | "response.output_item.added"
            | "response.output_item.done"
            | "response.completed"
            | "response.incomplete"
            | "response.failed"
    );
    if !consumed {
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
        .map_err(|error| {
            LlmError::StreamParse(format!(
                "deserialize OpenAI Responses stream event: {error}"
            ))
        })
}

#[cfg(test)]
fn parse_stream_event(event: Value) -> Result<Option<ResponseStreamEvent>, LlmError> {
    Ok(parse_stream_event_with_raw(event)?.map(|parsed| parsed.event))
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
    http: SharedHttpClient,
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
            SharedHttpClient::default(),
        )
    }

    /// Build the adapter with request-time credential resolution and a shared
    /// HTTP transport.
    pub fn new_with_auth(
        credentials: Arc<dyn CredentialProvider>,
        base_url: impl Into<String>,
        compact_threshold: Option<u32>,
        http: SharedHttpClient,
    ) -> Self {
        Self {
            credentials,
            http,
            base_url: base_url.into().trim_end_matches('/').to_string(),
            compact_threshold,
            session_id: None,
        }
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Sets the Tact session id used as the OpenCode `x-opencode-session`
    /// value (cache-distinguishing session key) for this adapter's endpoint.
    pub fn set_session_id(&mut self, session_id: String) {
        self.session_id = Some(session_id);
    }

    fn opencode_session(&self) -> Option<&str> {
        self.session_id.as_deref()
    }

    /// Builds the SDK client for the current request after resolving
    /// credentials, so OAuth-style flows can refresh tokens between calls.
    async fn sdk_client(&self) -> Result<Client<ResponsesCompatConfig>, LlmError> {
        let secret = self.credentials.resolve().await?;
        let key = SecretString::from(LegacyExposeSecret::expose_secret(&secret).clone());
        let mut config = ResponsesCompatConfig::new(self.base_url.clone(), Some(key));
        config.opencode_session = self.opencode_session().map(str::to_owned);
        Ok(Client::build(self.http.inner().clone(), config))
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
        create_response(request, provider_state, self.compact_threshold, true)
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
        ui_tx: Option<UnboundedSender<AgentUpdate>>,
    ) -> Result<LlmResponse, LlmError> {
        if let Some(ProviderConversationState::OpenAiResponses(state)) = provider_state {
            self.validate_state_binding(state)?;
        }
        let (mut wire_request, input_items) = self.build_wire_request(request, provider_state)?;
        wire_request["stream"] = serde_json::Value::Bool(true);
        let request_body = serde_json::to_vec(&wire_request)?;
        let client = self.sdk_client().await?;
        let mut response_stream = client
            .responses()
            .create_stream_byot::<_, Value>(wire_request)
            .await
            .map_err(LlmError::from)?;
        let mut state = ResponsesStreamState::default();

        while let Some(result) = response_stream.next().await {
            let event = match result {
                Ok(event) => match parse_stream_event_with_raw(event)? {
                    Some(parsed) => (parsed.event, parsed.raw_output_items),
                    None => continue,
                },
                Err(error) => {
                    if let Some(update) = state.close_thinking()
                        && let Some(tx) = &ui_tx
                    {
                        let _ = tx.send(update);
                    }
                    return Err(LlmError::from(error));
                }
            };
            let updates = match state.apply_with_raw(event.0, event.1) {
                Ok(updates) => updates,
                Err(error) => {
                    if let Some(update) = state.close_thinking()
                        && let Some(tx) = &ui_tx
                    {
                        let _ = tx.send(update);
                    }
                    return Err(error);
                }
            };
            for update in updates {
                if let Some(tx) = &ui_tx {
                    let _ = tx.send(update);
                }
            }
        }

        if let Some(update) = state.close_thinking()
            && let Some(tx) = &ui_tx
        {
            let _ = tx.send(update);
        }
        let normalized = state.finish()?;
        if let Some(usage) = &normalized.usage
            && let Some(tx) = &ui_tx
        {
            let _ = tx.send(AgentUpdate::TokenUsage(usage.clone()));
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
        let client = self.sdk_client().await?;
        let response = client
            .responses()
            .create_byot::<_, Value>(wire_request)
            .await
            .map_err(LlmError::from)?;
        let envelope = parse_response_envelope(response)?;
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
        // prompt or `create_message()` call is used.
        let (body, _) = create_response(request, provider_state, None, false)?;
        let compact_request = serde_json::json!({
            "model": request.model,
            "input": body["input"],
        });
        let request_body = serde_json::to_vec(&compact_request)?;
        let secret = self.credentials.resolve().await?;
        let key = LegacyExposeSecret::expose_secret(&secret).clone();
        let url = format!("{}/responses/compact", self.base_url);
        let response = self
            .http
            .inner()
            .post(&url)
            .header(AUTHORIZATION, format!("Bearer {key}"))
            .headers(crate::opencode::endpoint_headers(
                &self.base_url,
                self.opencode_session(),
            ))
            .json(&compact_request)
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
        let body_bytes = response
            .bytes()
            .await
            .map_err(|error| LlmError::Unsupported(format!("HTTP request failed: {error}")))?;
        if !status.is_success() {
            return Err(LlmError::HttpError {
                status: status.as_u16(),
                body: String::from_utf8_lossy(&body_bytes).into_owned(),
            });
        }
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

#[cfg(test)]
mod tests {
    use super::stream::ResponsesStreamState;
    use super::{normalize_stream_event_json, parse_stream_event, parse_stream_event_with_raw};
    use crate::{
        ContentBlock, CreateMessageParams, LlmClient, Message, RequiredMessageParams, Role,
        StopReason, Tool,
    };
    use async_openai_responses::types::responses::OutputItem;

    #[test]
    fn opencode_config_headers_carry_the_session_id() {
        use async_openai_responses::config::Config;

        let mut config =
            super::ResponsesCompatConfig::new("https://opencode.ai/zen/go/v1".into(), None);
        // Simulate Agent::with_session → LlmProvider::set_user_id wiring.
        config.opencode_session = Some("tact-session-1".into());
        let headers = config.headers();
        assert_eq!(
            headers
                .get(crate::opencode::X_OPENCODE_SESSION)
                .and_then(|v| v.to_str().ok()),
            Some("tact-session-1"),
            "x-opencode-session must equal the session id (cache-distinguishing key)"
        );
        let ua = headers
            .get(reqwest13::header::USER_AGENT)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default();
        assert!(ua.starts_with("tact/"), "user agent identifies tact: {ua}");
    }

    #[test]
    fn opencode_config_without_session_uses_fallback_token() {
        use async_openai_responses::config::Config;

        let config =
            super::ResponsesCompatConfig::new("https://opencode.ai/zen/go/v1".into(), None);
        let headers = config.headers();
        let session = headers
            .get(crate::opencode::X_OPENCODE_SESSION)
            .and_then(|v| v.to_str().ok())
            .expect("x-opencode-session must always be present on opencode endpoints");
        assert!(!session.is_empty());
    }

    #[test]
    fn set_session_id_is_used_by_compact_and_sdk_headers() {
        // The adapter stores the session id and hands it to both the SDK
        // config (ordinary /responses) and the direct compact POST.
        let mut adapter =
            super::OpenAiResponsesAdapter::new("test-key", "https://opencode.ai/zen/go/v1", None);
        assert!(adapter.opencode_session().is_none());
        adapter.set_session_id("tact-session-2".into());
        assert_eq!(adapter.opencode_session(), Some("tact-session-2"));
    }

    #[test]
    fn fills_missing_output_text_annotations_for_terminal_events() {
        let event = normalize_stream_event_json(serde_json::json!({
            "type": "response.completed",
            "response": {
                "output": [{
                    "type": "message",
                    "content": [{"type": "output_text", "text": "answer"}]
                }]
            }
        }));
        assert_eq!(
            event["response"]["output"][0]["content"][0]["annotations"],
            serde_json::json!([])
        );
    }

    #[test]
    fn parses_content_part_events_without_deserializing_provider_specific_items() {
        let event = parse_stream_event(serde_json::json!({
            "type": "response.content_part.added",
            "sequence_number": 1,
            "item_id": "msg_1",
            "output_index": 0,
            "content_index": 0,
            "part": {"type": "output_text", "text": "answer", "annotations": []}
        }))
        .unwrap();

        assert!(event.is_some());
    }

    #[test]
    fn parses_terminal_unknown_item_with_raw_output_metadata() {
        let mut response = super::normalize::tests::completed_response_json();
        response["output"].as_array_mut().unwrap().insert(
            0,
            serde_json::json!({
                "type": "future_item",
                "id": "future-1",
                "payload": {"x": 1}
            }),
        );
        let parsed = parse_stream_event_with_raw(serde_json::json!({
            "type": "response.completed",
            "sequence_number": 1,
            "response": response
        }))
        .unwrap()
        .unwrap();
        assert!(parsed.raw_output_items.is_some());
        assert_eq!(parsed.raw_output_items.unwrap()[0]["type"], "future_item");
    }

    #[test]
    fn stream_finish_retains_unknown_terminal_items_for_state() {
        let mut response = super::normalize::tests::completed_response_json();
        response["output"].as_array_mut().unwrap().insert(
            0,
            serde_json::json!({
                "type": "future_item",
                "id": "future-1",
                "payload": {"x": 1}
            }),
        );
        let parsed = parse_stream_event_with_raw(serde_json::json!({
            "type": "response.completed",
            "sequence_number": 1,
            "response": response
        }))
        .unwrap()
        .unwrap();
        let mut state = ResponsesStreamState::default();
        state
            .apply_with_raw(parsed.event, parsed.raw_output_items)
            .unwrap();
        let normalized = state.finish().unwrap();
        assert_eq!(normalized.output_items[0]["type"], "future_item");
    }

    #[test]
    fn parses_output_item_added_and_done_events() {
        for event_type in ["response.output_item.added", "response.output_item.done"] {
            let event = parse_stream_event(serde_json::json!({
                "type": event_type,
                "sequence_number": 1,
                "output_index": 0,
                "item": {
                    "type": "message",
                    "id": "msg_1",
                    "role": "assistant",
                    "status": "completed",
                    "content": [{
                        "type": "output_text",
                        "text": "answer",
                        "annotations": []
                    }]
                }
            }))
            .unwrap();

            assert!(event.is_some());
        }
    }

    #[test]
    fn parses_web_search_call_item_with_queries_array() {
        // Compatible endpoints emit the search action with a `queries` array
        // instead of the singular `query`; the stream parser must normalize
        // the item before typed deserialization.
        for event_type in ["response.output_item.added", "response.output_item.done"] {
            let event = parse_stream_event(serde_json::json!({
                "type": event_type,
                "sequence_number": 1,
                "output_index": 0,
                "item": {
                    "type": "web_search_call",
                    "id": "ws-1",
                    "status": "completed",
                    "action": {"type": "search", "queries": ["Tokyo weather", "ws_call_id=ws-1"]}
                }
            }))
            .unwrap()
            .expect("event must parse");

            match event {
                async_openai_responses::types::responses::ResponseStreamEvent::ResponseOutputItemAdded(
                    ev,
                ) => {
                    let OutputItem::WebSearchCall(call) = &ev.item else {
                        panic!("expected WebSearchCall item");
                    };
                    assert_eq!(
                        call.action
                            .as_ref()
                            .and_then(|action| match action {
                                async_openai_responses::types::responses::WebSearchToolCallAction::Search(
                                    search,
                                ) => Some(search.query.as_str()),
                                _ => None,
                            }),
                        Some("Tokyo weather")
                    );
                }
                async_openai_responses::types::responses::ResponseStreamEvent::ResponseOutputItemDone(
                    ev,
                ) => {
                    let OutputItem::WebSearchCall(call) = &ev.item else {
                        panic!("expected WebSearchCall item");
                    };
                    assert_eq!(
                        call.action
                            .as_ref()
                            .and_then(|action| match action {
                                async_openai_responses::types::responses::WebSearchToolCallAction::Search(
                                    search,
                                ) => Some(search.query.as_str()),
                                _ => None,
                            }),
                        Some("Tokyo weather")
                    );
                }
                other => panic!("expected output item event, got {other:?}"),
            }
        }
    }

    #[test]
    fn parses_output_item_done_with_a_compaction_item() {
        let event = parse_stream_event(serde_json::json!({
            "type": "response.output_item.done",
            "sequence_number": 1,
            "output_index": 2,
            "item": {
                "type": "compaction",
                "id": "cmp_1",
                "encrypted_content": "encrypted-compaction"
            }
        }))
        .unwrap();

        assert!(event.is_some());
    }

    #[test]
    fn fills_missing_terminal_response_ids_before_deserializing() {
        let mut response = super::normalize::tests::completed_response_json();
        response.as_object_mut().unwrap().remove("id");
        response["output"][1].as_object_mut().unwrap().remove("id");

        let event = parse_stream_event(serde_json::json!({
            "type": "response.completed",
            "sequence_number": 1,
            "response": response
        }))
        .unwrap();

        assert!(event.is_some());
    }

    #[test]
    fn infers_terminal_response_status_from_the_event_type() {
        let mut response = super::normalize::tests::completed_response_json();
        response.as_object_mut().unwrap().remove("status");
        response["output"][1]
            .as_object_mut()
            .unwrap()
            .remove("status");

        let event = parse_stream_event(serde_json::json!({
            "type": "response.completed",
            "sequence_number": 1,
            "response": response
        }))
        .unwrap();

        assert!(event.is_some());
    }

    #[test]
    fn infers_completed_status_for_terminal_function_calls() {
        let mut response = super::normalize::tests::completed_response_json();
        response["output"][2]
            .as_object_mut()
            .unwrap()
            .remove("status");

        let event = normalize_stream_event_json(serde_json::json!({
            "type": "response.completed",
            "sequence_number": 1,
            "response": response
        }));

        assert_eq!(
            event["response"]["output"][2]["status"],
            serde_json::json!("completed")
        );
    }

    #[test]
    fn parse_stream_event_accepts_terminal_compaction_item() {
        let mut response = super::normalize::tests::completed_response_json();
        response["output"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({
                "type": "compaction",
                "id": "cmp_1",
                "encrypted_content": "encrypted-compaction"
            }));

        let event = parse_stream_event(serde_json::json!({
            "type": "response.completed",
            "sequence_number": 1,
            "response": response
        }))
        .unwrap();

        assert!(event.is_some());
    }

    #[test]
    fn accepts_supported_lifecycle_and_completion_events() {
        let response = super::normalize::tests::completed_response_json();
        let events = [
            serde_json::json!({
                "type": "response.created",
                "sequence_number": 1,
                "response": response
            }),
            serde_json::json!({
                "type": "response.queued",
                "sequence_number": 2,
                "response": super::normalize::tests::completed_response_json()
            }),
            serde_json::json!({
                "type": "response.in_progress",
                "sequence_number": 3,
                "response": super::normalize::tests::completed_response_json()
            }),
            serde_json::json!({
                "type": "response.content_part.added",
                "sequence_number": 4,
                "item_id": "msg",
                "output_index": 0,
                "content_index": 0,
                "part": {"type": "output_text", "text": "", "annotations": [], "logprobs": null}
            }),
            serde_json::json!({
                "type": "response.content_part.done",
                "sequence_number": 5,
                "item_id": "msg",
                "output_index": 0,
                "content_index": 0,
                "part": {"type": "output_text", "text": "done", "annotations": [], "logprobs": null}
            }),
            serde_json::json!({
                "type": "response.output_text.done",
                "sequence_number": 6,
                "item_id": "msg",
                "output_index": 0,
                "content_index": 0,
                "text": "done",
                "logprobs": []
            }),
            serde_json::json!({
                "type": "response.refusal.done",
                "sequence_number": 7,
                "item_id": "msg",
                "output_index": 0,
                "content_index": 0,
                "refusal": "no"
            }),
            serde_json::json!({
                "type": "response.reasoning_summary_part.added",
                "sequence_number": 8,
                "item_id": "rs",
                "output_index": 0,
                "summary_index": 0,
                "part": {"type": "summary_text", "text": ""}
            }),
            serde_json::json!({
                "type": "response.reasoning_summary_part.done",
                "sequence_number": 9,
                "item_id": "rs",
                "output_index": 0,
                "summary_index": 0,
                "part": {"type": "summary_text", "text": "done"}
            }),
            serde_json::json!({
                "type": "response.reasoning_summary_text.done",
                "sequence_number": 10,
                "item_id": "rs",
                "output_index": 0,
                "summary_index": 0,
                "text": "done"
            }),
            serde_json::json!({
                "type": "response.reasoning_text.done",
                "sequence_number": 11,
                "item_id": "rs",
                "output_index": 0,
                "content_index": 0,
                "text": "done"
            }),
            serde_json::json!({
                "type": "response.function_call_arguments.delta",
                "sequence_number": 12,
                "item_id": "fc",
                "output_index": 1,
                "delta": "{\"x\":"
            }),
            serde_json::json!({
                "type": "response.function_call_arguments.done",
                "sequence_number": 13,
                "item_id": "fc",
                "output_index": 1,
                "name": null,
                "arguments": "{\"x\":1}"
            }),
        ];

        for value in events {
            assert!(parse_stream_event(value).unwrap().is_some());
        }
    }

    fn adapter_with_state(
        base_url: &str,
        model: &str,
    ) -> (
        super::OpenAiResponsesAdapter,
        crate::ResponsesConversationState,
    ) {
        let adapter = super::OpenAiResponsesAdapter::new("test-key", base_url, None);
        let state = crate::ResponsesConversationState {
            version: 1,
            provider: "openai_responses".to_string(),
            base_url: base_url.to_string(),
            model: model.to_string(),
            input_items: vec![],
            compaction_id: None,
            is_compacted: false,
            logical_message_count: 0,
            logical_context_hash: String::new(),
        };
        (adapter, state)
    }

    #[test]
    fn state_binding_accepts_matching_provider_and_base_url() {
        let (adapter, state) = adapter_with_state("https://api.openai.com/v1", "gpt-5");
        adapter
            .validate_state_binding(&state)
            .expect("matching binding is valid");
    }

    #[test]
    fn state_binding_rejects_another_provider() {
        let (adapter, mut state) = adapter_with_state("https://api.openai.com/v1", "gpt-5");
        state.provider = "anthropic".to_string();
        let error = adapter
            .validate_state_binding(&state)
            .unwrap_err()
            .to_string();
        assert!(error.contains("anthropic"));
        assert!(error.contains("openai_responses"));
    }

    #[test]
    fn state_binding_rejects_another_base_url() {
        let (adapter, mut state) = adapter_with_state("https://api.openai.com/v1", "gpt-5");
        state.base_url = "https://other.example.com/v1".to_string();
        let error = adapter
            .validate_state_binding(&state)
            .unwrap_err()
            .to_string();
        assert!(error.contains("other.example.com"));
    }

    #[test]
    fn state_binding_allows_another_model_for_experiment() {
        let (adapter, mut state) = adapter_with_state("https://api.openai.com/v1", "gpt-5");
        state.model = "gpt-4o".to_string();
        adapter
            .validate_state_binding(&state)
            .expect("model mismatch should not be rejected by local state validation");
    }

    /// Run with:
    /// `cargo test -p tact_llm live_responses_stream_handles_test_endpoint -- --ignored --nocapture`
    #[ignore = "hits a real Responses endpoint and requires OPENAI_API_KEY_TEST and OPENAI_BASE_URL_TEST"]
    #[tokio::test]
    async fn live_responses_stream_handles_test_endpoint() {
        dotenvy::dotenv().ok();

        let api_key = std::env::var("OPENAI_API_KEY_TEST")
            .expect("OPENAI_API_KEY_TEST must be set for the live Responses test");
        let base_url = std::env::var("OPENAI_BASE_URL_TEST")
            .expect("OPENAI_BASE_URL_TEST must be set for the live Responses test");
        let model =
            std::env::var("OPENAI_MODEL_TEST").unwrap_or_else(|_| "gpt-5.4-mini".to_string());
        let first_user = Message::new_text(Role::User, "Reply with the single word: responses.");
        let request = CreateMessageParams::new(RequiredMessageParams {
            model,
            messages: vec![first_user.clone()],
            max_tokens: 128,
        });
        let adapter = super::OpenAiResponsesAdapter::new(api_key, base_url, None);

        let response = adapter
            .stream_message(&request, None, None)
            .await
            .expect("Responses stream request should succeed");
        let blocks = response.blocks;
        let stop_reason = response.stop_reason;
        let request_body = response.request_body;
        let visible_text = blocks
            .iter()
            .filter_map(|block| match block {
                crate::ContentBlock::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<String>();

        assert_eq!(stop_reason, Some(StopReason::EndTurn));
        assert!(
            !visible_text.trim().is_empty(),
            "expected visible response text"
        );
        assert!(
            request_body.is_some(),
            "expected serialized Responses request"
        );

        let follow_up = CreateMessageParams::new(RequiredMessageParams {
            model: request.model.clone(),
            messages: vec![
                first_user,
                Message::new_blocks(Role::Assistant, blocks),
                Message::new_text(Role::User, "Reply with the single word: followup."),
            ],
            max_tokens: 128,
        });
        let response = adapter
            .stream_message(&follow_up, None, None)
            .await
            .expect("second Responses stream request should succeed");
        let follow_up_blocks = response.blocks;
        let follow_up_stop_reason = response.stop_reason;

        assert_eq!(follow_up_stop_reason, Some(StopReason::EndTurn));
        assert!(follow_up_blocks.iter().any(|block| {
            matches!(block, crate::ContentBlock::Text { text } if !text.trim().is_empty())
        }));
    }

    /// Run with:
    /// `cargo test -p tact_llm live_responses_stream_calls_tool_on_test_endpoint -- --ignored --nocapture`
    #[ignore = "hits a real Responses endpoint and requires OPENAI_API_KEY_TEST and OPENAI_BASE_URL_TEST"]
    #[tokio::test]
    async fn live_responses_stream_calls_tool_on_test_endpoint() {
        dotenvy::dotenv().ok();

        let api_key = std::env::var("OPENAI_API_KEY_TEST")
            .expect("OPENAI_API_KEY_TEST must be set for the live Responses test");
        let base_url = std::env::var("OPENAI_BASE_URL_TEST")
            .expect("OPENAI_BASE_URL_TEST must be set for the live Responses test");
        let model =
            std::env::var("OPENAI_MODEL_TEST").unwrap_or_else(|_| "gpt-5.4-mini".to_string());
        let first_user = Message::new_text(Role::User, "commit");
        let system = "You are a coding agent. Complete the user's request instead of stopping after an \
             explanation. Before committing, use the bash tool to inspect repository status. \
             After receiving that result, use bash again to create the commit.";
        let tools = vec![Tool {
            name: "bash".into(),
            description: Some("Run a shell command in the repository".into()),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {"cmd": {"type": "string"}},
                "required": ["cmd"]
            }),
        }];
        let request = CreateMessageParams::new(RequiredMessageParams {
            model,
            messages: vec![first_user.clone()],
            max_tokens: 512,
        })
        .with_system(system)
        .with_tools(tools.clone());
        let adapter = super::OpenAiResponsesAdapter::new(api_key, base_url, None);

        let response = adapter
            .stream_message(&request, None, None)
            .await
            .expect("Responses stream request with a tool should succeed");
        let blocks = response.blocks;
        let stop_reason = response.stop_reason;

        assert_eq!(
            stop_reason,
            Some(StopReason::ToolUse),
            "blocks: {blocks:#?}"
        );
        assert!(
            blocks
                .iter()
                .any(|block| matches!(block, ContentBlock::ToolUse { name, .. } if name == "bash")),
            "expected a bash tool call, got: {blocks:#?}"
        );

        let tool_use_id = blocks
            .iter()
            .find_map(|block| match block {
                ContentBlock::ToolUse { id, .. } => Some(id.clone()),
                _ => None,
            })
            .expect("expected tool call id");
        let follow_up = CreateMessageParams::new(RequiredMessageParams {
            model: request.model.clone(),
            messages: vec![
                first_user,
                Message::new_blocks(Role::Assistant, blocks),
                Message::new_blocks(
                    Role::User,
                    vec![ContentBlock::ToolResult {
                        tool_use_id,
                        content: "## main\n M src/lib.rs".into(),
                    }],
                ),
            ],
            max_tokens: 512,
        })
        .with_system(system)
        .with_tools(tools);

        let response = adapter
            .stream_message(&follow_up, None, None)
            .await
            .expect("Responses follow-up after a tool result should succeed");
        let follow_up_blocks = response.blocks;
        let follow_up_stop_reason = response.stop_reason;

        assert_eq!(
            follow_up_stop_reason,
            Some(StopReason::ToolUse),
            "blocks: {follow_up_blocks:#?}"
        );
        assert!(
            follow_up_blocks
                .iter()
                .any(|block| matches!(block, ContentBlock::ToolUse { name, .. } if name == "bash"))
        );
    }

    // ── Regression: configured `responses_compact_threshold` must reach
    // ── ordinary `/responses` requests as `context_management` ──────────────

    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn simple_request() -> CreateMessageParams {
        CreateMessageParams::new(RequiredMessageParams {
            model: "gpt-5".to_string(),
            messages: vec![Message::new_text(Role::User, "hello")],
            max_tokens: 256,
        })
    }

    /// Minimal terminal `/responses` body accepted by `normalize_response`.
    fn completed_response_value() -> serde_json::Value {
        serde_json::json!({
            "id": "resp_1",
            "object": "response",
            "created_at": 1754000001,
            "completed_at": 1754000002,
            "status": "completed",
            "model": "gpt-5",
            "output": [{
                "type": "message",
                "id": "msg_1",
                "role": "assistant",
                "status": "completed",
                "content": [{
                    "type": "output_text",
                    "annotations": [],
                    "logprobs": null,
                    "text": "hello"
                }]
            }],
            "usage": {
                "input_tokens": 10,
                "input_tokens_details": {"cached_tokens": 0},
                "output_tokens": 2,
                "output_tokens_details": {"reasoning_tokens": 0},
                "total_tokens": 12
            }
        })
    }

    /// Build the adapter through `ProviderInfo::build_client()` so the whole
    /// configuration → adapter wiring (including the Responses threshold) is
    /// exercised, then return the captured `/responses` request body.
    async fn captured_request_body(
        server: &MockServer,
        compact_threshold: Option<u32>,
    ) -> serde_json::Value {
        let info = crate::ProviderInfo {
            api_key: "test-key".to_string(),
            base_url: server.uri(),
            model: "gpt-5".to_string(),
            provider: crate::ProviderKind::OpenAi,
            protocol: crate::OpenAiProtocol::Responses,
            responses_compact_threshold: compact_threshold,
        };
        let crate::LlmProvider::OpenAiResponses(adapter) = info.build_client().unwrap() else {
            panic!("expected OpenAiResponses adapter");
        };
        adapter
            .create_message(&simple_request(), None)
            .await
            .expect("ordinary Responses request should succeed");
        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 1, "exactly one ordinary /responses request");
        serde_json::from_slice(&requests[0].body).unwrap()
    }

    #[test]
    fn unknown_output_fixture_is_well_formed() {
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("fixtures/unknown_output_item.json")).unwrap();
        assert_eq!(fixture["output"][0]["type"], "future_item");
        assert_eq!(fixture["output"][1]["type"], "message");
    }

    #[tokio::test]
    async fn ordinary_response_state_update_retains_unknown_output_item() {
        let server = MockServer::start().await;
        let mut response = completed_response_value();
        response["output"].as_array_mut().unwrap().insert(
            0,
            serde_json::json!({
                "type": "future_item",
                "id": "future-1",
                "payload": {"x": 1}
            }),
        );
        Mock::given(method("POST"))
            .and(path("/responses"))
            .respond_with(ResponseTemplate::new(200).set_body_json(response))
            .expect(1)
            .mount(&server)
            .await;

        let info = crate::ProviderInfo {
            api_key: "test-key".to_string(),
            base_url: server.uri(),
            model: "gpt-5".to_string(),
            provider: crate::ProviderKind::OpenAi,
            protocol: crate::OpenAiProtocol::Responses,
            responses_compact_threshold: None,
        };
        let crate::LlmProvider::OpenAiResponses(adapter) = info.build_client().unwrap() else {
            panic!("expected OpenAiResponses adapter");
        };
        let response = adapter
            .create_message(&simple_request(), None)
            .await
            .expect("unknown output item must not abort ordinary response");
        let crate::ProviderStateUpdate::Replace(crate::ProviderConversationState::OpenAiResponses(
            state,
        )) = response.state_update
        else {
            panic!("expected Responses state replacement");
        };
        assert_eq!(state.input_items[0]["type"], "message");
        assert_eq!(state.input_items[1]["type"], "future_item");
        server.verify().await;
    }

    #[tokio::test]
    async fn ordinary_responses_request_includes_context_management_when_threshold_configured() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/responses"))
            .respond_with(ResponseTemplate::new(200).set_body_json(completed_response_value()))
            .expect(1)
            .mount(&server)
            .await;

        let body = captured_request_body(&server, Some(160_000)).await;

        assert_eq!(
            body["context_management"][0]["type"],
            serde_json::json!("compaction"),
            "ordinary /responses request must declare native compaction, got: {body}"
        );
        assert_eq!(
            body["context_management"][0]["compact_threshold"],
            serde_json::json!(160_000),
            "configured threshold must reach the wire request, got: {body}"
        );
        server.verify().await;
    }

    #[tokio::test]
    async fn ordinary_responses_request_omits_context_management_without_threshold() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/responses"))
            .respond_with(ResponseTemplate::new(200).set_body_json(completed_response_value()))
            .expect(1)
            .mount(&server)
            .await;

        let body = captured_request_body(&server, None).await;

        assert!(
            body.get("context_management").is_none(),
            "no threshold must mean no context_management (no default injection), got: {body}"
        );
        server.verify().await;
    }

    #[tokio::test]
    async fn streamed_responses_request_includes_context_management_when_threshold_configured() {
        let server = MockServer::start().await;
        let sse = format!(
            "data: {}\n\n",
            serde_json::json!({
                "type": "response.completed",
                "sequence_number": 1,
                "response": completed_response_value()
            })
        );
        Mock::given(method("POST"))
            .and(path("/responses"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(sse, "text/event-stream"))
            .expect(1)
            .mount(&server)
            .await;

        let info = crate::ProviderInfo {
            api_key: "test-key".to_string(),
            base_url: server.uri(),
            model: "gpt-5".to_string(),
            provider: crate::ProviderKind::OpenAi,
            protocol: crate::OpenAiProtocol::Responses,
            responses_compact_threshold: Some(160_000),
        };
        let crate::LlmProvider::OpenAiResponses(adapter) = info.build_client().unwrap() else {
            panic!("expected OpenAiResponses adapter");
        };
        adapter
            .stream_message(&simple_request(), None, None)
            .await
            .expect("streamed Responses request should succeed");

        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 1, "exactly one streamed /responses request");
        let body: serde_json::Value = serde_json::from_slice(&requests[0].body).unwrap();
        assert_eq!(
            body["context_management"][0]["compact_threshold"],
            serde_json::json!(160_000),
            "streamed ordinary request must carry the configured threshold, got: {body}"
        );
        server.verify().await;
    }

    #[tokio::test]
    async fn unknown_stream_fixture_keeps_visible_text_and_state_item() {
        let server = MockServer::start().await;
        let sse = include_str!("fixtures/unknown_event_stream.jsonl");
        Mock::given(method("POST"))
            .and(path("/responses"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(sse, "text/event-stream"))
            .expect(1)
            .mount(&server)
            .await;
        let adapter = super::OpenAiResponsesAdapter::new("test-key", server.uri(), None);
        let response = adapter
            .stream_message(&simple_request(), None, None)
            .await
            .expect("unknown stream event must not abort response");
        assert!(response.blocks.iter().any(|block| {
            matches!(block, crate::ContentBlock::Text { text } if text == "hello")
        }));
        let crate::ProviderStateUpdate::Replace(crate::ProviderConversationState::OpenAiResponses(
            state,
        )) = response.state_update
        else {
            panic!("expected Responses state replacement");
        };
        assert!(
            state
                .input_items
                .iter()
                .any(|item| item["type"] == "future_item")
        );
        server.verify().await;
    }

    #[tokio::test]
    async fn compact_preserves_the_resource_reported_usage() {
        let server = MockServer::start().await;
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("fixtures/explicit_compact.json")).unwrap();
        Mock::given(method("POST"))
            .and(path("/responses/compact"))
            .respond_with(ResponseTemplate::new(200).set_body_json(fixture))
            .expect(1)
            .mount(&server)
            .await;

        let crate::LlmProvider::OpenAiResponses(adapter) = crate::ProviderInfo {
            api_key: "test-key".to_string(),
            base_url: server.uri(),
            model: "gpt-5.4-mini".to_string(),
            provider: crate::ProviderKind::OpenAi,
            protocol: crate::OpenAiProtocol::Responses,
            responses_compact_threshold: None,
        }
        .build_client()
        .unwrap() else {
            panic!("expected OpenAiResponses adapter");
        };
        let response = adapter
            .compact(&simple_request(), None)
            .await
            .expect("native compact should succeed");
        let usage = response
            .usage
            .expect("compact resource usage must be preserved");
        assert_eq!(usage.total, 1540);
        assert_eq!(usage.prompt, 1200);
        assert_eq!(usage.completion, 340);
        server.verify().await;
    }

    #[tokio::test]
    async fn compact_reports_missing_endpoint_clearly() {
        let server = MockServer::start().await;
        // A compatible proxy that does not implement /responses/compact
        // returns 404 with an HTML page. The adapter must surface a clear
        // message instead of dumping the HTML body through the SDK's
        // JSON-deserialization error.
        Mock::given(method("POST"))
            .and(path("/responses/compact"))
            .respond_with(
                ResponseTemplate::new(404).set_body_string(
                    "<!DOCTYPE html><html><title>Not Found | proxy</title></html>",
                ),
            )
            .expect(1)
            .mount(&server)
            .await;

        let crate::LlmProvider::OpenAiResponses(adapter) = crate::ProviderInfo {
            api_key: "test-key".to_string(),
            base_url: server.uri(),
            model: "gpt-5.4-mini".to_string(),
            provider: crate::ProviderKind::OpenAi,
            protocol: crate::OpenAiProtocol::Responses,
            responses_compact_threshold: None,
        }
        .build_client()
        .unwrap() else {
            panic!("expected OpenAiResponses adapter");
        };
        let error = adapter
            .compact(&simple_request(), None)
            .await
            .expect_err("missing /responses/compact must fail");
        let message = error.to_string();
        assert!(
            message.contains("does not support POST /responses/compact"),
            "expected a clear unsupported-endpoint message, got: {message}"
        );
        assert!(
            !message.contains("<!DOCTYPE html>"),
            "the HTML body must not leak into the error, got: {message}"
        );
        server.verify().await;
    }
}
