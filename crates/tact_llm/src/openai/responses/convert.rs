use async_openai_responses::types::responses::{
    CreateResponseArgs, EasyInputContent, EasyInputMessage, FunctionCallOutput,
    FunctionCallOutputItemParam, FunctionTool, FunctionToolCall, IncludeEnum, InputContent,
    InputImageContent, InputItem, InputParam, InputTextContent, Item, MessageType, OutputStatus,
    Reasoning, ReasoningSummary, Role as ResponsesRole, Tool as ResponsesTool, ToolChoiceFunction,
    ToolChoiceOptions, ToolChoiceParam, WebSearchTool,
};

use super::history;
use crate::{
    ContentBlock, CreateMessageParams, LlmError, Message, MessageContent,
    ProviderConversationState, ResponsesConversationState, Role, ToolChoice, context_hash,
};

fn responses_role(role: Role) -> ResponsesRole {
    match role {
        Role::User => ResponsesRole::User,
        Role::Assistant => ResponsesRole::Assistant,
    }
}

fn message_item(role: Role, content: EasyInputContent) -> InputItem {
    InputItem::EasyMessage(EasyInputMessage {
        r#type: MessageType::Message,
        role: responses_role(role),
        content,
        phase: None,
    })
}

fn flush_message_content(role: Role, content: &mut Vec<InputContent>, input: &mut Vec<InputItem>) {
    if !content.is_empty() {
        input.push(message_item(
            role,
            EasyInputContent::ContentList(std::mem::take(content)),
        ));
    }
}

fn reasoning_item(signature: &str) -> Result<Option<InputItem>, LlmError> {
    if signature.is_empty() {
        return Ok(None);
    }
    if let Some(state) = history::decode(signature)? {
        return Ok(Some(InputItem::Item(Item::Reasoning(state.reasoning))));
    }
    Ok(None)
}

fn message_to_input(message: &Message) -> Result<Vec<InputItem>, LlmError> {
    let Message { role, content, .. } = message;
    if let MessageContent::Text { content } = content {
        return Ok(vec![message_item(
            *role,
            EasyInputContent::Text(content.clone()),
        )]);
    }

    let MessageContent::Blocks { content } = content else {
        unreachable!("all MessageContent variants handled")
    };
    let mut function_call_item_ids = std::collections::BTreeMap::new();
    for block in content {
        if let ContentBlock::Thinking { signature, .. } = block
            && let Some(state) = history::decode(signature)?
        {
            function_call_item_ids.extend(state.function_call_item_ids);
        }
    }
    let mut input = Vec::new();
    let mut message_content = Vec::new();
    for block in content {
        match block {
            ContentBlock::Text { text } => {
                message_content.push(InputContent::InputText(InputTextContent {
                    text: text.clone(),
                }));
            }
            ContentBlock::Image { source } => {
                message_content.push(InputContent::InputImage(InputImageContent {
                    detail: Default::default(),
                    file_id: None,
                    image_url: Some(format!("data:{};base64,{}", source.media_type, source.data)),
                }));
            }
            ContentBlock::Thinking { signature, .. } => {
                flush_message_content(*role, &mut message_content, &mut input);
                if let Some(reasoning) = reasoning_item(signature)? {
                    input.push(reasoning);
                }
            }
            ContentBlock::RedactedThinking { .. } => {}
            ContentBlock::ToolUse {
                id,
                name,
                input: args,
            } => {
                flush_message_content(*role, &mut message_content, &mut input);
                let arguments = serde_json::to_string(args).map_err(|error| {
                    LlmError::Unsupported(format!("serialize arguments for tool '{name}': {error}"))
                })?;
                input.push(InputItem::Item(Item::FunctionCall(FunctionToolCall {
                    arguments,
                    call_id: id.clone(),
                    namespace: None,
                    name: name.clone(),
                    id: function_call_item_ids.get(id).cloned(),
                    status: Some(OutputStatus::Completed),
                })));
            }
            ContentBlock::ToolResult {
                tool_use_id,
                content,
            } => {
                flush_message_content(*role, &mut message_content, &mut input);
                input.push(InputItem::Item(Item::FunctionCallOutput(
                    FunctionCallOutputItemParam {
                        call_id: tool_use_id.clone(),
                        output: FunctionCallOutput::Text(content.clone()),
                        id: None,
                        status: Some(OutputStatus::Completed),
                    },
                )));
            }
        }
    }
    flush_message_content(*role, &mut message_content, &mut input);
    Ok(input)
}

