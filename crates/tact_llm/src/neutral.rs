//! Conversions between Tact's provider wire types and the neutral agent API.

use tact_agent_api::{
    AgentContentBlock, AgentImageSource, AgentMessage, AgentRole, CursorUpdate, MessageId,
    MessageOrigin, ModelCallOptions, ModelCursor, ModelError, ModelErrorKind, ModelMessage,
    ModelRequest, ModelResponse, ReasoningEffort, StopReason as NeutralStopReason, TokenUsage,
};

use crate::{
    ContentBlock, CreateMessageParams, ImageSource, LlmError, LlmResponse, Message, MessageContent,
    MessageKind, ModelCallParams, OpenAiReasoningEffort, ProviderConversationState,
    ProviderStateUpdate, Role, StopReason, Thinking, ThinkingType, TokenUsageInfo, Tool,
};

const CURSOR_PROVIDER: &str = "tact_llm";
const CURSOR_VERSION: u32 = 1;

pub fn agent_content_block(block: &ContentBlock) -> AgentContentBlock {
    match block {
        ContentBlock::Text { text } => AgentContentBlock::Text { text: text.clone() },
        ContentBlock::Image { source } => AgentContentBlock::Image {
            source: AgentImageSource {
                type_: source.type_.clone(),
                media_type: source.media_type.clone(),
                data: source.data.clone(),
            },
        },
        ContentBlock::ToolUse { id, name, input } => AgentContentBlock::ToolUse {
            id: id.clone(),
            name: name.clone(),
            input: input.clone(),
        },
        ContentBlock::ToolResult {
            tool_use_id,
            content,
        } => AgentContentBlock::ToolResult {
            tool_use_id: tool_use_id.clone(),
            content: content.clone(),
        },
        ContentBlock::Thinking {
            thinking,
            signature,
        } => AgentContentBlock::Thinking {
            thinking: thinking.clone(),
            signature: signature.clone(),
        },
        ContentBlock::RedactedThinking { data } => {
            AgentContentBlock::RedactedThinking { data: data.clone() }
        }
    }
}

pub fn llm_content_block(block: &AgentContentBlock) -> ContentBlock {
    match block {
        AgentContentBlock::Text { text } => ContentBlock::Text { text: text.clone() },
        AgentContentBlock::Image { source } => ContentBlock::Image {
            source: ImageSource {
                type_: source.type_.clone(),
                media_type: source.media_type.clone(),
                data: source.data.clone(),
            },
        },
        AgentContentBlock::ToolUse { id, name, input } => ContentBlock::ToolUse {
            id: id.clone(),
            name: name.clone(),
            input: input.clone(),
        },
        AgentContentBlock::ToolResult {
            tool_use_id,
            content,
        } => ContentBlock::ToolResult {
            tool_use_id: tool_use_id.clone(),
            content: content.clone(),
        },
        AgentContentBlock::Thinking {
            thinking,
            signature,
        } => ContentBlock::Thinking {
            thinking: thinking.clone(),
            signature: signature.clone(),
        },
        AgentContentBlock::RedactedThinking { data } => {
            ContentBlock::RedactedThinking { data: data.clone() }
        }
    }
}

fn agent_origin(role: Role, kind: MessageKind, blocks: &[ContentBlock]) -> MessageOrigin {
    match kind {
        MessageKind::Summary => MessageOrigin::CompactionSummary,
        MessageKind::HookContext => MessageOrigin::InjectedContext {
            source: "hook".into(),
        },
        MessageKind::GoalContext => MessageOrigin::GoalContext,
        MessageKind::Normal => match role {
            Role::Assistant => MessageOrigin::Assistant,
            Role::User
                if !blocks.is_empty()
                    && blocks
                        .iter()
                        .any(|block| matches!(block, ContentBlock::ToolResult { .. }))
                    && blocks.iter().all(|block| {
                        matches!(
                            block,
                            ContentBlock::ToolResult { .. } | ContentBlock::Image { .. }
                        )
                    }) =>
            {
                MessageOrigin::ToolResult
            }
            Role::User => MessageOrigin::User,
        },
    }
}

