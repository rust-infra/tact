//! Conversion helpers between Anthropic and OpenAI types.

use anthropic_ai_sdk::types::message::{
    ContentBlock, Message, MessageContent, Role, StopReason, Tool,
};
use async_openai::types::{
    ChatCompletionMessageToolCall, ChatCompletionRequestAssistantMessage,
    ChatCompletionRequestMessage, ChatCompletionRequestMessageContentPart,
    ChatCompletionRequestMessageContentPartImage, ChatCompletionRequestMessageContentPartText,
    ChatCompletionRequestSystemMessage, ChatCompletionRequestToolMessage,
    ChatCompletionRequestUserMessage, ChatCompletionRequestUserMessageContent,
    ChatCompletionTool, ChatCompletionToolType, CreateChatCompletionRequest, FinishReason,
    FunctionObject, ImageUrl, ImageUrlDetail, Role as OpenAiRole,
};

/// Convert Anthropic tool definitions to OpenAI tool definitions.
pub fn anthropic_tools_to_openai(tools: &[Tool]) -> Vec<ChatCompletionTool> {
    tools
        .iter()
        .map(|tool| ChatCompletionTool {
            r#type: ChatCompletionToolType::Function,
            function: FunctionObject {
                name: tool.name.clone(),
                description: tool.description.clone(),
                parameters: Some(tool.input_schema.clone()),
            },
        })
        .collect()
}

