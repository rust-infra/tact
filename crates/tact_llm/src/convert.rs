//! Conversion helpers between Tact types and OpenAI Chat Completions types.
//!
//! Prefer [`From`] / [`Into`] for 1:1 mappings. Multi-message expansions (e.g. a
//! user turn with embedded tool results) stay as free functions.

use async_openai::types::{
    ChatCompletionMessageToolCall, ChatCompletionNamedToolChoice, ChatCompletionRequestAssistantMessage,
    ChatCompletionRequestMessage, ChatCompletionRequestMessageContentPart,
    ChatCompletionRequestMessageContentPartImage, ChatCompletionRequestMessageContentPartText,
    ChatCompletionRequestSystemMessage, ChatCompletionRequestToolMessage, ChatCompletionRequestUserMessage,
    ChatCompletionRequestUserMessageContent, ChatCompletionTool, ChatCompletionToolChoiceOption,
    ChatCompletionToolType, FinishReason, FunctionCall, FunctionName, FunctionObject, ImageUrl, ImageUrlDetail,
    Role as OpenAiRole,
};

use crate::{
    ContentBlock, CreateMessageParams, ImageSource, Message, MessageContent, Role, StopReason, Tool, ToolChoice,
    openai::CreateChatCompletionRequest,
};

impl From<&Tool> for ChatCompletionTool {
    fn from(tool: &Tool) -> Self {
        Self {
            r#type: ChatCompletionToolType::Function,
            function: FunctionObject {
                name: tool.name.clone(),
                description: tool.description.clone(),
                parameters: Some(tool.input_schema.clone()),
            },
        }
    }
}

impl From<&ToolChoice> for ChatCompletionToolChoiceOption {
    fn from(tc: &ToolChoice) -> Self {
        match tc {
            // OpenAI has no exact "any" equivalent; auto is closest.
            ToolChoice::Auto | ToolChoice::Any => Self::Auto,
            ToolChoice::None => Self::None,
            ToolChoice::Tool { name } => Self::Named(ChatCompletionNamedToolChoice {
                r#type: ChatCompletionToolType::Function,
                function: FunctionName { name: name.clone() },
            }),
        }
    }
}

impl From<FinishReason> for StopReason {
    #[allow(deprecated)]
    fn from(reason: FinishReason) -> Self {
        match reason {
            FinishReason::Stop => Self::EndTurn,
            FinishReason::Length => Self::MaxTokens,
            FinishReason::ToolCalls | FinishReason::FunctionCall => Self::ToolUse,
            FinishReason::ContentFilter => Self::StopSequence,
        }
    }
}

impl From<&ImageSource> for ChatCompletionRequestMessageContentPart {
    fn from(source: &ImageSource) -> Self {
        Self::Image(ChatCompletionRequestMessageContentPartImage {
            r#type: "image_url".to_string(),
            image_url: ImageUrl {
                url: format!("data:{};base64,{}", source.media_type, source.data),
                detail: ImageUrlDetail::Auto,
            },
        })
    }
}

fn text_content_part(text: &str) -> ChatCompletionRequestMessageContentPart {
    ChatCompletionRequestMessageContentPart::Text(ChatCompletionRequestMessageContentPartText {
        r#type: "text".to_string(),
        text: text.to_owned(),
    })
}

fn user_text_message(content: impl Into<String>) -> ChatCompletionRequestMessage {
    ChatCompletionRequestMessage::User(ChatCompletionRequestUserMessage {
        content: ChatCompletionRequestUserMessageContent::Text(content.into()),
        role: OpenAiRole::User,
        name: None,
    })
}

fn user_parts_message(parts: Vec<ChatCompletionRequestMessageContentPart>) -> ChatCompletionRequestMessage {
    ChatCompletionRequestMessage::User(ChatCompletionRequestUserMessage {
        content: ChatCompletionRequestUserMessageContent::Array(parts),
        role: OpenAiRole::User,
        name: None,
    })
}