pub fn agent_message(message: &Message) -> AgentMessage {
    let (role, blocks) = match &message.content {
        MessageContent::Text { content } => (
            message.role,
            vec![ContentBlock::Text {
                text: content.clone(),
            }],
        ),
        MessageContent::Blocks { content } => (message.role, content.clone()),
    };
    AgentMessage::new(
        neutral_role(role),
        agent_origin(role, message.kind, &blocks),
        blocks.iter().map(agent_content_block).collect(),
    )
}

pub fn llm_message(message: &AgentMessage) -> Message {
    Message::new_blocks(
        llm_role(message.role),
        message.content.iter().map(llm_content_block).collect(),
    )
}

pub fn neutral_message(message: &Message) -> ModelMessage {
    ModelMessage {
        role: neutral_role(message.role),
        content: match &message.content {
            MessageContent::Text { content } => vec![AgentContentBlock::Text {
                text: content.clone(),
            }],
            MessageContent::Blocks { content } => content.iter().map(agent_content_block).collect(),
        },
    }
}

pub fn neutral_role(role: Role) -> AgentRole {
    match role {
        Role::User => AgentRole::User,
        Role::Assistant => AgentRole::Assistant,
    }
}

pub fn llm_role(role: AgentRole) -> Role {
    match role {
        AgentRole::User => Role::User,
        AgentRole::Assistant => Role::Assistant,
    }
}

pub fn llm_tool(tool: &tact_agent_api::ToolSpec) -> Tool {
    Tool {
        name: tool.name.clone(),
        description: (!tool.description.is_empty()).then(|| tool.description.clone()),
        input_schema: tool.input_schema.clone(),
    }
}

pub fn neutral_tool(tool: &Tool) -> tact_agent_api::ToolSpec {
    tact_agent_api::ToolSpec {
        name: tool.name.clone(),
        description: tool.description.clone().unwrap_or_default(),
        input_schema: tool.input_schema.clone(),
    }
}

pub fn llm_usage(usage: TokenUsage) -> TokenUsageInfo {
    TokenUsageInfo {
        prompt: usage.input_tokens,
        completion: usage.output_tokens,
        total: usage.total_tokens,
        prompt_cache_hit_tokens: usage.cache_read_input_tokens,
        prompt_cache_miss_tokens: usage.cache_miss_input_tokens,
        reasoning_tokens: usage.reasoning_tokens,
    }
}

pub fn neutral_usage(usage: &TokenUsageInfo) -> TokenUsage {
    TokenUsage {
        input_tokens: usage.prompt,
        output_tokens: usage.completion,
        total_tokens: usage.total,
        cache_read_input_tokens: usage.prompt_cache_hit_tokens,
        cache_miss_input_tokens: usage.prompt_cache_miss_tokens,
        reasoning_tokens: usage.reasoning_tokens,
    }
}

pub fn neutral_stop_reason(reason: Option<StopReason>) -> NeutralStopReason {
    match reason {
        None | Some(StopReason::EndTurn) => NeutralStopReason::EndTurn,
        Some(StopReason::MaxTokens) => NeutralStopReason::MaxTokens,
        Some(StopReason::StopSequence) => NeutralStopReason::StopSequence,
        Some(StopReason::ToolUse) => NeutralStopReason::ToolUse,
        Some(StopReason::Refusal) => NeutralStopReason::Refusal,
        Some(StopReason::PauseTurn) => NeutralStopReason::PauseTurn,
        Some(StopReason::Unknown(raw)) => NeutralStopReason::Unknown { raw },
    }
}

pub fn llm_stop_reason(reason: &NeutralStopReason) -> StopReason {
    match reason {
        NeutralStopReason::EndTurn => StopReason::EndTurn,
        NeutralStopReason::MaxTokens => StopReason::MaxTokens,
        NeutralStopReason::StopSequence => StopReason::StopSequence,
        NeutralStopReason::ToolUse => StopReason::ToolUse,
        NeutralStopReason::Refusal => StopReason::Refusal,
        NeutralStopReason::PauseTurn => StopReason::PauseTurn,
        NeutralStopReason::Unknown { raw } => StopReason::Unknown(raw.clone()),
    }
}

