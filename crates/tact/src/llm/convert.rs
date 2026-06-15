//! Conversion helpers between Anthropic and OpenAI types.

use anthropic_ai_sdk::types::message::{
    ContentBlock, Message, MessageContent, Role, StopReason, Tool,
};
use async_openai::types::{
    ChatCompletionMessageToolCall, ChatCompletionRequestAssistantMessage,
    ChatCompletionRequestMessage, ChatCompletionRequestSystemMessage,
    ChatCompletionRequestToolMessage, ChatCompletionRequestUserMessage,
    ChatCompletionRequestUserMessageContent, ChatCompletionTool, ChatCompletionToolType,
    CreateChatCompletionRequest, FinishReason, FunctionObject, Role as OpenAiRole,
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
#[allow(deprecated)]
pub fn anthropic_messages_to_openai(messages: &[Message]) -> Vec<ChatCompletionRequestMessage> {
    let mut result = Vec::new();
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
                }
                MessageContent::Blocks { content } => {
                    for block in content {
                        match block {
                            ContentBlock::Text { text } => {
                                result.push(ChatCompletionRequestMessage::User(
                                    ChatCompletionRequestUserMessage {
                                        content: ChatCompletionRequestUserMessageContent::Text(
                                            text.clone(),
                                        ),
                                        role: OpenAiRole::User,
                                        name: None,
                                    },
                                ));
                            }
                            ContentBlock::ToolResult {
                                tool_use_id,
                                content,
                            } => {
                                result.push(ChatCompletionRequestMessage::Tool(
                                    ChatCompletionRequestToolMessage {
                                        role: OpenAiRole::Tool,
                                        content: content.clone(),
                                        tool_call_id: tool_use_id.clone(),
                                    },
                                ));
                            }
                            // Drop images, thinking, etc. on the user side for now.
                            _ => {}
                        }
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
                }
                MessageContent::Blocks { content } => {
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
                }
            },
        }
    }
    // Defensive: strip orphaned tool_calls that aren't followed by tool messages.
    sanitize_tool_call_sequence(&mut result);
    result
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
#[allow(deprecated)]
pub fn build_openai_request(
    request: &anthropic_ai_sdk::types::message::CreateMessageParams,
) -> CreateChatCompletionRequest {
    let mut messages = Vec::new();

    // Anthropic sends system as a top-level field; OpenAI expects it as the first system message.
    if let Some(system) = &request.system {
        messages.push(ChatCompletionRequestMessage::System(
            ChatCompletionRequestSystemMessage {
                content: system.clone(),
                role: OpenAiRole::System,
                name: None,
            },
        ));
    }

    messages.extend(anthropic_messages_to_openai(&request.messages));

    let mut openai_request = CreateChatCompletionRequest {
        model: request.model.clone(),
        messages,
        frequency_penalty: None,
        logit_bias: None,
        logprobs: None,
        top_logprobs: None,
        max_tokens: Some(request.max_tokens as u16),
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

    openai_request
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
