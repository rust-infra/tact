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
use tact_protocol::{AgentUpdate, TokenUsageInfo};
use tokio::sync::mpsc::UnboundedSender;

use self::{convert::create_response, normalize::NormalizedResponse, stream::ResponsesStreamState};
use crate::{
    ContentBlock, CreateMessageParams, LlmClient, LlmError, LlmRequestBody, OpenAiReasoningEffort,
    StopReason,
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
        return Err(LlmError::Other(
            "deserialize OpenAI Responses stream event: missing field `type`".to_string(),
        ));
    };
    let consumed = matches!(
        event_type,
        "error"
            | "response.reasoning_summary_text.delta"
            | "response.reasoning_text.delta"
            | "response.output_text.delta"
            | "response.refusal.delta"
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
            LlmError::Other(format!(
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
}

impl OpenAiResponsesAdapter {
    pub fn new(
        api_key: impl Into<String>,
        base_url: impl Into<String>,
        reasoning_effort: Option<OpenAiReasoningEffort>,
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
        }
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    fn into_result(
        normalized: NormalizedResponse,
        request_body: LlmRequestBody,
    ) -> (
        Vec<ContentBlock>,
        Option<StopReason>,
        Option<TokenUsageInfo>,
        Option<LlmRequestBody>,
    ) {
        (
            normalized.blocks,
            normalized.stop_reason,
            normalized.usage,
            Some(request_body),
        )
    }
}

#[async_trait::async_trait]
impl LlmClient for OpenAiResponsesAdapter {
    async fn stream_message(
        &self,
        request: &CreateMessageParams,
        ui_tx: Option<UnboundedSender<AgentUpdate>>,
    ) -> Result<
        (
            Vec<ContentBlock>,
            Option<StopReason>,
            Option<TokenUsageInfo>,
            Option<LlmRequestBody>,
        ),
        LlmError,
    > {
        let mut wire_request = create_response(request, self.reasoning_effort)?;
        wire_request["stream"] = serde_json::Value::Bool(true);
        let request_body = serde_json::to_vec(&wire_request)
            .map_err(|error| LlmError::Other(format!("serialize Responses request: {error}")))?;
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
        Ok(Self::into_result(normalized, request_body))
    }

    async fn create_message(
        &self,
        request: &CreateMessageParams,
    ) -> Result<
        (
            Vec<ContentBlock>,
            Option<StopReason>,
            Option<TokenUsageInfo>,
            Option<LlmRequestBody>,
        ),
        LlmError,
    > {
        let mut wire_request = create_response(request, self.reasoning_effort)?;
        wire_request["stream"] = serde_json::Value::Bool(false);
        let request_body = serde_json::to_vec(&wire_request)
            .map_err(|error| LlmError::Other(format!("serialize Responses request: {error}")))?;
        let response = self
            .client
            .responses()
            .create_byot::<_, Response>(wire_request)
            .await
            .map_err(LlmError::from)?;
        Ok(Self::into_result(
            normalize::normalize_response(response)?,
            request_body,
        ))
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
            "type": "response.output_item.added",
            "sequence_number": 1,
            "output_index": 0,
            "item": {"type": "message", "role": "assistant", "content": []}
        }))
        .unwrap();

        assert!(event.is_none());
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

        let (blocks, stop_reason, _usage, request_body) = adapter
            .stream_message(&request, None)
            .await
            .expect("Responses stream request should succeed");
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
        let (follow_up_blocks, follow_up_stop_reason, _usage, _request_body) = adapter
            .stream_message(&follow_up, None)
            .await
            .expect("second Responses stream request should succeed");

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

        let (blocks, stop_reason, _usage, _request_body) = adapter
            .stream_message(&request, None)
            .await
            .expect("Responses stream request with a tool should succeed");

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

        let (follow_up_blocks, follow_up_stop_reason, _usage, _request_body) = adapter
            .stream_message(&follow_up, None)
            .await
            .expect("Responses follow-up after a tool result should succeed");

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
}
