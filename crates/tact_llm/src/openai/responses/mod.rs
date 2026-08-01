mod convert;
mod history;
mod normalize;
mod stream;

use async_openai_responses::{
    Client,
    config::OpenAIConfig,
    types::responses::{Response, ResponseStreamEvent},
};
use futures_util::StreamExt;
use serde_json::Value;
use tact_protocol::AgentUpdate;
use tokio::sync::mpsc::UnboundedSender;

use self::{
    convert::create_response,
    normalize::{NormalizedResponse, parse_compact_resource},
    stream::ResponsesStreamState,
};
use crate::{
    CreateMessageParams, LlmClient, LlmError, LlmRequestBody, LlmResponse, OpenAiReasoningEffort,
    ProviderConversationState, ProviderStateUpdate, ResponsesConversationState, context_hash,
};

fn set_default_id(value: &mut Value, default_id: String) {
    let Some(object) = value.as_object_mut() else {
        return;
    };
    if !object.get("id").is_some_and(Value::is_string) {
        object.insert("id".to_string(), Value::String(default_id));
    }
}

fn normalize_stream_event_json(mut event: Value) -> Value {
    let event_type = event.get("type").and_then(Value::as_str);
    let terminal_status = match event_type {
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

fn parse_stream_event(event: Value) -> Result<Option<ResponseStreamEvent>, LlmError> {
    let Some(event_type) = event.get("type").and_then(Value::as_str) else {
        return Err(LlmError::StreamParse(
            "missing field `type` in OpenAI Responses stream event".to_string(),
        ));
    };
    let consumed = matches!(
        event_type,
        "error"
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
    serde_json::from_value(normalize_stream_event_json(event))
        .map(Some)
        .map_err(|error| {
            LlmError::StreamParse(format!(
                "deserialize OpenAI Responses stream event: {error}"
            ))
        })
}

/// OpenAI Responses API adapter backed by async-openai 0.41.x.
#[derive(Clone)]
pub struct OpenAiResponsesAdapter {
    client: Client<OpenAIConfig>,
    base_url: String,
    reasoning_effort: Option<OpenAiReasoningEffort>,
    /// Optional `context_management.compact_threshold` (tokens) sent on
    /// every ordinary `/responses` request. `None` omits `context_management`
    /// entirely; Tact never falls back to a local summary compaction for
    /// Responses providers.
    compact_threshold: Option<u32>,
}

impl OpenAiResponsesAdapter {
    pub fn new(
        api_key: impl Into<String>,
        base_url: impl Into<String>,
        reasoning_effort: Option<OpenAiReasoningEffort>,
        compact_threshold: Option<u32>,
    ) -> Self {
        let base_url = base_url.into().trim_end_matches('/').to_string();
        let config = OpenAIConfig::new()
            .with_api_key(api_key)
            .with_api_base(base_url.clone())
            .with_org_id("")
            .with_project_id("");
        Self {
            client: Client::with_config(config),
            base_url,
            reasoning_effort,
            compact_threshold,
        }
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Builds the ordinary `/responses` wire request for this adapter,
    /// including `context_management` when a compact threshold is
    /// configured. Shared by the streaming and non-streaming paths so the
    /// configured threshold can never be dropped by one of them.
    fn build_wire_request(
        &self,
        request: &CreateMessageParams,
        provider_state: Option<&ProviderConversationState>,
    ) -> Result<(serde_json::Value, Vec<serde_json::Value>), LlmError> {
        create_response(
            request,
            provider_state,
            self.compact_threshold,
            self.reasoning_effort,
        )
    }

    /// Validates that a persisted Responses state is bound to this adapter's
    /// provider, base URL, and request model before it is reused.
    fn validate_state_binding(
        &self,
        state: &ResponsesConversationState,
        model: &str,
    ) -> Result<(), LlmError> {
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
        if state.model != model {
            return Err(LlmError::Unsupported(format!(
                "provider state is bound to model '{}', expected '{}'",
                state.model, model
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
            request.messages.len(),
            context_hash(&request.messages)?,
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
            self.validate_state_binding(state, &request.model)?;
        }
        let (mut wire_request, input_items) = self.build_wire_request(request, provider_state)?;
        wire_request["stream"] = serde_json::Value::Bool(true);
        let request_body = serde_json::to_vec(&wire_request)?;
        let mut response_stream = self
            .client
            .responses()
            .create_stream_byot::<_, Value>(wire_request)
            .await
            .map_err(LlmError::from)?;
        let mut state = ResponsesStreamState::default();

        while let Some(result) = response_stream.next().await {
            let event = match result {
                Ok(event) => match parse_stream_event(event)? {
                    Some(event) => event,
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
            let updates = match state.apply(event) {
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
            self.validate_state_binding(state, &request.model)?;
        }
        let (mut wire_request, input_items) = self.build_wire_request(request, provider_state)?;
        wire_request["stream"] = serde_json::Value::Bool(false);
        let request_body = serde_json::to_vec(&wire_request)?;
        let response = self
            .client
            .responses()
            .create_byot::<_, Response>(wire_request)
            .await
            .map_err(LlmError::from)?;
        let normalized = normalize::normalize_response(response)?;
        let state_update = self.state_update(&normalized, request, input_items)?;
        Ok(Self::into_result(normalized, request_body, state_update))
    }

    async fn compact(
        &self,
        request: &CreateMessageParams,
        provider_state: Option<&ProviderConversationState>,
    ) -> Result<LlmResponse, LlmError> {
        if let Some(ProviderConversationState::OpenAiResponses(state)) = provider_state {
            self.validate_state_binding(state, &request.model)?;
        }
        // The compact request carries the current protocol baseline plus any
        // logical messages not yet represented in it. The exact JSON input
        // items (including unknown/future item types) are preserved by
        // sending the request through the byot JSON path; no local summary
        // prompt or `create_message()` call is used.
        let (body, _) = create_response(request, provider_state, None, None)?;
        let compact_request = serde_json::json!({
            "model": request.model,
            "input": body["input"],
        });
        let request_body = serde_json::to_vec(&compact_request)?;
        let resource = self
            .client
            .responses()
            .compact_byot::<_, Value>(compact_request)
            .await
            .map_err(LlmError::from)?;
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
            usage: None,
            request_body: Some(request_body),
            state_update: ProviderStateUpdate::Replace(ProviderConversationState::OpenAiResponses(
                state,
            )),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{normalize_stream_event_json, parse_stream_event};
    use crate::{
        ContentBlock, CreateMessageParams, LlmClient, Message, RequiredMessageParams, Role,
        StopReason, Tool,
    };

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
    fn skips_unconsumed_events_without_deserializing_provider_specific_items() {
        let event = parse_stream_event(serde_json::json!({
            "type": "response.content_part.added",
            "sequence_number": 1,
            "item_id": "msg_1",
            "output_index": 0,
            "content_index": 0,
            "part": {"type": "output_text", "text": "answer", "annotations": []}
        }))
        .unwrap();

        assert!(event.is_none());
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

    fn adapter_with_state(
        base_url: &str,
        model: &str,
    ) -> (
        super::OpenAiResponsesAdapter,
        crate::ResponsesConversationState,
    ) {
        let adapter = super::OpenAiResponsesAdapter::new("test-key", base_url, None, None);
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
    fn state_binding_accepts_matching_provider_base_url_and_model() {
        let (adapter, state) = adapter_with_state("https://api.openai.com/v1", "gpt-5");
        adapter
            .validate_state_binding(&state, "gpt-5")
            .expect("matching binding is valid");
    }

    #[test]
    fn state_binding_rejects_another_provider() {
        let (adapter, mut state) = adapter_with_state("https://api.openai.com/v1", "gpt-5");
        state.provider = "anthropic".to_string();
        let error = adapter
            .validate_state_binding(&state, "gpt-5")
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
            .validate_state_binding(&state, "gpt-5")
            .unwrap_err()
            .to_string();
        assert!(error.contains("other.example.com"));
    }

    #[test]
    fn state_binding_rejects_another_model() {
        let (adapter, mut state) = adapter_with_state("https://api.openai.com/v1", "gpt-5");
        state.model = "gpt-4o".to_string();
        let error = adapter
            .validate_state_binding(&state, "gpt-5")
            .unwrap_err()
            .to_string();
        assert!(error.contains("gpt-4o"));
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
        let adapter = super::OpenAiResponsesAdapter::new(api_key, base_url, None, None);

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
        let adapter = super::OpenAiResponsesAdapter::new(api_key, base_url, None, None);

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
            reasoning_effort: None,
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
            reasoning_effort: None,
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
}
