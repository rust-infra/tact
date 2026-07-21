use async_openai_responses::types::responses::{
    CreateResponseArgs, EasyInputContent, EasyInputMessage, FunctionCallOutput,
    FunctionCallOutputItemParam, FunctionTool, FunctionToolCall, IncludeEnum, InputContent,
    InputImageContent, InputItem, InputParam, InputTextContent, Item, MessageType, OutputStatus,
    Reasoning, ReasoningSummary, Role as ResponsesRole, Tool as ResponsesTool, ToolChoiceFunction,
    ToolChoiceOptions, ToolChoiceParam,
};

use crate::{
    ContentBlock, CreateMessageParams, LlmError, Message, MessageContent, OpenAiReasoningEffort,
    Role, ToolChoice, effective_reasoning_effort,
};

use super::history;

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
    let Message { role, content } = message;
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
                    LlmError::Other(format!("serialize arguments for tool '{name}': {error}"))
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

fn normalize_assistant_history_items(body: &mut serde_json::Value) {
    let Some(input) = body
        .get_mut("input")
        .and_then(serde_json::Value::as_array_mut)
    else {
        return;
    };

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

pub(crate) fn create_response(
    request: &CreateMessageParams,
    configured_effort: Option<OpenAiReasoningEffort>,
) -> Result<serde_json::Value, LlmError> {
    let mut input = Vec::new();
    for message in &request.messages {
        input.extend(message_to_input(message)?);
    }

    let mut builder = CreateResponseArgs::default();
    builder
        .model(request.model.clone())
        .input(InputParam::Items(input))
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
    if let Some(tools) = &request.tools {
        builder.tools(
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
                .collect::<Vec<_>>(),
        );
    }
    if let Some(choice) = &request.tool_choice {
        builder.tool_choice(tool_choice(choice));
    } else if request
        .tools
        .as_ref()
        .is_some_and(|tools| !tools.is_empty())
    {
        builder.tool_choice(ToolChoiceOptions::Auto);
    }
    if request.thinking.is_some() || configured_effort.is_some() {
        builder.reasoning(Reasoning {
            effort: None,
            summary: Some(ReasoningSummary::Auto),
        });
    }

    let typed_request = builder
        .build()
        .map_err(|error| LlmError::Other(format!("build OpenAI Responses request: {error}")))?;
    let mut body = serde_json::to_value(typed_request)
        .map_err(|error| LlmError::Other(format!("serialize OpenAI Responses request: {error}")))?;
    normalize_assistant_history_items(&mut body);
    let budget_tokens = request
        .thinking
        .as_ref()
        .map_or(0, |thinking| thinking.budget_tokens);
    let effort = match configured_effort {
        Some(effort) => Some(effort),
        None => effective_reasoning_effort(None, budget_tokens),
    };
    if let Some(effort) = effort {
        body["reasoning"]["effort"] = serde_json::Value::String(effort.as_str().to_owned());
    }
    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::create_response;
    use crate::{
        ContentBlock, CreateMessageParams, ImageSource, Message, OpenAiReasoningEffort,
        RequiredMessageParams, Role, Thinking, ThinkingType, Tool, ToolChoice,
    };

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
        let body = create_response(&request_with_history(), None).unwrap();

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
        let body = create_response(&request_with_history(), None).unwrap();

        assert_eq!(body["include"][0], "reasoning.encrypted_content");
        assert_eq!(body["reasoning"]["summary"], "auto");
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

        let body = create_response(&request, None).unwrap();
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
            let body = create_response(&request, None).unwrap();
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

        let body = create_response(&request, None).unwrap();

        assert_eq!(body["tool_choice"], serde_json::json!("auto"));
    }

    #[test]
    fn serializes_explicit_max_reasoning_effort() {
        let body =
            create_response(&request_with_history(), Some(OpenAiReasoningEffort::Max)).unwrap();
        assert_eq!(body["reasoning"]["effort"], "max");
    }

    #[test]
    fn explicit_reasoning_effort_wins_over_budget_fallback() {
        let body =
            create_response(&request_with_history(), Some(OpenAiReasoningEffort::Low)).unwrap();
        assert_eq!(body["reasoning"]["effort"], "low");
    }

    #[test]
    fn serializes_assistant_history_as_completed_output_message() {
        let body = create_response(&request_with_history(), None).unwrap();
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
}