pub fn model_cursor(state: &ProviderConversationState) -> ModelCursor {
    ModelCursor {
        provider: CURSOR_PROVIDER.into(),
        version: CURSOR_VERSION,
        payload: serde_json::to_value(state).expect("provider conversation state serializes"),
    }
}

pub fn provider_state(cursor: &ModelCursor) -> Result<ProviderConversationState, ModelError> {
    if cursor.provider != CURSOR_PROVIDER || cursor.version != CURSOR_VERSION {
        return Err(ModelError::new(
            ModelErrorKind::Permanent,
            format!(
                "unsupported provider cursor {} v{}",
                cursor.provider, cursor.version
            ),
        ));
    }
    serde_json::from_value(cursor.payload.clone()).map_err(|error| {
        ModelError::new(
            ModelErrorKind::Permanent,
            format!("invalid provider cursor payload: {error}"),
        )
    })
}

pub fn cursor_update(update: ProviderStateUpdate) -> CursorUpdate {
    match update {
        ProviderStateUpdate::Unchanged => CursorUpdate::Unchanged,
        ProviderStateUpdate::Replace(state) => CursorUpdate::Replace {
            cursor: Some(model_cursor(&state)),
        },
        ProviderStateUpdate::Clear => CursorUpdate::Replace { cursor: None },
    }
}

pub fn provider_state_update(update: CursorUpdate) -> Result<ProviderStateUpdate, ModelError> {
    Ok(match update {
        CursorUpdate::Unchanged => ProviderStateUpdate::Unchanged,
        CursorUpdate::Replace { cursor: None } => ProviderStateUpdate::Clear,
        CursorUpdate::Replace {
            cursor: Some(cursor),
        } => ProviderStateUpdate::Replace(provider_state(&cursor)?),
    })
}

pub fn model_response(response: LlmResponse, message_id: MessageId) -> ModelResponse {
    ModelResponse {
        message_id,
        message: AgentMessage::new(
            AgentRole::Assistant,
            MessageOrigin::Assistant,
            response.blocks.iter().map(agent_content_block).collect(),
        ),
        stop_reason: neutral_stop_reason(response.stop_reason),
        usage: response.usage.as_ref().map(neutral_usage),
        request_body: response.request_body,
        cursor: cursor_update(response.state_update),
    }
}

pub fn llm_response(response: ModelResponse) -> Result<LlmResponse, ModelError> {
    Ok(LlmResponse {
        blocks: response
            .message
            .content
            .iter()
            .map(llm_content_block)
            .collect(),
        stop_reason: Some(llm_stop_reason(&response.stop_reason)),
        usage: response.usage.map(llm_usage),
        request_body: response.request_body,
        state_update: provider_state_update(response.cursor)?,
    })
}

pub fn model_error(error: LlmError) -> ModelError {
    let message = error.to_string();
    let lowered = message.to_ascii_lowercase();
    let kind = match &error {
        LlmError::Unsupported(_) | LlmError::UnsupportedHook(_) => ModelErrorKind::Unsupported,
        LlmError::HttpError { status, .. }
            if *status == 408 || *status == 429 || *status >= 500 =>
        {
            ModelErrorKind::Transient
        }
        _ if lowered.contains("context length")
            || lowered.contains("context window")
            || lowered.contains("prompt") && lowered.contains("long")
            || lowered.contains("too many tokens")
            || lowered.contains("overlong_prompt") =>
        {
            ModelErrorKind::ContextOverflow
        }
        LlmError::Request(_) => ModelErrorKind::Transient,
        _ if lowered.contains("timed out")
            || lowered.contains("rate limit")
            || lowered.contains("too many requests")
            || lowered.contains("connection")
            || lowered.contains("temporarily") =>
        {
            ModelErrorKind::Transient
        }
        _ => ModelErrorKind::Permanent,
    };
    ModelError::new(kind, message)
}

