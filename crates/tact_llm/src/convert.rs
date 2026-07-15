//! Conversion helpers between Anthropic and OpenAI types.

use crate::{
    ContentBlock, CreateMessageParams, Message, MessageContent, Role, StopReason, Tool, ToolChoice,
    openai::CreateChatCompletionRequest,
};
use async_openai::types::{
    ChatCompletionMessageToolCall, ChatCompletionRequestAssistantMessage,
    ChatCompletionRequestMessage, ChatCompletionRequestMessageContentPart,
    ChatCompletionRequestMessageContentPartImage, ChatCompletionRequestMessageContentPartText,
    ChatCompletionRequestSystemMessage, ChatCompletionRequestToolMessage,
    ChatCompletionRequestUserMessage, ChatCompletionRequestUserMessageContent, ChatCompletionTool,
    ChatCompletionToolType, FinishReason, FunctionObject, ImageUrl, ImageUrlDetail,
    Role as OpenAiRole,
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
                                let data_url =
                                    format!("data:{};base64,{}", source.media_type, source.data);
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
                            // REVIEW: For non-Kimi providers this can leave the
                            // assistant message with no content and no tool_calls,
                            // which violates OpenAI's message schema. We currently
                            // rely on sanitize_assistant_messages to stub such
                            // messages; reconsider if the upstream format changes.
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
    // Defensive: strip orphaned tool_calls that aren't followed by tool messages
    // and ensure no assistant message is completely empty (OpenAI rejects empty
    // assistant messages, which can happen after MaxTokens when thinking blocks
    // are dropped or a tool-call was truncated before any text was emitted).
    sanitize_assistant_messages(&mut result, &mut reasoning);
    (result, reasoning)
}

/// Defensive validation of assistant messages for OpenAI-compatible APIs.
///
/// 1. Every assistant message with `tool_calls` must be immediately followed by
///    matching `ToolMessage` entries. If not, strip the orphaned `tool_calls`
///    **and** remove the consecutive following `ToolMessage`s (leaving them
///    would produce orphan tool results that OpenAI rejects with 400).
/// 2. Assistant messages cannot have empty `content` and no `tool_calls`. If a
///    message ends up empty (e.g. thinking block was dropped, or truncation
///    happened before any tokens), insert a short stub.
///
/// REVIEW: This is a workaround for 400 Bad Request errors observed after
/// MaxTokens recovery. The real fix may be to avoid producing empty assistant
/// messages in the agent runtime / conversion pipeline rather than patching
/// them after the fact.
fn sanitize_assistant_messages(
    messages: &mut Vec<ChatCompletionRequestMessage>,
    reasoning: &mut Vec<Option<String>>,
) {
    debug_assert_eq!(messages.len(), reasoning.len());
    let mut i = 0;
    while i < messages.len() {
        let ChatCompletionRequestMessage::Assistant(assistant) = &messages[i] else {
            i += 1;
            continue;
        };

        let has_tool_calls = assistant
            .tool_calls
            .as_ref()
            .is_some_and(|tcs| !tcs.is_empty());

        if has_tool_calls {
            // Count how many of the assistant's tool_call_ids find a matching
            // tool message in the immediately following positions.
            let tool_call_ids: Vec<String> = assistant
                .tool_calls
                .as_ref()
                .unwrap()
                .iter()
                .map(|tc| tc.id.clone())
                .filter(|id| !id.is_empty())
                .collect();

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
                // Incomplete tool results. Drop *all* tool_calls from this
                // assistant and remove the consecutive ToolMessages that would
                // otherwise become orphans without a parent tool_calls list.
                if let ChatCompletionRequestMessage::Assistant(ref mut assistant) = messages[i] {
                    tracing::warn!(
                        orphaned_tool_calls = ?tool_call_ids,
                        matched_tool_messages = matched,
                        "Stripping orphaned tool_calls and following tool messages \
                         (context may have been compacted or continued past MaxTokens)."
                    );
                    assistant.tool_calls = None;
                }
                let remove_end = j;
                let remove_start = i + 1;
                if remove_start < remove_end {
                    messages.drain(remove_start..remove_end);
                    reasoning.drain(remove_start..remove_end);
                }
            }
        }

        // Ensure the assistant message is not empty. This can happen when the
        // only content was a thinking block that gets dropped for non-Kimi
        // providers, or when the response was truncated before emitting text.
        if let ChatCompletionRequestMessage::Assistant(ref mut assistant) = messages[i] {
            let has_tool_calls_now = assistant
                .tool_calls
                .as_ref()
                .is_some_and(|tcs| !tcs.is_empty());
            if !has_tool_calls_now && assistant.content.as_deref().unwrap_or("").is_empty() {
                tracing::warn!(
                    "Replacing empty assistant message with stub to keep OpenAI request valid"
                );
                assistant.content =
                    Some("[Assistant response was empty or truncated. Continuing...]".to_string());
            }
        }

        i += 1;
    }
}