fn tool_choice(tool_choice: &ToolChoice) -> ToolChoiceParam {
    match tool_choice {
        ToolChoice::Auto => ToolChoiceParam::Mode(ToolChoiceOptions::Auto),
        ToolChoice::Any => ToolChoiceParam::Mode(ToolChoiceOptions::Required),
        ToolChoice::None => ToolChoiceParam::Mode(ToolChoiceOptions::None),
        ToolChoice::Tool { name } => {
            ToolChoiceParam::Function(ToolChoiceFunction { name: name.clone() })
        }
    }
}

fn normalize_assistant_history_items(input: &mut [serde_json::Value]) {
    for (index, item) in input.iter_mut().enumerate() {
        if item.get("type").and_then(serde_json::Value::as_str) != Some("message")
            || item.get("role").and_then(serde_json::Value::as_str) != Some("assistant")
        {
            continue;
        }
        let Some(content) = item.get_mut("content") else {
            continue;
        };
        let output_content = match content {
            serde_json::Value::String(text) => vec![serde_json::json!({
                "type": "output_text",
                "text": text,
                "annotations": [],
            })],
            serde_json::Value::Array(parts) => parts
                .iter()
                .filter(|part| {
                    part.get("type").and_then(serde_json::Value::as_str) == Some("input_text")
                })
                .map(|part| {
                    serde_json::json!({
                        "type": "output_text",
                        "text": part.get("text").cloned().unwrap_or_default(),
                        "annotations": [],
                    })
                })
                .collect(),
            _ => continue,
        };
        *content = serde_json::Value::Array(output_content);
        item["id"] = serde_json::Value::String(format!("tact-assistant-history-{index}"));
        item["status"] = serde_json::Value::String("completed".to_string());
    }
}

/// Validates that a persisted Responses state may be reused for the given
/// request. The state version and provider must match, and the logical-message
/// prefix represented by the state must hash to the recorded value. The model
/// is intentionally not checked here so callers can experiment with model
/// changes; the provider may still reject incompatible wire state.
fn validate_conversion_state(
    state: &ResponsesConversationState,
    request: &CreateMessageParams,
) -> Result<(), LlmError> {
    if state.version != 1 {
        return Err(LlmError::Unsupported(format!(
            "provider state version {} is unsupported; expected version 1",
            state.version
        )));
    }
    if state.provider != "openai_responses" {
        return Err(LlmError::Unsupported(format!(
            "provider state is bound to provider '{}', expected 'openai_responses'",
            state.provider
        )));
    }
    if state.logical_message_count > request.messages.len() {
        return Err(LlmError::Unsupported(format!(
            "provider state covers {} logical messages but the request provides {}",
            state.logical_message_count,
            request.messages.len()
        )));
    }
    let expected_hash = context_hash(&request.messages[..state.logical_message_count])?;
    if expected_hash != state.logical_context_hash {
        return Err(LlmError::Unsupported(
            "provider state logical context hash mismatch; refusing to reuse a stale Responses baseline"
                .to_string(),
        ));
    }
    Ok(())
}