pub fn model_request(request: &CreateMessageParams) -> ModelRequest {
    let mut extra = serde_json::Map::new();
    if let Some(tool_choice) = request.tool_choice.as_ref()
        && let Ok(value) = serde_json::to_value(tool_choice)
    {
        extra.insert("tool_choice".into(), value);
    }
    if let Some(responses_options) = request.responses_options.as_ref()
        && let Ok(value) = serde_json::to_value(responses_options)
    {
        extra.insert("responses_options".into(), value);
    }
    if let Some(extra_body) = request
        .thinking
        .as_ref()
        .and_then(|thinking| serde_json::to_value(thinking).ok())
    {
        extra.insert("extra_body".into(), extra_body);
    }
    ModelRequest {
        model: request.model.clone(),
        system: request.system.clone().unwrap_or_default(),
        messages: request.messages.iter().map(neutral_message).collect(),
        tools: request
            .tools
            .as_ref()
            .map(|tools| tools.iter().map(neutral_tool).collect())
            .unwrap_or_default(),
        max_output_tokens: request.max_tokens,
        options: ModelCallOptions {
            reasoning_effort: request.reasoning_effort.map(ReasoningEffort::from),
            thinking_budget: request
                .thinking
                .as_ref()
                .and_then(|thinking| u32::try_from(thinking.budget_tokens).ok()),
            temperature: request.temperature,
            stop_sequences: request.stop_sequences.clone(),
            top_k: request.top_k,
            top_p: request.top_p,
            extra,
        },
    }
}

pub fn create_message_params(request: &ModelRequest) -> Result<CreateMessageParams, ModelError> {
    let options = &request.options;
    let reasoning_effort = options.reasoning_effort.map(OpenAiReasoningEffort::from);
    let mut request_out = CreateMessageParams::new(crate::RequiredMessageParams {
        model: request.model.clone(),
        messages: request
            .messages
            .iter()
            .map(|message| {
                Message::new_blocks(
                    llm_role(message.role),
                    message.content.iter().map(llm_content_block).collect(),
                )
            })
            .collect(),
        max_tokens: request.max_output_tokens,
    });
    if !request.system.is_empty() {
        request_out = request_out.with_system(request.system.clone());
    }
    if !request.tools.is_empty() {
        request_out = request_out.with_tools(request.tools.iter().map(llm_tool).collect());
    }
    request_out.reasoning_effort = reasoning_effort;
    request_out.temperature = options.temperature;
    request_out.stop_sequences = options.stop_sequences.clone();
    request_out.top_k = options.top_k;
    request_out.top_p = options.top_p;
    request_out.tool_choice = options
        .extra_value("tool_choice")
        .cloned()
        .map(serde_json::from_value)
        .transpose()
        .map_err(|error| {
            ModelError::new(
                ModelErrorKind::Permanent,
                format!("invalid tool_choice option: {error}"),
            )
        })?;
    request_out.responses_options = options
        .extra_value("responses_options")
        .cloned()
        .map(serde_json::from_value)
        .transpose()
        .map_err(|error| {
            ModelError::new(
                ModelErrorKind::Permanent,
                format!("invalid responses_options option: {error}"),
            )
        })?;
    if let Some(budget_tokens) = options.thinking_budget {
        request_out.thinking = Some(Thinking {
            budget_tokens: budget_tokens as usize,
            type_: ThinkingType::Enabled,
        });
    }
    Ok(request_out)
}