/// Build an OpenAI `CreateChatCompletionRequest` from Tact [`CreateMessageParams`].
///
/// Also returns a parallel vector of optional `reasoning_content` strings, one for each
/// message in the generated OpenAI message list. This is needed for providers such as
/// Kimi/Moonshot that require historical `reasoning_content` to be echoed back on
/// assistant messages (especially assistant messages containing `tool_calls`).
#[allow(deprecated)]
pub fn build_openai_request(
    request: &CreateMessageParams,
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

    let (converted_messages, converted_reasoning) = anthropic_messages_to_openai(&request.messages);
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
        stream_options: None,
        temperature: request.temperature,
        top_p: request.top_p,
        tools: request.tools.as_ref().map(|t| anthropic_tools_to_openai(t)),
        tool_choice: None,
        user: None,
        user_id: None,
        thinking: None,
        reasoning_effort: None,
    };

    // Map Anthropic tool_choice to OpenAI if present.
    if let Some(tc) = &request.tool_choice {
        openai_request.tool_choice = Some(map_tool_choice(tc));
    }

    (openai_request, reasoning_per_message)
}

fn map_tool_choice(tc: &ToolChoice) -> async_openai::types::ChatCompletionToolChoiceOption {
    use async_openai::types::ChatCompletionToolChoiceOption as Otco;
    match tc {
        ToolChoice::Auto => Otco::Auto,
        ToolChoice::Any => Otco::Auto, // OpenAI has no exact "any" equivalent; auto is closest.
        ToolChoice::None => Otco::None,
        ToolChoice::Tool { name } => {
            Otco::Named(async_openai::types::ChatCompletionNamedToolChoice {
                r#type: ChatCompletionToolType::Function,
                function: async_openai::types::FunctionName { name: name.clone() },
            })
        }
    }
}