fn tool_result_message(tool_use_id: &str, content: &str) -> ChatCompletionRequestMessage {
    ChatCompletionRequestMessage::Tool(ChatCompletionRequestToolMessage {
        role: OpenAiRole::Tool,
        content: content.to_owned(),
        tool_call_id: tool_use_id.to_owned(),
    })
}

fn assistant_message(
    content: Option<String>,
    tool_calls: Option<Vec<ChatCompletionMessageToolCall>>,
) -> ChatCompletionRequestMessage {
    ChatCompletionRequestMessage::Assistant(ChatCompletionRequestAssistantMessage {
        role: OpenAiRole::Assistant,
        content,
        name: None,
        tool_calls,
        ..Default::default()
    })
}

fn tool_call_from_block(block: &ContentBlock) -> Option<ChatCompletionMessageToolCall> {
    let ContentBlock::ToolUse { id, name, input } = block else {
        return None;
    };
    Some(ChatCompletionMessageToolCall {
        id: id.clone(),
        r#type: ChatCompletionToolType::Function,
        function: FunctionCall { name: name.clone(), arguments: input.to_string() },
    })
}

/// Convert Tact messages (Anthropic Messages shape) to OpenAI chat completion messages.
///
/// Returns a parallel vector of optional `reasoning_content` strings aligned with
/// each emitted OpenAI message (for Kimi/Moonshot replay).
///
/// Not a [`From`] impl: one Tact message may expand into multiple OpenAI messages
/// (e.g. user blocks that mix text with `tool_result`).
pub fn messages_to_openai(messages: &[Message]) -> (Vec<ChatCompletionRequestMessage>, Vec<Option<String>>) {
    // Lower bound: one OpenAI message per Tact message (user turns with tool
    // results may emit more).
    let mut result = Vec::with_capacity(messages.len());
    let mut reasoning = Vec::with_capacity(messages.len());
    for msg in messages {
        match msg.role {
            Role::User => match &msg.content {
                MessageContent::Text { content } => {
                    result.push(user_text_message(content.clone()));
                    reasoning.push(None);
                },
                MessageContent::Blocks { content } => {
                    let mut parts = Vec::with_capacity(content.len());
                    let mut tool_results = Vec::new();
                    for block in content {
                        match block {
                            ContentBlock::Text { text } => parts.push(text_content_part(text)),
                            ContentBlock::Image { source } => parts.push(source.into()),
                            ContentBlock::ToolResult { tool_use_id, content } => {
                                tool_results.push(tool_result_message(tool_use_id, content))
                            },
                            // Drop thinking on the user side for now.
                            _ => {},
                        }
                    }
                    if !parts.is_empty() {
                        result.push(user_parts_message(parts));
                        reasoning.push(None);
                    }
                    reasoning.reserve(tool_results.len());
                    for tool_result in tool_results {
                        result.push(tool_result);
                        reasoning.push(None);
                    }
                },
            },
            Role::Assistant => match &msg.content {
                MessageContent::Text { content } => {
                    result.push(assistant_message(Some(content.clone()), None));
                    reasoning.push(None);
                },
                MessageContent::Blocks { content } => {
                    let mut assistant_reasoning = String::new();
                    let mut text_parts = Vec::new();
                    let mut tool_calls = Vec::new();
                    for block in content {
                        match block {
                            ContentBlock::Text { text } => text_parts.push(text.as_str()),
                            ContentBlock::Thinking { thinking, .. } => {
                                assistant_reasoning.push_str(thinking);
                            },
                            ContentBlock::ToolUse { .. } => {
                                if let Some(tc) = tool_call_from_block(block) {
                                    tool_calls.push(tc);
                                }
                            },
                            // Drop redacted / unknown blocks when sending to OpenAI.
                            // REVIEW: thinking-only turns can leave empty assistant
                            // messages; sanitize_assistant_messages stubs those.
                            _ => {},
                        }
                    }
                    let content = if text_parts.is_empty() { None } else { Some(text_parts.join("")) };
                    let tool_calls = (!tool_calls.is_empty()).then_some(tool_calls);
                    result.push(assistant_message(content, tool_calls));
                    reasoning.push((!assistant_reasoning.is_empty()).then_some(assistant_reasoning));
                },
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
fn sanitize_assistant_messages(messages: &mut Vec<ChatCompletionRequestMessage>, reasoning: &mut Vec<Option<String>>) {
    debug_assert_eq!(messages.len(), reasoning.len());
    let mut i = 0;
    while i < messages.len() {
        let ChatCompletionRequestMessage::Assistant(assistant) = &messages[i] else {
            i += 1;
            continue;
        };

        let Some(tool_calls) = assistant.tool_calls.as_ref().filter(|tcs| !tcs.is_empty()) else {
            // No tool calls — still ensure the message is not empty.
            stub_empty_assistant_if_needed(&mut messages[i]);
            i += 1;
            continue;
        };

        // Own the ids so we can later mutate `messages[i]` (which currently
        // borrows `tool_calls`).
        let tool_call_ids: Vec<String> =
            tool_calls.iter().map(|tc| tc.id.clone()).filter(|id| !id.is_empty()).collect();

        let mut matched = 0;
        let mut j = i + 1;
        while j < messages.len() {
            match &messages[j] {
                ChatCompletionRequestMessage::Tool(tm) => {
                    if tool_call_ids.iter().any(|id| id == &tm.tool_call_id) {
                        matched += 1;
                    }
                    j += 1;
                },
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

        // Ensure the assistant message is not empty after orphan stripping.
        stub_empty_assistant_if_needed(&mut messages[i]);
        i += 1;
    }
}

fn stub_empty_assistant_if_needed(message: &mut ChatCompletionRequestMessage) {
    let ChatCompletionRequestMessage::Assistant(assistant) = message else {
        return;
    };
    let has_tool_calls = assistant.tool_calls.as_ref().is_some_and(|tcs| !tcs.is_empty());
    if !has_tool_calls && assistant.content.as_deref().unwrap_or("").is_empty() {
        tracing::warn!("Replacing empty assistant message with stub to keep OpenAI request valid");
        assistant.content = Some("[Assistant response was empty or truncated. Continuing...]".to_string());
    }
}

/// Build an OpenAI `CreateChatCompletionRequest` from Tact [`CreateMessageParams`].
///
/// Also returns a parallel vector of optional `reasoning_content` strings, one for each
/// message in the generated OpenAI message list. This is needed for providers such as
/// Kimi/Moonshot that require historical `reasoning_content` to be echoed back on
/// assistant messages (especially assistant messages containing `tool_calls`).
///
/// Provider-specific fields (`thinking`, `reasoning_effort`, `user_id`) are injected
/// later by [`crate::openai::body::OpenAiBodyHook`], not via this typed request.
pub fn build_openai_request(request: &CreateMessageParams) -> (CreateChatCompletionRequest, Vec<Option<String>>) {
    let system_slots = usize::from(request.system.is_some());
    let mut messages = Vec::with_capacity(system_slots + request.messages.len());
    let mut reasoning_per_message = Vec::with_capacity(system_slots + request.messages.len());

    // Anthropic sends system as a top-level field; OpenAI expects it as the first system message.
    if let Some(system) = &request.system {
        messages.push(ChatCompletionRequestMessage::System(ChatCompletionRequestSystemMessage {
            content: system.clone(),
            role: OpenAiRole::System,
            name: None,
        }));
        reasoning_per_message.push(None);
    }

    let (converted_messages, converted_reasoning) = messages_to_openai(&request.messages);
    messages.extend(converted_messages);
    reasoning_per_message.extend(converted_reasoning);

    let openai_request = CreateChatCompletionRequest {
        model: request.model.clone(),
        messages,
        frequency_penalty: None,
        logit_bias: None,
        logprobs: None,
        top_logprobs: None,
        // async-openai's field is `u16`; clamp instead of truncating with `as`.
        max_tokens: Some(u16::try_from(request.max_tokens).unwrap_or(u16::MAX)),
        n: Some(1),
        presence_penalty: None,
        response_format: None,
        seed: None,
        stop: None,
        stream: Some(true),
        stream_options: None,
        temperature: request.temperature,
        top_p: request.top_p,
        tools: request.tools.as_ref().map(|tools| tools.iter().map(ChatCompletionTool::from).collect()),
        tool_choice: request.tool_choice.as_ref().map(Into::into),
        user: None,
    };

    (openai_request, reasoning_per_message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ContentBlock, ImageSource, Message, RequiredMessageParams, Role};

    #[test]
    fn finish_reason_maps_via_from() {
        assert_eq!(StopReason::from(FinishReason::Stop), StopReason::EndTurn);
        assert_eq!(StopReason::from(FinishReason::Length), StopReason::MaxTokens);
        assert_eq!(StopReason::from(FinishReason::ToolCalls), StopReason::ToolUse);
        assert_eq!(StopReason::from(FinishReason::ContentFilter), StopReason::StopSequence);
    }

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

        let (openai, _) = messages_to_openai(&[msg]);
        assert_eq!(openai.len(), 1);
        let ChatCompletionRequestMessage::User(user) = &openai[0] else {
            panic!("expected user message");
        };
        let ChatCompletionRequestUserMessageContent::Array(parts) = &user.content else {
            panic!("expected array content");
        };
        assert_eq!(parts.len(), 2);
        assert!(matches!(&parts[0], ChatCompletionRequestMessageContentPart::Text(t) if t.text == "describe this"));
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
            assistant_msg.content.as_deref().is_some_and(|c| !c.is_empty()),
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
            assistant_msg.content.as_deref().is_some_and(|c| !c.is_empty()),
            "orphaned tool_calls should be stripped and content stubbed"
        );
    }

    #[test]
    fn thinking_only_assistant_gets_stubbed() {
        let assistant = Message::new_blocks(
            Role::Assistant,
            vec![ContentBlock::Thinking { thinking: "partial reasoning".to_string(), signature: String::new() }],
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
            assistant_msg.content.as_deref().is_some_and(|c| !c.is_empty()),
            "thinking-only assistant should be stubbed after thinking is dropped"
        );
        assert!(assistant_msg.tool_calls.is_none());
    }

    #[test]
    fn assistant_with_text_and_thinking_keeps_text() {
        let assistant = Message::new_blocks(
            Role::Assistant,
            vec![
                ContentBlock::Thinking { thinking: "reasoning".to_string(), signature: String::new() },
                ContentBlock::Text { text: "actual answer".to_string() },
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
                ContentBlock::Text { text: "let me check".to_string() },
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
            vec![ContentBlock::ToolResult { tool_use_id: "call_1".to_string(), content: "file content".to_string() }],
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
            openai_request.messages.iter().any(|m| matches!(m, ChatCompletionRequestMessage::Tool(_))),
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
            vec![ContentBlock::ToolResult { tool_use_id: "call_1".to_string(), content: "partial".to_string() }],
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
            openai_request.messages.iter().all(|m| !matches!(m, ChatCompletionRequestMessage::Tool(_))),
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
                ContentBlock::Thinking { thinking: "plan both tools".to_string(), signature: String::new() },
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
                ContentBlock::ToolResult { tool_use_id: "call_a".to_string(), content: "a".to_string() },
                ContentBlock::ToolResult { tool_use_id: "call_b".to_string(), content: "b".to_string() },
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

        let tool_count =
            openai_request.messages.iter().filter(|msg| matches!(msg, ChatCompletionRequestMessage::Tool(_))).count();
        assert_eq!(tool_count, 2);
    }
}