pub fn model_call_params(request: &CreateMessageParams) -> ModelCallParams {
    ModelCallParams {
        model: request.model.clone(),
        max_tokens: request.max_tokens,
        thinking_budget: request
            .thinking
            .as_ref()
            .map(|thinking| thinking.budget_tokens as u32),
        reasoning_effort: request.reasoning_effort.map(ReasoningEffort::from),
        extra_body: request
            .thinking
            .as_ref()
            .and_then(|thinking| serde_json::to_string(thinking).ok()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_and_messages_round_trip_through_the_neutral_contract() {
        let original = Message::new_blocks(
            Role::User,
            vec![
                ContentBlock::Text {
                    text: "hello".into(),
                },
                ContentBlock::ToolResult {
                    tool_use_id: "call_1".into(),
                    content: "done".into(),
                },
            ],
        );
        let neutral = agent_message(&original);
        assert_eq!(neutral.origin, MessageOrigin::User);
        let round_trip = llm_message(&neutral);
        assert_eq!(round_trip.role, Role::User);
        assert_eq!(round_trip.content, original.content);
    }

    #[test]
    fn grouped_tool_result_with_image_companion_keeps_tool_origin() {
        let message = Message::new_blocks(
            Role::User,
            vec![
                ContentBlock::ToolResult {
                    tool_use_id: "call_1".into(),
                    content: "preview".into(),
                },
                ContentBlock::Image {
                    source: ImageSource {
                        type_: "base64".into(),
                        media_type: "image/png".into(),
                        data: "AAAA".into(),
                    },
                },
            ],
        );

        assert_eq!(agent_message(&message).origin, MessageOrigin::ToolResult);
    }

    #[test]
    fn provider_cursor_preserves_the_conversation_state() {
        let state = ProviderConversationState::OpenAiResponses(crate::ResponsesConversationState {
            version: 1,
            provider: "openai".into(),
            base_url: "https://api.openai.com/v1".into(),
            model: "gpt".into(),
            input_items: vec![serde_json::json!({"type": "message"})],
            compaction_id: None,
            is_compacted: false,
            logical_message_count: 1,
            logical_context_hash: "hash".into(),
        });
        let cursor = model_cursor(&state);
        assert_eq!(provider_state(&cursor).unwrap(), state);
    }

    #[test]
    fn usage_and_stop_reasons_round_trip() {
        let usage = TokenUsageInfo {
            prompt: 1,
            completion: 2,
            total: 3,
            prompt_cache_hit_tokens: 4,
            prompt_cache_miss_tokens: 5,
            reasoning_tokens: 6,
        };
        assert_eq!(llm_usage(neutral_usage(&usage)), usage);
        for reason in [
            StopReason::EndTurn,
            StopReason::ToolUse,
            StopReason::Unknown("future".into()),
        ] {
            assert_eq!(
                llm_stop_reason(&neutral_stop_reason(Some(reason.clone()))),
                reason
            );
        }
    }

    #[test]
    fn terminal_response_preserves_diagnostics_and_cursor_clear() {
        let response = LlmResponse {
            blocks: vec![ContentBlock::Text {
                text: "answer".into(),
            }],
            stop_reason: Some(StopReason::EndTurn),
            usage: Some(TokenUsageInfo {
                prompt: 1,
                completion: 2,
                total: 3,
                ..TokenUsageInfo::default()
            }),
            request_body: Some(br#"{"stream":true}"#.to_vec()),
            state_update: ProviderStateUpdate::Clear,
        };
        let neutral = model_response(response, MessageId::new("message-1"));
        assert_eq!(
            neutral.request_body.as_deref(),
            Some(br#"{"stream":true}"#.as_slice())
        );
        assert_eq!(neutral.cursor, CursorUpdate::Replace { cursor: None });
        assert_eq!(
            llm_response(neutral).unwrap().state_update,
            ProviderStateUpdate::Clear
        );
    }

    #[test]
    fn unknown_cursor_does_not_silently_become_unchanged() {
        let error = llm_response(ModelResponse {
            message_id: MessageId::new("message-1"),
            message: AgentMessage::new(AgentRole::Assistant, MessageOrigin::Assistant, Vec::new()),
            stop_reason: NeutralStopReason::EndTurn,
            usage: None,
            request_body: None,
            cursor: CursorUpdate::Replace {
                cursor: Some(ModelCursor {
                    provider: "unknown".into(),
                    version: 1,
                    payload: serde_json::json!({}),
                }),
            },
        })
        .expect_err("unknown cursors must be a typed error");
        assert_eq!(error.kind, ModelErrorKind::Permanent);
    }
}