/// Convert OpenAI typed `FinishReason` into Tact [`StopReason`].
pub fn finish_reason_to_stop_reason(reason: Option<FinishReason>) -> Option<StopReason> {
    match reason {
        Some(FinishReason::Stop) => Some(StopReason::EndTurn),
        Some(FinishReason::Length) => Some(StopReason::MaxTokens),
        Some(FinishReason::ToolCalls) | Some(FinishReason::FunctionCall) => {
            Some(StopReason::ToolUse)
        }
        Some(FinishReason::ContentFilter) => Some(StopReason::StopSequence),
        None => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ContentBlock, ImageSource, Message, RequiredMessageParams, Role};

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
    fn empty_assistant_message_gets_stubbed() {
        let assistant = Message::new_blocks(Role::Assistant, vec![]);
        let request = CreateMessageParams::new(RequiredMessageParams {
            model: "mock".to_string(),
            messages: vec![assistant],
            max_tokens: 1024,
        });

        let (openai_request, _) = build_openai_request(&request);
        let assistant_msg = openai_request
            .messages
            .iter()
            .find_map(|m| match m {
                ChatCompletionRequestMessage::Assistant(a) => Some(a),
                _ => None,
            })
            .expect("assistant message");
        assert!(
            assistant_msg
                .content
                .as_deref()
                .is_some_and(|c| !c.is_empty()),
            "empty assistant message should be replaced with a stub"
        );
        assert!(assistant_msg.tool_calls.is_none());
    }

    #[test]
    fn orphaned_tool_calls_are_stripped() {
        let assistant = Message::new_blocks(
            Role::Assistant,
            vec![ContentBlock::ToolUse {
                id: "orphan".to_string(),
                name: "read_file".to_string(),
                input: serde_json::json!({"path": "x.rs"}),
            }],
        );
        let user = Message::new_text(Role::User, "keep going".to_string());
        let request = CreateMessageParams::new(RequiredMessageParams {
            model: "mock".to_string(),
            messages: vec![assistant, user],
            max_tokens: 1024,
        });

        let (openai_request, _) = build_openai_request(&request);
        let assistant_msg = openai_request
            .messages
            .iter()
            .find_map(|m| match m {
                ChatCompletionRequestMessage::Assistant(a) => Some(a),
                _ => None,
            })
            .expect("assistant message");
        assert!(assistant_msg.tool_calls.is_none());
        assert!(
            assistant_msg
                .content
                .as_deref()
                .is_some_and(|c| !c.is_empty()),
            "orphaned tool_calls should be stripped and content stubbed"
        );
    }

    #[test]
    fn thinking_only_assistant_gets_stubbed() {
        let assistant = Message::new_blocks(
            Role::Assistant,
            vec![ContentBlock::Thinking {
                thinking: "partial reasoning".to_string(),
                signature: String::new(),
            }],
        );
        let request = CreateMessageParams::new(RequiredMessageParams {
            model: "mock".to_string(),
            messages: vec![assistant],
            max_tokens: 1024,
        });

        let (openai_request, _) = build_openai_request(&request);
        let assistant_msg = openai_request
            .messages
            .iter()
            .find_map(|m| match m {
                ChatCompletionRequestMessage::Assistant(a) => Some(a),
                _ => None,
            })
            .expect("assistant message");
        assert!(
            assistant_msg
                .content
                .as_deref()
                .is_some_and(|c| !c.is_empty()),
            "thinking-only assistant should be stubbed after thinking is dropped"
        );
        assert!(assistant_msg.tool_calls.is_none());
    }

    #[test]
    fn assistant_with_text_and_thinking_keeps_text() {
        let assistant = Message::new_blocks(
            Role::Assistant,
            vec![
                ContentBlock::Thinking {
                    thinking: "reasoning".to_string(),
                    signature: String::new(),
                },
                ContentBlock::Text {
                    text: "actual answer".to_string(),
                },
            ],
        );
        let request = CreateMessageParams::new(RequiredMessageParams {
            model: "mock".to_string(),
            messages: vec![assistant],
            max_tokens: 1024,
        });

        let (openai_request, _) = build_openai_request(&request);
        let assistant_msg = openai_request
            .messages
            .iter()
            .find_map(|m| match m {
                ChatCompletionRequestMessage::Assistant(a) => Some(a),
                _ => None,
            })
            .expect("assistant message");
        assert_eq!(assistant_msg.content.as_deref(), Some("actual answer"));
        assert!(assistant_msg.tool_calls.is_none());
    }

    #[test]
    fn orphaned_tool_call_with_existing_text_keeps_text_and_drops_tool_calls() {
        let assistant = Message::new_blocks(
            Role::Assistant,
            vec![
                ContentBlock::Text {
                    text: "let me check".to_string(),
                },
                ContentBlock::ToolUse {
                    id: "orphan".to_string(),
                    name: "read_file".to_string(),
                    input: serde_json::json!({"path": "x.rs"}),
                },
            ],
        );
        let user = Message::new_text(Role::User, "keep going".to_string());
        let request = CreateMessageParams::new(RequiredMessageParams {
            model: "mock".to_string(),
            messages: vec![assistant, user],
            max_tokens: 1024,
        });

        let (openai_request, _) = build_openai_request(&request);
        let assistant_msg = openai_request
            .messages
            .iter()
            .find_map(|m| match m {
                ChatCompletionRequestMessage::Assistant(a) => Some(a),
                _ => None,
            })
            .expect("assistant message");
        assert!(assistant_msg.tool_calls.is_none());
        assert_eq!(assistant_msg.content.as_deref(), Some("let me check"));
    }

    #[test]
    fn valid_tool_call_sequence_is_preserved() {
        let assistant = Message::new_blocks(
            Role::Assistant,
            vec![ContentBlock::ToolUse {
                id: "call_1".to_string(),
                name: "read_file".to_string(),
                input: serde_json::json!({"path": "x.rs"}),
            }],
        );
        let user = Message::new_blocks(
            Role::User,
            vec![ContentBlock::ToolResult {
                tool_use_id: "call_1".to_string(),
                content: "file content".to_string(),
            }],
        );
        let request = CreateMessageParams::new(RequiredMessageParams {
            model: "mock".to_string(),
            messages: vec![assistant, user],
            max_tokens: 1024,
        });

        let (openai_request, _) = build_openai_request(&request);
        let assistant_msg = openai_request
            .messages
            .iter()
            .find_map(|m| match m {
                ChatCompletionRequestMessage::Assistant(a) => Some(a),
                _ => None,
            })
            .expect("assistant message");
        assert!(assistant_msg.tool_calls.is_some());
        assert_eq!(assistant_msg.tool_calls.as_ref().unwrap().len(), 1);
        assert!(
            openai_request
                .messages
                .iter()
                .any(|m| matches!(m, ChatCompletionRequestMessage::Tool(_))),
            "matching tool result should be kept"
        );
    }

    #[test]
    fn partial_tool_results_strip_tool_calls_and_orphan_tool_messages() {
        // Assistant requested two tools, but only one result is present (e.g. after
        // compaction). Stripping tool_calls alone would leave an orphan ToolMessage.
        let assistant = Message::new_blocks(
            Role::Assistant,
            vec![
                ContentBlock::ToolUse {
                    id: "call_1".to_string(),
                    name: "read_file".to_string(),
                    input: serde_json::json!({"path": "a.rs"}),
                },
                ContentBlock::ToolUse {
                    id: "call_2".to_string(),
                    name: "read_file".to_string(),
                    input: serde_json::json!({"path": "b.rs"}),
                },
            ],
        );
        let user = Message::new_blocks(
            Role::User,
            vec![ContentBlock::ToolResult {
                tool_use_id: "call_1".to_string(),
                content: "partial".to_string(),
            }],
        );
        let follow_up = Message::new_text(Role::User, "continue".to_string());
        let request = CreateMessageParams::new(RequiredMessageParams {
            model: "mock".to_string(),
            messages: vec![assistant, user, follow_up],
            max_tokens: 1024,
        });

        let (openai_request, reasoning) = build_openai_request(&request);
        assert_eq!(
            openai_request.messages.len(),
            reasoning.len(),
            "reasoning must stay aligned after sanitize removals"
        );
        let assistant_msg = openai_request
            .messages
            .iter()
            .find_map(|m| match m {
                ChatCompletionRequestMessage::Assistant(a) => Some(a),
                _ => None,
            })
            .expect("assistant message");
        assert!(assistant_msg.tool_calls.is_none());
        assert!(
            openai_request
                .messages
                .iter()
                .all(|m| !matches!(m, ChatCompletionRequestMessage::Tool(_))),
            "orphan tool messages must be removed with stripped tool_calls"
        );
        assert!(
            openai_request.messages.iter().any(|m| matches!(
                m,
                ChatCompletionRequestMessage::User(u)
                    if matches!(
                        &u.content,
                        ChatCompletionRequestUserMessageContent::Text(t) if t == "continue"
                    )
            )),
            "subsequent user messages must be preserved"
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

        let request = CreateMessageParams::new(RequiredMessageParams {
            model: "kimi-k2.5".to_string(),
            messages: vec![assistant.clone(), user],
            max_tokens: 1024,
        });

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