/// Convert a list of Anthropic messages to OpenAI chat completion messages.
///
/// Returns a parallel vector of optional `reasoning_content` strings aligned with
/// each emitted OpenAI message (for Kimi/Moonshot replay).
#[allow(deprecated)]
pub fn anthropic_messages_to_openai(
    messages: &[Message],
) -> (Vec<ChatCompletionRequestMessage>, Vec<Option<String>>) {
    let mut result = Vec::new();
    let mut reasoning = Vec::new();
    for msg in messages {
        match msg.role {
            Role::User => match &msg.content {
                MessageContent::Text { content } => {
                    result.push(ChatCompletionRequestMessage::User(
                        ChatCompletionRequestUserMessage {
                            content: ChatCompletionRequestUserMessageContent::Text(content.clone()),
                            role: OpenAiRole::User,
                            name: None,
                        },
                    ));
                    reasoning.push(None);
                }
                MessageContent::Blocks { content } => {
                    let mut parts: Vec<ChatCompletionRequestMessageContentPart> = Vec::new();
                    let mut tool_results: Vec<ChatCompletionRequestMessage> = Vec::new();
                    for block in content {
                        match block {
                            ContentBlock::Text { text } => {
                                parts.push(ChatCompletionRequestMessageContentPart::Text(
                                    ChatCompletionRequestMessageContentPartText {
                                        r#type: "text".to_string(),
                                        text: text.clone(),
                                    },
                                ));
                            }
                            ContentBlock::Image { source } => {
                                let data_url = format!(
                                    "data:{};base64,{}",
                                    source.media_type, source.data
                                );
                                parts.push(ChatCompletionRequestMessageContentPart::Image(
                                    ChatCompletionRequestMessageContentPartImage {
                                        r#type: "image_url".to_string(),
                                        image_url: ImageUrl {
                                            url: data_url,
                                            detail: ImageUrlDetail::Auto,
                                        },
                                    },
                                ));
                            }
                            ContentBlock::ToolResult {
                                tool_use_id,
                                content,
                            } => {
                                tool_results.push(ChatCompletionRequestMessage::Tool(
                                    ChatCompletionRequestToolMessage {
                                        role: OpenAiRole::Tool,
                                        content: content.clone(),
                                        tool_call_id: tool_use_id.clone(),
                                    },
                                ));
                            }
                            // Drop thinking on the user side for now.
                            _ => {}
                        }
                    }
                    if !parts.is_empty() {
                        result.push(ChatCompletionRequestMessage::User(
                            ChatCompletionRequestUserMessage {
                                content: ChatCompletionRequestUserMessageContent::Array(parts),
                                role: OpenAiRole::User,
                                name: None,
                            },
                        ));
                        reasoning.push(None);
                    }
                    for tool_result in tool_results {
                        result.push(tool_result);
                        reasoning.push(None);
                    }
                }
            },
            Role::Assistant => match &msg.content {
                MessageContent::Text { content } => {
                    result.push(ChatCompletionRequestMessage::Assistant(
                        ChatCompletionRequestAssistantMessage {
                            role: OpenAiRole::Assistant,
                            content: Some(content.clone()),
                            name: None,
                            tool_calls: None,
                            function_call: None,
                        },
                    ));
                    reasoning.push(None);
                }
                MessageContent::Blocks { content } => {
                    let assistant_reasoning = content
                        .iter()
                        .filter_map(|block| match block {
                            ContentBlock::Thinking { thinking, .. } => Some(thinking.as_str()),
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join("");
                    let mut text_parts = Vec::new();
                    let mut tool_calls = Vec::new();
                    for block in content {
                        match block {
                            ContentBlock::Text { text } => {
                                text_parts.push(text.as_str());
                            }
                            ContentBlock::ToolUse { id, name, input } => {
                                tool_calls.push(ChatCompletionMessageToolCall {
                                    id: id.clone(),
                                    r#type: ChatCompletionToolType::Function,
                                    function: async_openai::types::FunctionCall {
                                        name: name.clone(),
                                        arguments: input.to_string(),
                                    },
                                });
                            }
                            // Drop thinking blocks when sending to OpenAI.
                            _ => {}
                        }
                    }
                    let content = if text_parts.is_empty() {
                        None
                    } else {
                        Some(text_parts.join(""))
                    };
                    result.push(ChatCompletionRequestMessage::Assistant(
                        ChatCompletionRequestAssistantMessage {
                            role: OpenAiRole::Assistant,
                            content,
                            name: None,
                            tool_calls: if tool_calls.is_empty() {
                                None
                            } else {
                                Some(tool_calls)
                            },
                            function_call: None,
                        },
                    ));
                    reasoning.push(if assistant_reasoning.is_empty() {
                        None
                    } else {
                        Some(assistant_reasoning)
                    });
                }
            },
        }
    }
    // Defensive: strip orphaned tool_calls that aren't followed by tool messages.
    sanitize_tool_call_sequence(&mut result);
    (result, reasoning)
}

/// Defensive validation: OpenAI requires every assistant message with
/// `tool_calls` to be immediately followed by `ToolMessage` entries for
/// every `tool_call_id`. If this invariant is violated (e.g., after a
/// MaxTokens continuation that didn't execute tools), strip the orphaned
/// `tool_calls` so the request doesn't get rejected by the API.
fn sanitize_tool_call_sequence(messages: &mut Vec<ChatCompletionRequestMessage>) {
    let mut i = 0;
    while i < messages.len() {
        // Collect tool_call_ids from an assistant message.
        let tool_call_ids: Vec<String> = match &messages[i] {
            ChatCompletionRequestMessage::Assistant(assistant)
                if assistant.tool_calls.is_some() =>
            {
                assistant
                    .tool_calls
                    .as_ref()
                    .unwrap()
                    .iter()
                    .map(|tc| tc.id.clone())
                    .filter(|id| !id.is_empty())
                    .collect()
            }
            _ => {
                i += 1;
                continue;
            }
        };

        if tool_call_ids.is_empty() {
            i += 1;
            continue;
        }

        // Count how many of these IDs find a matching tool message
        // in the immediately following positions.
        let mut matched = 0;
        let mut j = i + 1;
        while j < messages.len() {
            match &messages[j] {
                ChatCompletionRequestMessage::Tool(tm) => {
                    if tool_call_ids.contains(&tm.tool_call_id) {
                        matched += 1;
                    }
                    j += 1;
                }
                // Stop scanning when we hit a non-tool message.
                _ => break,
            }
        }

        if matched < tool_call_ids.len() {
            // Some tool calls are orphaned. Drop *all* tool_calls from this
            // assistant message to keep the API happy. The model will have
            // another chance to request the tools in the next turn.
            if let ChatCompletionRequestMessage::Assistant(ref mut assistant) = messages[i] {
                tracing::warn!(
                    orphaned_tool_calls = ?tool_call_ids,
                    matched_tool_messages = matched,
                    "Stripping orphaned tool_calls from assistant message \
                     (context may have been compacted or continued past MaxTokens)."
                );
                assistant.tool_calls = None;
                // If there was no text content either, replace the assistant
                // message with a short stub so the message isn't empty.
                if assistant.content.as_deref().unwrap_or("").is_empty() {
                    assistant.content = Some(
                        "[Tool calls were pending but context was truncated before \
                         their results arrived. Please re-issue if needed.]"
                            .to_string(),
                    );
                }
            }
        }

        i += 1;
    }
}

/// Build an OpenAI `CreateChatCompletionRequest` from Anthropic `CreateMessageParams`.
///
/// Also returns a parallel vector of optional `reasoning_content` strings, one for each
/// message in the generated OpenAI message list. This is needed for providers such as
/// Kimi/Moonshot that require historical `reasoning_content` to be echoed back on
/// assistant messages (especially assistant messages containing `tool_calls`).
#[allow(deprecated)]
pub fn build_openai_request(
    request: &anthropic_ai_sdk::types::message::CreateMessageParams,
) -> (CreateChatCompletionRequest, Vec<Option<String>>) {
    let mut messages = Vec::new();
    let mut reasoning_per_message = Vec::new();

    // Anthropic sends system as a top-level field; OpenAI expects it as the first system message.
    if let Some(system) = &request.system {
        messages.push(ChatCompletionRequestMessage::System(
            ChatCompletionRequestSystemMessage {
                content: system.clone(),
                role: OpenAiRole::System,
                name: None,
            },
        ));
        reasoning_per_message.push(None);
    }

    let (converted_messages, converted_reasoning) =
        anthropic_messages_to_openai(&request.messages);
    messages.extend(converted_messages);
    reasoning_per_message.extend(converted_reasoning);

    let mut openai_request = CreateChatCompletionRequest {
        model: request.model.clone(),
        messages,
        frequency_penalty: None,
        logit_bias: None,
        logprobs: None,
        top_logprobs: None,
        max_tokens: Some(request.max_tokens.min(u16::MAX as u32) as u16),
        n: Some(1),
        presence_penalty: None,
        response_format: None,
        seed: None,
        stop: None,
        stream: Some(true),
        temperature: request.temperature,
        top_p: request.top_p,
        tools: request.tools.as_ref().map(|t| anthropic_tools_to_openai(t)),
        tool_choice: None,
        user: None,
        function_call: None,
        functions: None,
    };

    // Map Anthropic tool_choice to OpenAI if present.
    if let Some(tc) = &request.tool_choice {
        openai_request.tool_choice = Some(map_tool_choice(tc));
    }

    (openai_request, reasoning_per_message)
}

fn map_tool_choice(
    tc: &anthropic_ai_sdk::types::message::ToolChoice,
) -> async_openai::types::ChatCompletionToolChoiceOption {
    use anthropic_ai_sdk::types::message::ToolChoice as Atc;
    use async_openai::types::ChatCompletionToolChoiceOption as Otco;
    match tc {
        Atc::Auto => Otco::Auto,
        Atc::Any => Otco::Auto, // OpenAI has no exact "any" equivalent; auto is closest.
        Atc::None => Otco::None,
        Atc::Tool { name } => Otco::Named(async_openai::types::ChatCompletionNamedToolChoice {
            r#type: ChatCompletionToolType::Function,
            function: async_openai::types::FunctionName { name: name.clone() },
        }),
    }
}

/// Convert OpenAI `FinishReason` to Anthropic `StopReason`.
pub fn finish_reason_to_stop_reason(reason: Option<FinishReason>) -> Option<StopReason> {
    match reason {
        Some(FinishReason::Stop) => Some(StopReason::EndTurn),
        Some(FinishReason::Length) => Some(StopReason::MaxTokens),
        Some(FinishReason::ToolCalls) => Some(StopReason::ToolUse),
        Some(FinishReason::ContentFilter) => Some(StopReason::StopSequence),
        Some(FinishReason::FunctionCall) => Some(StopReason::ToolUse),
        None => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anthropic_ai_sdk::types::message::{ContentBlock, ImageSource, Message, Role};

    #[test]
    fn convert_user_message_with_image_to_openai_content_parts() {
        let msg = Message::new_blocks(
            Role::User,
            vec![
                ContentBlock::Text {
                    text: "describe this".to_string(),
                },
                ContentBlock::Image {
                    source: ImageSource {
                        type_: "base64".to_string(),
                        media_type: "image/png".to_string(),
                        data: "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+ip1sAAAAASUVORK5CYII=".to_string(),
                    },
                },
            ],
        );

        let (openai, _) = anthropic_messages_to_openai(&[msg]);
        assert_eq!(openai.len(), 1);
        let ChatCompletionRequestMessage::User(user) = &openai[0] else {
            panic!("expected user message");
        };
        let ChatCompletionRequestUserMessageContent::Array(parts) = &user.content else {
            panic!("expected array content");
        };
        assert_eq!(parts.len(), 2);
        assert!(
            matches!(&parts[0], ChatCompletionRequestMessageContentPart::Text(t) if t.text == "describe this")
        );
        assert!(
            matches!(&parts[1], ChatCompletionRequestMessageContentPart::Image(img) if img.image_url.url.starts_with("data:image/png;base64,"))
        );
    }

    #[test]
    fn reasoning_aligns_when_user_tool_results_split_into_multiple_messages() {
        let assistant = Message::new_blocks(
            Role::Assistant,
            vec![
                ContentBlock::Thinking {
                    thinking: "plan both tools".to_string(),
                    signature: String::new(),
                },
                ContentBlock::ToolUse {
                    id: "call_a".to_string(),
                    name: "read_file".to_string(),
                    input: serde_json::json!({"path": "a.rs"}),
                },
                ContentBlock::ToolUse {
                    id: "call_b".to_string(),
                    name: "read_file".to_string(),
                    input: serde_json::json!({"path": "b.rs"}),
                },
            ],
        );
        let user = Message::new_blocks(
            Role::User,
            vec![
                ContentBlock::ToolResult {
                    tool_use_id: "call_a".to_string(),
                    content: "a".to_string(),
                },
                ContentBlock::ToolResult {
                    tool_use_id: "call_b".to_string(),
                    content: "b".to_string(),
                },
            ],
        );

        let request = anthropic_ai_sdk::types::message::CreateMessageParams::new(
            anthropic_ai_sdk::types::message::RequiredMessageParams {
                model: "kimi-k2.5".to_string(),
                messages: vec![assistant.clone(), user],
                max_tokens: 1024,
            },
        );

        let (openai_request, reasoning) = build_openai_request(&request);
        assert_eq!(openai_request.messages.len(), reasoning.len());

        let assistant_idx = openai_request
            .messages
            .iter()
            .position(|msg| matches!(msg, ChatCompletionRequestMessage::Assistant(_)))
            .expect("assistant message");
        assert_eq!(reasoning[assistant_idx].as_deref(), Some("plan both tools"));

        let tool_count = openai_request
            .messages
            .iter()
            .filter(|msg| matches!(msg, ChatCompletionRequestMessage::Tool(_)))
            .count();
        assert_eq!(tool_count, 2);
    }
}