/// Builds a state-aware `/responses` request body.
///
/// Returns the serialized request body and the exact `input`-item JSON sent in
/// this request (the state baseline plus the newly converted uncovered
/// suffix). With no state, every logical message is converted. With an OpenAI
/// Responses state, the provider/model binding and the logical prefix hash are
/// validated before only the uncovered suffix is converted. The state baseline
/// items are reused verbatim as JSON so unknown fields and future item types
/// survive.
///
/// When `native_web_search` is true, a `Tool::WebSearch` is injected
/// alongside the function tools so the Provider can execute web search
/// server-side. This is independent of any MCP-provided `web_search`
/// function tool — it never inspects or replaces tool names.
pub(crate) fn create_response(
    request: &CreateMessageParams,
    provider_state: Option<&ProviderConversationState>,
    compact_threshold: Option<u32>,
    native_web_search: bool,
) -> Result<(serde_json::Value, Vec<serde_json::Value>), LlmError> {
    let (baseline, covered) = match provider_state {
        None => (Vec::new(), 0),
        Some(ProviderConversationState::OpenAiResponses(state)) => {
            validate_conversion_state(state, request)?;
            (state.input_items.clone(), state.logical_message_count)
        }
    };

    let mut converted = Vec::new();
    for message in &request.messages[covered..] {
        converted.extend(message_to_input(message)?);
    }

    let mut builder = CreateResponseArgs::default();
    builder
        .model(request.model.clone())
        .input(InputParam::Items(converted.clone()))
        .max_output_tokens(request.max_tokens)
        .include(vec![IncludeEnum::ReasoningEncryptedContent])
        .store(false);

    if let Some(system) = &request.system {
        builder.instructions(system.clone());
    }
    if let Some(temperature) = request.temperature {
        builder.temperature(temperature);
    }
    if let Some(top_p) = request.top_p {
        builder.top_p(top_p);
    }
    // Collect all tools: function tools from the request plus (when
    // enabled) the native web search hosted tool. Native web search is an
    // additional tool — it does not replace or inspect any function tool
    // (MCP-provided web_search stays as-is).
    let mut response_tools: Vec<ResponsesTool> = request
        .tools
        .as_ref()
        .map(|tools| {
            tools
                .iter()
                .map(|tool| {
                    ResponsesTool::Function(FunctionTool {
                        name: tool.name.clone(),
                        parameters: Some(tool.input_schema.clone()),
                        strict: None,
                        description: tool.description.clone(),
                        defer_loading: None,
                    })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if native_web_search {
        response_tools.push(ResponsesTool::WebSearch(WebSearchTool {
            user_location: None,
            search_context_size: None,
            filters: None,
            search_content_types: None,
        }));
    }
    let has_tools = !response_tools.is_empty();
    if has_tools {
        builder.tools(response_tools);
    }
    if let Some(choice) = &request.tool_choice {
        builder.tool_choice(tool_choice(choice));
    } else if has_tools {
        builder.tool_choice(ToolChoiceOptions::Auto);
    }
    if request.thinking.is_some() || request.reasoning_effort.is_some() {
        builder.reasoning(Reasoning {
            effort: None,
            summary: Some(ReasoningSummary::Detailed),
        });
    }

    let typed_request = builder.build().map_err(|error| {
        LlmError::Unsupported(format!("build OpenAI Responses request: {error}"))
    })?;
    let mut body = serde_json::to_value(typed_request)?;
    if let Some(options) = &request.responses_options {
        options.apply_to(&mut body)?;
    }

    // The exact input for this request: the state baseline (verbatim JSON)
    // followed by the newly converted uncovered items. Only the newly
    // converted assistant-history items are normalized; baseline items are
    // already in protocol shape and must not be rewritten.
    let mut new_items = Vec::with_capacity(converted.len());
    for item in converted {
        new_items.push(serde_json::to_value(item)?);
    }
    normalize_assistant_history_items(&mut new_items);
    let mut input_items = baseline;
    input_items.extend(new_items);
    body["input"] = serde_json::Value::Array(input_items.clone());

    if let Some(threshold) = compact_threshold {
        body["context_management"] = serde_json::json!([
            {
                "type": "compaction",
                "compact_threshold": threshold,
            }
        ]);
    }

    // Explicit per-request effort; None = omit (provider default, e.g. medium).
    if let Some(effort) = request.reasoning_effort {
        body["reasoning"]["effort"] = serde_json::Value::String(effort.as_str().to_owned());
    }
    Ok((body, input_items))
}

#[cfg(test)]
mod tests {
    use super::super::ResponsesRequestOptions;
    use super::super::normalize::parse_compact_resource;
    use super::{create_response, message_to_input};
    use crate::{
        ContentBlock, CreateMessageParams, ImageSource, Message, OpenAiReasoningEffort,
        RequiredMessageParams, ResponsesConversationState, Role, Thinking, ThinkingType, Tool,
        ToolChoice, context_hash,
    };

    fn state_covering_first_message(request: &CreateMessageParams) -> ResponsesConversationState {
        let first =
            serde_json::to_value(&message_to_input(&request.messages[0]).unwrap()[0]).unwrap();
        ResponsesConversationState {
            version: 1,
            provider: "openai_responses".into(),
            base_url: "https://api.openai.com/v1".into(),
            model: request.model.clone(),
            input_items: vec![first],
            compaction_id: None,
            is_compacted: false,
            logical_message_count: 1,
            logical_context_hash: context_hash(&request.messages[..1]).unwrap(),
        }
    }

    fn compact_resource_without_compaction_item() -> serde_json::Value {
        serde_json::json!({"id":"cmp-test","object":"response.compaction","output":[]})
    }

    fn compact_resource_with_empty_encrypted_content() -> serde_json::Value {
        serde_json::json!({"id":"cmp-test","object":"response.compaction","output":[
            {"type":"compaction","id":"cmp-item","encrypted_content":""}
        ]})
    }

    fn request_with_history() -> CreateMessageParams {
        let mut request = CreateMessageParams::new(RequiredMessageParams {
            model: "gpt-5".to_string(),
            messages: vec![
                Message::new_blocks(
                    Role::User,
                    vec![
                        ContentBlock::Text {
                            text: "inspect this".to_string(),
                        },
                        ContentBlock::Image {
                            source: ImageSource {
                                type_: "base64".to_string(),
                                media_type: "image/png".to_string(),
                                data: "aGVsbG8=".to_string(),
                            },
                        },
                    ],
                ),
                Message::new_blocks(
                    Role::Assistant,
                    vec![
                        ContentBlock::Thinking {
                            thinking: "summary".to_string(),
                            signature: "encrypted-payload".to_string(),
                        },
                        ContentBlock::Text {
                            text: "checking".to_string(),
                        },
                        ContentBlock::ToolUse {
                            id: "call-1".to_string(),
                            name: "bash".to_string(),
                            input: serde_json::json!({"cmd": "pwd"}),
                        },
                    ],
                ),
                Message::new_blocks(
                    Role::User,
                    vec![ContentBlock::ToolResult {
                        tool_use_id: "call-1".to_string(),
                        content: "/tmp/project".to_string(),
                    }],
                ),
            ],
            max_tokens: 4096,
        });
        request.system = Some("system instruction".to_string());
        request.temperature = Some(0.2);
        request.top_p = Some(0.8);
        request.tools = Some(vec![Tool {
            name: "bash".to_string(),
            description: Some("Run a command".to_string()),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {"cmd": {"type": "string"}},
                "required": ["cmd"]
            }),
        }]);
        request.tool_choice = Some(ToolChoice::Any);
        request.thinking = Some(Thinking {
            budget_tokens: 32_000,
            type_: ThinkingType::Enabled,
        });
        request
    }

    #[test]
    fn converts_multimodal_tool_history_and_options() {
        let (body, _) = create_response(&request_with_history(), None, None, false).unwrap();

        assert_eq!(body["model"], "gpt-5");
        assert_eq!(body["instructions"], "system instruction");
        assert_eq!(body["max_output_tokens"], 4096);
        assert!((body["temperature"].as_f64().unwrap() - 0.2).abs() < 1e-6);
        assert!((body["top_p"].as_f64().unwrap() - 0.8).abs() < 1e-6);
        assert_eq!(body["tool_choice"], "required");
        assert_eq!(body["tools"][0]["type"], "function");
        assert_eq!(body["tools"][0]["name"], "bash");

        let input = body["input"].as_array().unwrap();
        assert!(input.iter().any(|item| {
            item["role"] == "user"
                && item["content"].as_array().is_some_and(|content| {
                    content.iter().any(|part| {
                        part["type"] == "input_image"
                            && part["image_url"] == "data:image/png;base64,aGVsbG8="
                    })
                })
        }));
        assert!(input.iter().any(|item| {
            item["type"] == "function_call"
                && item["call_id"] == "call-1"
                && item["name"] == "bash"
                && item["arguments"] == r#"{"cmd":"pwd"}"#
        }));
        assert!(input.iter().any(|item| {
            item["type"] == "function_call_output"
                && item["call_id"] == "call-1"
                && item["output"] == "/tmp/project"
        }));
    }

    #[test]
    fn omits_unscoped_signature_from_another_provider() {
        let (body, _) = create_response(&request_with_history(), None, None, false).unwrap();

        assert_eq!(body["include"][0], "reasoning.encrypted_content");
        assert_eq!(body["reasoning"]["summary"], "detailed");
        let input = body["input"].as_array().unwrap();
        assert!(input.iter().all(|item| item["type"] != "reasoning"));
    }

    #[test]
    fn omits_reasoning_without_encrypted_payload() {
        let mut request = request_with_history();
        let crate::MessageContent::Blocks { content } = &mut request.messages[1].content else {
            panic!("expected blocks");
        };
        let ContentBlock::Thinking { signature, .. } = &mut content[0] else {
            panic!("expected thinking");
        };
        signature.clear();

        let (body, _) = create_response(&request, None, None, false).unwrap();
        assert!(
            body["input"]
                .as_array()
                .unwrap()
                .iter()
                .all(|item| item["type"] != "reasoning")
        );
    }

    #[test]
    fn converts_all_tool_choice_variants() {
        let cases = [
            (ToolChoice::Auto, serde_json::json!("auto")),
            (ToolChoice::Any, serde_json::json!("required")),
            (ToolChoice::None, serde_json::json!("none")),
            (
                ToolChoice::Tool {
                    name: "bash".to_string(),
                },
                serde_json::json!({"type": "function", "name": "bash"}),
            ),
        ];

        for (choice, expected) in cases {
            let mut request = request_with_history();
            request.tool_choice = Some(choice);
            let (body, _) = create_response(&request, None, None, false).unwrap();
            assert_eq!(body["tool_choice"], expected);
        }
    }

    #[test]
    fn defaults_tool_choice_to_auto_when_tools_are_present() {
        let request = CreateMessageParams::new(RequiredMessageParams {
            model: "gpt-5".into(),
            messages: vec![Message::new_text(Role::User, "run pwd")],
            max_tokens: 128,
        })
        .with_tools(vec![Tool {
            name: "bash".into(),
            description: Some("Run a shell command".into()),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {"cmd": {"type": "string"}},
                "required": ["cmd"]
            }),
        }]);

        let (body, _) = create_response(&request, None, None, false).unwrap();

        assert_eq!(body["tool_choice"], serde_json::json!("auto"));
    }

    #[test]
    fn serializes_explicit_max_reasoning_effort() {
        let (body, _) = create_response(
            &request_with_history().with_reasoning_effort(Some(OpenAiReasoningEffort::Max)),
            None,
            None,
            false,
        )
        .unwrap();
        assert_eq!(body["reasoning"]["effort"], "max");
    }

    #[test]
    fn serializes_explicit_low_reasoning_effort() {
        let (body, _) = create_response(
            &request_with_history().with_reasoning_effort(Some(OpenAiReasoningEffort::Low)),
            None,
            None,
            false,
        )
        .unwrap();
        assert_eq!(body["reasoning"]["effort"], "low");
    }

    #[test]
    fn serializes_assistant_history_as_completed_output_message() {
        let (body, _) = create_response(&request_with_history(), None, None, false).unwrap();
        let assistant = body["input"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["role"] == "assistant")
            .expect("assistant history item");

        assert_eq!(assistant["status"], "completed");
        assert!(
            assistant["id"]
                .as_str()
                .is_some_and(|id| id.starts_with("tact-assistant-history-"))
        );
        assert_eq!(assistant["content"][0]["type"], "output_text");
        assert_eq!(assistant["content"][0]["text"], "checking");
        assert_eq!(
            assistant["content"][0]["annotations"],
            serde_json::json!([])
        );
    }

    #[test]
    fn state_baseline_only_converts_uncovered_messages() {
        let request = request_with_history();
        let state = state_covering_first_message(&request);
        let (body, sent_items) = create_response(
            &request,
            Some(&crate::ProviderConversationState::OpenAiResponses(
                state.clone(),
            )),
            Some(160_000),
            false,
        )
        .unwrap();
        // The baseline (first converted user message) is reused verbatim; only
        // the uncovered suffix (assistant message, function call, function call
        // output) is converted.
        assert_eq!(sent_items.len(), 4);
        assert_eq!(body["input"].as_array().unwrap().len(), 4);
        assert_eq!(body["input"][0], state.input_items[0]);
        assert_eq!(body["input"][1]["role"], "assistant");
        assert!(
            body["input"]
                .as_array()
                .unwrap()
                .iter()
                .any(|item| item["type"] == "function_call" && item["call_id"] == "call-1")
        );
        assert!(
            body["input"]
                .as_array()
                .unwrap()
                .iter()
                .any(|item| item["type"] == "function_call_output" && item["call_id"] == "call-1")
        );
    }

    #[test]
    fn responses_request_injects_context_management_and_keeps_stateless_fields() {
        let request = request_with_history();
        let (body, _) = create_response(&request, None, Some(160_000), false).unwrap();
        assert_eq!(body["store"], false);
        assert!(body.get("previous_response_id").is_none());
        assert!(body.get("conversation").is_none());
        assert_eq!(body["context_management"][0]["type"], "compaction");
        assert_eq!(body["context_management"][0]["compact_threshold"], 160_000);
    }

    #[test]
    fn omits_context_management_without_a_threshold() {
        let request = request_with_history();
        let (body, _) = create_response(&request, None, None, false).unwrap();
        assert!(body.get("context_management").is_none());
    }

    #[test]
    fn explicit_compact_requires_one_non_empty_compaction_item() {
        let missing = compact_resource_without_compaction_item();
        assert!(parse_compact_resource(missing).is_err());
        let empty = compact_resource_with_empty_encrypted_content();
        assert!(parse_compact_resource(empty).is_err());
    }

    #[test]
    fn compact_resource_requires_expected_object_and_id() {
        let missing_object = serde_json::json!({
            "id":"cmp","output":[{"type":"compaction","id":"item","encrypted_content":"opaque"}]
        });
        assert!(parse_compact_resource(missing_object).is_err());

        let wrong_object = serde_json::json!({
            "id":"cmp","object":"response",
            "output":[{"type":"compaction","id":"item","encrypted_content":"opaque"}]
        });
        assert!(parse_compact_resource(wrong_object).is_err());

        let missing_id = serde_json::json!({
            "object":"response.compaction",
            "output":[{"type":"compaction","id":"item","encrypted_content":"opaque"}]
        });
        assert!(parse_compact_resource(missing_id).is_err());

        let empty_id = serde_json::json!({
            "id":"","object":"response.compaction",
            "output":[{"type":"compaction","id":"item","encrypted_content":"opaque"}]
        });
        assert!(parse_compact_resource(empty_id).is_err());
    }

    #[test]
    fn compact_resource_with_expected_object_and_id_is_accepted() {
        let valid = serde_json::json!({
            "id":"cmp-resource","object":"response.compaction",
            "output":[{"type":"compaction","id":"cmp-item","encrypted_content":"opaque"}]
        });
        let parsed = parse_compact_resource(valid).unwrap();
        assert_eq!(parsed.compaction_id, "cmp-item");
    }

    #[test]
    fn explicit_compact_fixture_parses_to_replacement_baseline() {
        let value: serde_json::Value =
            serde_json::from_str(include_str!("fixtures/explicit_compact.json")).unwrap();
        let parsed = parse_compact_resource(value).unwrap();
        assert_eq!(parsed.input_items.len(), 3);
        assert_eq!(parsed.input_items[2]["type"], "compaction");
        assert_eq!(parsed.compaction_id, "cmp_sanitized_01");
        // The retained function-call outputs survive verbatim.
        assert_eq!(parsed.input_items[0]["call_id"], "call_sanitized_1");
    }

    #[test]
    fn responses_request_options_are_patched_into_wire_body() {
        let request = request_with_history().with_responses_options(ResponsesRequestOptions {
            parallel_tool_calls: Some(false),
            ..Default::default()
        });
        let (body, _) = create_response(&request, None, None, false).unwrap();
        assert_eq!(body["parallel_tool_calls"], false);
    }

    #[test]
    fn responses_options_are_not_serialized_as_anthropic_fields() {
        let request = request_with_history().with_responses_options(ResponsesRequestOptions {
            user: Some("user-1".into()),
            ..Default::default()
        });
        let serialized = serde_json::to_value(&request).unwrap();
        assert!(serialized.get("responses_options").is_none());
    }

    #[test]
    fn state_with_mismatched_model_is_allowed_for_experiment() {
        let request = request_with_history();
        let mut state = state_covering_first_message(&request);
        state.model = "other-model".to_string();
        create_response(
            &request,
            Some(&crate::ProviderConversationState::OpenAiResponses(state)),
            None,
            false,
        )
        .expect("model mismatch should not be rejected by local state validation");
    }

    #[test]
    fn state_with_mismatched_logical_hash_is_rejected() {
        let request = request_with_history();
        let mut state = state_covering_first_message(&request);
        state.logical_context_hash = "deadbeef".repeat(8);
        let error = create_response(
            &request,
            Some(&crate::ProviderConversationState::OpenAiResponses(state)),
            None,
            false,
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("hash"));
    }

    #[test]
    fn state_covering_more_messages_than_request_is_rejected() {
        let request = request_with_history();
        let mut state = state_covering_first_message(&request);
        state.logical_message_count = request.messages.len() + 1;
        assert!(
            create_response(
                &request,
                Some(&crate::ProviderConversationState::OpenAiResponses(state)),
                None,
                false
            )
            .is_err()
        );
    }

    #[test]
    fn state_with_wrong_provider_variant_is_rejected() {
        let request = request_with_history();
        let mut state = state_covering_first_message(&request);
        state.provider = "anthropic".to_string();
        assert!(
            create_response(
                &request,
                Some(&crate::ProviderConversationState::OpenAiResponses(state)),
                None,
                false
            )
            .is_err()
        );
    }

    #[test]
    fn state_with_unknown_version_is_rejected() {
        let request = request_with_history();
        let mut state = state_covering_first_message(&request);
        state.version = 2;
        assert!(
            create_response(
                &request,
                Some(&crate::ProviderConversationState::OpenAiResponses(state)),
                None,
                false,
            )
            .is_err()
        );
    }

    // ── Native web search injection ─────────────────────────────────

    #[test]
    fn native_web_search_injects_hosted_tool() {
        let request = CreateMessageParams::new(RequiredMessageParams {
            model: "gpt-5".to_string(),
            messages: vec![Message::new_text(Role::User, "hello")],
            max_tokens: 256,
        });
        // No function tools — only native web search.
        let (body, _) = create_response(&request, None, None, true).unwrap();
        let tools = body["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0]["type"], "web_search");
    }

    #[test]
    fn native_web_search_off_omits_hosted_tool() {
        let request = CreateMessageParams::new(RequiredMessageParams {
            model: "gpt-5".to_string(),
            messages: vec![Message::new_text(Role::User, "hello")],
            max_tokens: 256,
        });
        let (body, _) = create_response(&request, None, None, false).unwrap();
        assert!(body.get("tools").is_none());
    }

    #[test]
    fn native_web_search_coexists_with_function_tools() {
        let request = CreateMessageParams::new(RequiredMessageParams {
            model: "gpt-5".to_string(),
            messages: vec![Message::new_text(Role::User, "run pwd")],
            max_tokens: 128,
        })
        .with_tools(vec![Tool {
            name: "bash".to_string(),
            description: Some("Run a command".to_string()),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {"cmd": {"type": "string"}}
            }),
        }]);

        let (body, _) = create_response(&request, None, None, true).unwrap();
        let tools = body["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 2);
        let types: Vec<&str> = tools.iter().map(|t| t["type"].as_str().unwrap()).collect();
        assert!(types.contains(&"web_search"));
        assert!(types.contains(&"function"));
    }

    #[test]
    fn native_web_search_does_not_replace_mcp_web_search() {
        // An MCP-provided `web_search` function tool must stay as a
        // function — native web search is an ADDITIONAL hosted tool.
        let request = CreateMessageParams::new(RequiredMessageParams {
            model: "gpt-5".to_string(),
            messages: vec![Message::new_text(Role::User, "search for AI news")],
            max_tokens: 256,
        })
        .with_tools(vec![Tool {
            name: "web_search".to_string(),
            description: Some("Search the web via MCP server".to_string()),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {"query": {"type": "string"}}
            }),
        }]);

        let (body, _) = create_response(&request, None, None, true).unwrap();
        let tools = body["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 2);
        // Both exist: the MCP web_search as a function, plus native hosted tool.
        assert!(tools.iter().any(|t| t["type"] == "web_search"));
        assert!(
            tools
                .iter()
                .any(|t| t["type"] == "function" && t["name"] == "web_search")
        );
    }
}
