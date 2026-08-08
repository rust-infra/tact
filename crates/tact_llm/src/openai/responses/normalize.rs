use std::collections::BTreeMap;

use async_openai_responses::types::responses::{
    OutputItem, OutputMessageContent, OutputStatus, Response, Status, SummaryPart,
};
use tact_protocol::TokenUsageInfo;

use super::history;
use crate::{
    ContentBlock, LlmError, Message, ProviderConversationState, ProviderStateUpdate,
    ResponsesConversationState, Role, StopReason, context_hash,
};

#[derive(Debug)]
pub(crate) struct NormalizedResponse {
    pub blocks: Vec<ContentBlock>,
    pub stop_reason: Option<StopReason>,
    pub usage: Option<TokenUsageInfo>,
    /// Every terminal output item retained as JSON in output order, including
    /// compaction and unknown/unmapped items. This is the protocol output used
    /// to build the next conversation baseline.
    pub output_items: Vec<serde_json::Value>,
}

fn terminal_stop_reason(
    response: &Response,
    has_tools: bool,
    has_refusal: bool,
) -> Result<Option<StopReason>, LlmError> {
    match response.status {
        Status::Completed => {
            if has_tools {
                Ok(Some(StopReason::ToolUse))
            } else if has_refusal {
                Ok(Some(StopReason::Refusal))
            } else {
                Ok(Some(StopReason::EndTurn))
            }
        }
        Status::Incomplete => {
            let reason = response
                .incomplete_details
                .as_ref()
                .map(|details| details.reason.as_str());
            match reason {
                Some("max_output_tokens") => Ok(Some(StopReason::MaxTokens)),
                Some("content_filter") => Ok(Some(StopReason::StopSequence)),
                Some(other) => Err(LlmError::Unsupported(format!(
                    "OpenAI Responses incomplete for unsupported reason '{other}'"
                ))),
                None => Err(LlmError::Unsupported(
                    "OpenAI Responses incomplete without a reason".to_string(),
                )),
            }
        }
        Status::Failed => {
            let detail = response
                .error
                .as_ref()
                .map(|error| format!("{}: {}", error.code, error.message))
                .unwrap_or_else(|| "response failed without error details".to_string());
            Err(LlmError::Unsupported(format!(
                "OpenAI Responses failed: {detail}"
            )))
        }
        Status::Cancelled => Err(LlmError::Unsupported(
            "OpenAI Responses request cancelled".into(),
        )),
        Status::InProgress | Status::Queued => Err(LlmError::Unsupported(format!(
            "OpenAI Responses ended with non-terminal status {:?}",
            response.status
        ))),
    }
}

pub(crate) fn normalize_response(response: Response) -> Result<NormalizedResponse, LlmError> {
    let mut blocks = Vec::new();
    let mut has_tools = false;
    let mut has_refusal = false;
    let function_call_item_ids: BTreeMap<String, String> = response
        .output
        .iter()
        .filter_map(|output| match output {
            OutputItem::FunctionCall(call) => call
                .id
                .as_ref()
                .map(|item_id| (call.call_id.clone(), item_id.clone())),
            _ => None,
        })
        .collect();

    // A terminal response may carry at most one compaction item, and its
    // encrypted content must be non-empty. A compaction item is protocol
    // state, not content: it is retained in `output_items` but never mapped
    // into a `ContentBlock`.
    let compaction_items = response
        .output
        .iter()
        .filter(|output| matches!(output, OutputItem::Compaction(_)))
        .collect::<Vec<_>>();
    if compaction_items.len() > 1 {
        return Err(LlmError::Unsupported(format!(
            "OpenAI Responses terminal response contains {} compaction items; exactly one is required",
            compaction_items.len()
        )));
    }
    if let Some(OutputItem::Compaction(compaction)) = compaction_items.first()
        && compaction.encrypted_content.is_empty()
    {
        return Err(LlmError::Unsupported(
            "OpenAI Responses compaction item has empty encrypted_content".to_string(),
        ));
    }

    for output in &response.output {
        match output {
            OutputItem::Reasoning(reasoning) => {
                let thinking = reasoning
                    .summary
                    .iter()
                    .map(|part| match part {
                        SummaryPart::SummaryText(summary) => summary.text.as_str(),
                    })
                    .collect::<Vec<_>>()
                    .join("\n\n");
                let signature = if reasoning
                    .encrypted_content
                    .as_ref()
                    .is_some_and(|encrypted| !encrypted.is_empty())
                {
                    history::encode(reasoning.clone(), function_call_item_ids.clone())?
                } else {
                    String::new()
                };
                if !thinking.is_empty() || !signature.is_empty() {
                    blocks.push(ContentBlock::Thinking {
                        thinking,
                        signature,
                    });
                }
            }
            OutputItem::Message(message) => {
                for content in &message.content {
                    match content {
                        OutputMessageContent::OutputText(output) => {
                            if !output.text.is_empty() {
                                blocks.push(ContentBlock::Text {
                                    text: output.text.clone(),
                                });
                            }
                        }
                        OutputMessageContent::Refusal(refusal) => {
                            has_refusal = true;
                            if !refusal.refusal.is_empty() {
                                blocks.push(ContentBlock::Text {
                                    text: refusal.refusal.clone(),
                                });
                            }
                        }
                    }
                }
            }
            OutputItem::FunctionCall(call) => {
                if call.status != Some(OutputStatus::Completed) {
                    continue;
                }
                let input = serde_json::from_str(&call.arguments).map_err(|error| {
                    LlmError::Unsupported(format!(
                        "parse arguments for OpenAI function '{}' call '{}': {error}",
                        call.name, call.call_id
                    ))
                })?;
                has_tools = true;
                blocks.push(ContentBlock::ToolUse {
                    id: call.call_id.clone(),
                    name: call.name.clone(),
                    input,
                });
            }
            // Compaction is opaque protocol state: it must not become a
            // ContentBlock, and it is retained as JSON in `output_items`.
            OutputItem::Compaction(_) => {}
            // Unmapped output items known to the typed SDK (file search, web
            // search, computer use, …) produce no ContentBlock but are
            // retained as JSON in `output_items`. Truly unknown future items
            // are filtered by the raw wire boundary before this typed
            // normalization path and are restored from the raw sequence.
            _ => {}
        }
    }

    let output_items = response
        .output
        .iter()
        .map(serde_json::to_value)
        .collect::<Result<Vec<_>, _>>()?;

    let stop_reason = terminal_stop_reason(&response, has_tools, has_refusal)?;
    let usage = response.usage.as_ref().map(|usage| TokenUsageInfo {
        prompt: usage.input_tokens,
        completion: usage.output_tokens,
        total: usage.total_tokens,
        prompt_cache_hit_tokens: usage.input_tokens_details.cached_tokens,
        prompt_cache_miss_tokens: usage
            .input_tokens
            .saturating_sub(usage.input_tokens_details.cached_tokens),
        reasoning_tokens: usage.output_tokens_details.reasoning_tokens,
    });

    Ok(NormalizedResponse {
        blocks,
        stop_reason,
        usage,
        output_items,
    })
}

/// Finds the single compaction item in a protocol output sequence.
///
/// Returns `Ok(None)` when no compaction item is present (an ordinary
/// response), `Ok(Some(index))` for exactly one compaction item with non-empty
/// `encrypted_content`, and a protocol error for multiple compaction items or
/// empty encrypted content.
fn find_single_compaction(output_items: &[serde_json::Value]) -> Result<Option<usize>, LlmError> {
    let indices = output_items
        .iter()
        .enumerate()
        .filter(|(_, item)| {
            item.get("type").and_then(serde_json::Value::as_str) == Some("compaction")
        })
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    match indices.len() {
        0 => Ok(None),
        1 => {
            let item = &output_items[indices[0]];
            match item
                .get("encrypted_content")
                .and_then(serde_json::Value::as_str)
            {
                Some(content) if !content.is_empty() => Ok(Some(indices[0])),
                _ => Err(LlmError::Unsupported(
                    "OpenAI Responses compaction item has empty encrypted_content".to_string(),
                )),
            }
        }
        count => Err(LlmError::Unsupported(format!(
            "OpenAI Responses output contains {count} compaction items; exactly one is required"
        ))),
    }
}

/// Parsed and validated `/responses/compact` output.
#[derive(Debug)]
pub(crate) struct ParsedCompactResource {
    /// The compacted output items, in protocol order (retained user items
    /// followed by the single compaction item). This is the replacement
    /// conversation baseline for the next request.
    pub input_items: Vec<serde_json::Value>,
    /// The id of the single validated compaction item.
    pub compaction_id: String,
    /// Token accounting reported by the compaction pass, when present.
    pub usage: Option<TokenUsageInfo>,
}

/// Reads a required responses usage token field as a `u32`, rejecting fields
/// that are absent, not unsigned integers, or larger than `u32::MAX` instead
/// of truncating.
fn token_u32(value: &serde_json::Value, field: &str) -> Result<u32, LlmError> {
    let raw = value.as_u64().ok_or_else(|| {
        LlmError::Unsupported(format!(
            "Responses usage field '{field}' is not an unsigned integer"
        ))
    })?;
    u32::try_from(raw)
        .map_err(|_| LlmError::Unsupported(format!("Responses usage field '{field}' exceeds u32")))
}

/// Reads an optional responses usage token field, defaulting to zero when
/// absent. A present field that is not an unsigned integer or exceeds
/// `u32::MAX` is a hard protocol error.
fn optional_token_u32(
    usage: &serde_json::Value,
    details_field: &str,
    token_field: &str,
) -> Result<u32, LlmError> {
    match usage
        .get(details_field)
        .and_then(|details| details.get(token_field))
    {
        Some(value) => token_u32(value, token_field),
        None => Ok(0),
    }
}

/// Maps the `usage` object of a `/responses/compact` resource (or any
/// responses-shaped usage JSON) into Tact's shared usage type. Unknown usage
/// fields are ignored; a missing `usage` object yields `None`. Required token
/// fields must be present and fit `u32`; optional `cached_tokens` and
/// `reasoning_tokens` default to zero only when absent.
fn usage_from_value(value: &serde_json::Value) -> Result<Option<TokenUsageInfo>, LlmError> {
    let usage = match value.get("usage") {
        Some(usage @ serde_json::Value::Object(_)) => usage,
        _ => return Ok(None),
    };
    let input_tokens = token_u32(
        usage.get("input_tokens").ok_or_else(|| {
            LlmError::Unsupported("Responses usage field 'input_tokens' is missing".to_string())
        })?,
        "input_tokens",
    )?;
    let output_tokens = token_u32(
        usage.get("output_tokens").ok_or_else(|| {
            LlmError::Unsupported("Responses usage field 'output_tokens' is missing".to_string())
        })?,
        "output_tokens",
    )?;
    let total = token_u32(
        usage.get("total_tokens").ok_or_else(|| {
            LlmError::Unsupported("Responses usage field 'total_tokens' is missing".to_string())
        })?,
        "total_tokens",
    )?;
    let cached = optional_token_u32(usage, "input_tokens_details", "cached_tokens")?;
    let reasoning = optional_token_u32(usage, "output_tokens_details", "reasoning_tokens")?;
    Ok(Some(TokenUsageInfo {
        prompt: input_tokens,
        completion: output_tokens,
        total,
        prompt_cache_hit_tokens: cached,
        prompt_cache_miss_tokens: input_tokens.saturating_sub(cached),
        reasoning_tokens: reasoning,
    }))
}

/// Validates a `CompactResource` JSON body and extracts its replacement
/// baseline. A valid compact resource must be a `response.compaction` object
/// with a non-empty top-level `id` and must contain exactly one compaction
/// item with non-empty `encrypted_content`.
pub(crate) fn parse_compact_resource(
    value: serde_json::Value,
) -> Result<ParsedCompactResource, LlmError> {
    let object = value
        .get("object")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| {
            LlmError::Unsupported("CompactResource is missing the `object` field".to_string())
        })?;
    if object != "response.compaction" {
        return Err(LlmError::Unsupported(format!(
            "CompactResource has object '{object}', expected 'response.compaction'"
        )));
    }
    let resource_id = value
        .get("id")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| {
            LlmError::Unsupported("CompactResource is missing the top-level `id`".to_string())
        })?;
    if resource_id.is_empty() {
        return Err(LlmError::Unsupported(
            "CompactResource has an empty top-level `id`".to_string(),
        ));
    }
    let output = value
        .get("output")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| {
            LlmError::Unsupported("CompactResource is missing the `output` array".to_string())
        })?;
    let compaction_index = find_single_compaction(output)?.ok_or_else(|| {
        LlmError::Unsupported(
            "CompactResource output contains no compaction item; exactly one is required"
                .to_string(),
        )
    })?;
    let compaction_id = output[compaction_index]
        .get("id")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| {
            LlmError::Unsupported("CompactResource compaction item is missing `id`".to_string())
        })?
        .to_string();
    Ok(ParsedCompactResource {
        input_items: output.clone(),
        compaction_id,
        usage: usage_from_value(&value)?,
    })
}

impl NormalizedResponse {
    /// Builds the provider state update for this terminal response.
    ///
    /// Ordinary responses append the terminal output to the exact request
    /// input. Responses containing a compaction boundary use the Phase 0
    /// fixture contract instead: the single compaction item replaces the
    /// entire prior baseline and is followed by the current response's
    /// non-compaction output items.
    ///
    /// The logical anchor is advanced to the **post-assistant** context when
    /// the response produced terminal output items: the baseline then covers
    /// the assistant message the agent pushes, so the next turn converts only
    /// the new user/tool suffix and never duplicates assistant/reasoning/
    /// function-call items or ids. When the terminal output is empty (the
    /// compatible-endpoint visible-text recovery path) the baseline did not
    /// grow, so the anchor stays at the request prefix and the assistant
    /// message is converted on the next turn instead of being dropped from
    /// the wire. The post-assistant hash covers `messages` plus exactly the
    /// assistant message built from the normalized blocks — the same message
    /// the agent pushes into its logical context.
    pub(crate) fn provider_state_update(
        &self,
        request_input_items: Vec<serde_json::Value>,
        provider: &str,
        base_url: &str,
        model: &str,
        messages: &[Message],
    ) -> Result<ProviderStateUpdate, LlmError> {
        let compaction_index = find_single_compaction(&self.output_items)?;
        let (input_items, compaction_id, is_compacted) = match compaction_index {
            None => {
                let mut items = request_input_items;
                items.extend(self.output_items.iter().cloned());
                (items, None, false)
            }
            Some(index) => {
                let mut items = Vec::with_capacity(self.output_items.len());
                items.push(self.output_items[index].clone());
                items.extend(
                    self.output_items
                        .iter()
                        .enumerate()
                        .filter(|(item_index, _)| *item_index != index)
                        .map(|(_, item)| item.clone()),
                );
                let id = self.output_items[index]
                    .get("id")
                    .and_then(serde_json::Value::as_str)
                    .ok_or_else(|| {
                        LlmError::Unsupported(
                            "OpenAI Responses compaction item is missing `id`".to_string(),
                        )
                    })?
                    .to_string();
                (items, Some(id), true)
            }
        };
        let (logical_message_count, logical_context_hash) = if self.output_items.is_empty() {
            (messages.len(), context_hash(messages)?)
        } else {
            let mut post_assistant = messages.to_vec();
            post_assistant.push(Message::new_blocks(Role::Assistant, self.blocks.clone()));
            (post_assistant.len(), context_hash(&post_assistant)?)
        };
        Ok(ProviderStateUpdate::Replace(
            ProviderConversationState::OpenAiResponses(ResponsesConversationState {
                version: 1,
                provider: provider.to_string(),
                base_url: base_url.to_string(),
                model: model.to_string(),
                input_items,
                compaction_id,
                is_compacted,
                logical_message_count,
                logical_context_hash,
            }),
        ))
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use async_openai_responses::types::responses::Response;

    use super::{normalize_response, parse_compact_resource};
    use crate::{
        ContentBlock, CreateMessageParams, Message, RequiredMessageParams, Role, StopReason,
    };

    pub(crate) fn completed_response_json() -> serde_json::Value {
        serde_json::json!({
            "created_at": 1,
            "completed_at": 2,
            "id": "resp_1",
            "model": "gpt-5",
            "object": "response",
            "output": [
                {
                    "type": "reasoning",
                    "id": "rs_1",
                    "summary": [{"type": "summary_text", "text": "plan"}],
                    "encrypted_content": "encrypted-plan",
                    "status": "completed"
                },
                {
                    "type": "message",
                    "id": "msg_1",
                    "role": "assistant",
                    "status": "completed",
                    "content": [{
                        "type": "output_text",
                        "annotations": [],
                        "logprobs": null,
                        "text": "answer"
                    }]
                },
                {
                    "type": "function_call",
                    "arguments": "{\"cmd\":\"pwd\"}",
                    "call_id": "call_1",
                    "name": "bash",
                    "id": "fc_1",
                    "status": "completed"
                }
            ],
            "status": "completed",
            "usage": {
                "input_tokens": 100,
                "input_tokens_details": {"cached_tokens": 12},
                "output_tokens": 25,
                "output_tokens_details": {"reasoning_tokens": 7},
                "total_tokens": 125
            }
        })
    }

    fn response_with_status(status: &str, reason: Option<&str>) -> Response {
        let mut value = completed_response_json();
        value["status"] = serde_json::Value::String(status.to_string());
        value["output"] = serde_json::json!([]);
        value["usage"] = serde_json::Value::Null;
        value["incomplete_details"] = reason
            .map(|reason| serde_json::json!({"reason": reason}))
            .unwrap_or(serde_json::Value::Null);
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn normalizes_text_reasoning_tools_usage_and_stop_reason() {
        let response: Response = serde_json::from_value(completed_response_json()).unwrap();
        let normalized = normalize_response(response).unwrap();

        assert!(matches!(
            &normalized.blocks[0],
            ContentBlock::Thinking { thinking, signature }
                if thinking == "plan" && !signature.is_empty()
        ));
        assert!(matches!(
            &normalized.blocks[1],
            ContentBlock::Text { text } if text == "answer"
        ));
        assert!(matches!(
            &normalized.blocks[2],
            ContentBlock::ToolUse { id, name, input }
                if id == "call_1" && name == "bash" && input["cmd"] == "pwd"
        ));
        assert_eq!(normalized.stop_reason, Some(StopReason::ToolUse));

        let usage = normalized.usage.unwrap();
        assert_eq!(usage.prompt, 100);
        assert_eq!(usage.prompt_cache_hit_tokens, 12);
        assert_eq!(usage.prompt_cache_miss_tokens, 88);
        assert_eq!(usage.completion, 25);
        assert_eq!(usage.reasoning_tokens, 7);
        assert_eq!(usage.total, 125);
    }

    #[test]
    fn incomplete_max_output_tokens_maps_to_max_tokens() {
        let normalized = normalize_response(response_with_status(
            "incomplete",
            Some("max_output_tokens"),
        ))
        .unwrap();
        assert_eq!(normalized.stop_reason, Some(StopReason::MaxTokens));
    }

    #[test]
    fn incomplete_max_output_tokens_with_tool_still_maps_to_max_tokens() {
        let mut value = completed_response_json();
        value["status"] = serde_json::json!("incomplete");
        value["incomplete_details"] = serde_json::json!({"reason": "max_output_tokens"});

        let normalized = normalize_response(serde_json::from_value(value).unwrap()).unwrap();
        assert!(
            normalized
                .blocks
                .iter()
                .any(|block| matches!(block, ContentBlock::ToolUse { .. }))
        );
        assert_eq!(normalized.stop_reason, Some(StopReason::MaxTokens));
    }

    #[test]
    fn failed_response_with_tool_is_an_error() {
        let mut value = completed_response_json();
        value["status"] = serde_json::json!("failed");
        value["error"] = serde_json::json!({
            "code": "server_error",
            "message": "generation failed"
        });

        let error = normalize_response(serde_json::from_value(value).unwrap())
            .unwrap_err()
            .to_string();
        assert!(error.contains("server_error"));
        assert!(error.contains("generation failed"));
    }

    #[test]
    fn incomplete_function_call_is_not_executable() {
        let mut value = completed_response_json();
        value["status"] = serde_json::json!("incomplete");
        value["incomplete_details"] = serde_json::json!({"reason": "max_output_tokens"});
        value["output"][2]["status"] = serde_json::json!("incomplete");

        let normalized = normalize_response(serde_json::from_value(value).unwrap()).unwrap();
        assert!(
            normalized
                .blocks
                .iter()
                .all(|block| !matches!(block, ContentBlock::ToolUse { .. }))
        );
        assert_eq!(normalized.stop_reason, Some(StopReason::MaxTokens));
    }

    #[test]
    fn unknown_incomplete_reason_is_an_error() {
        let error = normalize_response(response_with_status("incomplete", Some("new_reason")))
            .unwrap_err()
            .to_string();
        assert!(error.contains("new_reason"));
    }

    #[test]
    fn malformed_function_arguments_return_contextual_error() {
        let mut value = completed_response_json();
        value["output"][2]["arguments"] = serde_json::json!("{");
        let error = normalize_response(serde_json::from_value(value).unwrap())
            .unwrap_err()
            .to_string();
        assert!(error.contains("bash"));
        assert!(error.contains("call_1"));
    }

    #[test]
    fn response_output_item_ids_survive_tact_history_round_trip() {
        let mut value = completed_response_json();
        value["output"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({
                "type": "function_call",
                "arguments": "{\"path\":\"Cargo.toml\"}",
                "call_id": "call_2",
                "name": "read_file",
                "id": "fc_2",
                "status": "completed"
            }));
        let response: Response = serde_json::from_value(value).unwrap();
        let normalized = normalize_response(response).unwrap();
        let request = CreateMessageParams::new(RequiredMessageParams {
            model: "gpt-5".to_string(),
            messages: vec![
                Message::new_blocks(Role::Assistant, normalized.blocks),
                Message::new_blocks(
                    Role::User,
                    vec![
                        ContentBlock::ToolResult {
                            tool_use_id: "call_1".to_string(),
                            content: "/tmp/project".to_string(),
                        },
                        ContentBlock::ToolResult {
                            tool_use_id: "call_2".to_string(),
                            content: "[workspace]".to_string(),
                        },
                    ],
                ),
            ],
            max_tokens: 4096,
        });

        let (body, _) = super::super::convert::create_response(&request, None, None)
            .expect("round-trip request");
        let input = body["input"].as_array().unwrap();
        let reasoning = input
            .iter()
            .find(|item| item["type"] == "reasoning")
            .unwrap();
        let function_call = input
            .iter()
            .find(|item| item["type"] == "function_call")
            .unwrap();
        assert_eq!(reasoning["id"], "rs_1");
        assert_eq!(function_call["id"], "fc_1");
        let second_call = input
            .iter()
            .find(|item| item["call_id"] == "call_2")
            .unwrap();
        assert_eq!(second_call["id"], "fc_2");
    }

    #[test]
    fn retains_every_output_item_as_json_in_output_order() {
        let response: Response = serde_json::from_value(completed_response_json()).unwrap();
        let normalized = normalize_response(response).unwrap();

        assert_eq!(normalized.output_items.len(), 3);
        assert_eq!(normalized.output_items[0]["type"], "reasoning");
        assert_eq!(normalized.output_items[0]["id"], "rs_1");
        assert_eq!(normalized.output_items[1]["type"], "message");
        assert_eq!(normalized.output_items[1]["id"], "msg_1");
        assert_eq!(normalized.output_items[2]["type"], "function_call");
        assert_eq!(normalized.output_items[2]["call_id"], "call_1");
    }

    #[test]
    fn compaction_item_is_retained_as_json_but_not_mapped_to_a_block() {
        let mut value = completed_response_json();
        value["output"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({
                "type": "compaction",
                "id": "cmp_1",
                "encrypted_content": "encrypted-compaction"
            }));
        let response: Response = serde_json::from_value(value).unwrap();
        let normalized = normalize_response(response).unwrap();

        assert_eq!(normalized.output_items.len(), 4);
        assert_eq!(normalized.output_items[3]["type"], "compaction");
        assert_eq!(normalized.output_items[3]["id"], "cmp_1");
        assert!(normalized.blocks.iter().all(|block| {
            !matches!(block, ContentBlock::Text { text } if text.contains("encrypted"))
        }));
    }

    #[test]
    fn rejects_multiple_compaction_items_in_a_terminal_response() {
        let mut value = completed_response_json();
        value["output"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({
                "type": "compaction",
                "id": "cmp_1",
                "encrypted_content": "first"
            }));
        value["output"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({
                "type": "compaction",
                "id": "cmp_2",
                "encrypted_content": "second"
            }));
        let response: Response = serde_json::from_value(value).unwrap();

        let error = normalize_response(response).unwrap_err().to_string();
        assert!(error.contains("2 compaction items"));
    }

    #[test]
    fn rejects_empty_compaction_encrypted_content() {
        let mut value = completed_response_json();
        value["output"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({
                "type": "compaction",
                "id": "cmp_1",
                "encrypted_content": ""
            }));
        let response: Response = serde_json::from_value(value).unwrap();

        let error = normalize_response(response).unwrap_err().to_string();
        assert!(error.contains("empty encrypted_content"));
    }

    #[test]
    fn ordinary_response_appends_output_items_to_the_request_input() {
        let response: Response = serde_json::from_value(completed_response_json()).unwrap();
        let normalized = normalize_response(response).unwrap();
        let messages = vec![Message::new_text(Role::User, "hi")];
        let update = normalized
            .provider_state_update(
                vec![serde_json::json!({
                    "type": "message",
                    "role": "user",
                    "content": [{"type": "input_text", "text": "hi"}]
                })],
                "openai_responses",
                "https://api.openai.com/v1",
                "gpt-5",
                &messages,
            )
            .unwrap();

        let crate::ProviderStateUpdate::Replace(crate::ProviderConversationState::OpenAiResponses(
            state,
        )) = update
        else {
            panic!("expected a replacement state");
        };
        assert_eq!(state.input_items.len(), 4);
        assert_eq!(state.input_items[0]["role"], "user");
        assert_eq!(state.input_items[3]["type"], "function_call");
        assert_eq!(state.compaction_id, None);
        assert!(!state.is_compacted);
        // The baseline already covers the terminal output, so the anchor
        // advances past the assistant message the agent pushes: the next
        // turn converts only the new user/tool suffix and never duplicates
        // assistant/reasoning/function-call items or ids.
        let expected_post_assistant = vec![
            messages[0].clone(),
            Message::new_blocks(Role::Assistant, normalized.blocks.clone()),
        ];
        assert_eq!(state.logical_message_count, 2);
        assert_eq!(
            state.logical_context_hash,
            crate::context_hash(&expected_post_assistant).unwrap()
        );
    }

    #[test]
    fn response_without_terminal_output_keeps_pre_assistant_anchor() {
        // A terminal response without output items (visible-text recovery
        // path) must not advance the anchor: its baseline did not gain the
        // assistant output, so the assistant message is converted on the
        // next turn instead of being silently dropped from the wire.
        let mut value = completed_response_json();
        value["output"] = serde_json::json!([]);
        value["usage"] = serde_json::Value::Null;
        let response: Response = serde_json::from_value(value).unwrap();
        let normalized = normalize_response(response).unwrap();
        assert!(normalized.output_items.is_empty());

        let messages = vec![Message::new_text(Role::User, "hi")];
        let update = normalized
            .provider_state_update(
                vec![serde_json::json!({
                    "type": "message",
                    "role": "user",
                    "content": [{"type": "input_text", "text": "hi"}]
                })],
                "openai_responses",
                "https://api.openai.com/v1",
                "gpt-5",
                &messages,
            )
            .unwrap();
        let crate::ProviderStateUpdate::Replace(crate::ProviderConversationState::OpenAiResponses(
            state,
        )) = update
        else {
            panic!("expected a replacement state");
        };
        assert_eq!(state.input_items.len(), 1, "baseline did not grow");
        assert_eq!(state.logical_message_count, 1);
        assert_eq!(
            state.logical_context_hash,
            crate::context_hash(&messages).unwrap()
        );
    }

    #[test]
    fn unknown_output_item_type_is_a_hard_protocol_error() {
        // The typed SDK boundary: async-openai 0.41 `OutputItem` has no
        // `Unknown` variant, so a truly unknown terminal output item type
        // fails typed deserialization instead of being silently dropped.
        // Tact keeps this hard validation; there is no fallback.
        let mut value = completed_response_json();
        value["output"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({
                "type": "future_unknown_item",
                "opaque": { "any": ["shape"] }
            }));
        let error = serde_json::from_value::<Response>(value)
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("unknown variant"),
            "typed SDK must reject unknown output item types, got: {error}"
        );
    }

    #[test]
    fn automatic_compaction_replaces_the_baseline_from_the_fixture_contract() {
        let value: serde_json::Value =
            serde_json::from_str(include_str!("fixtures/automatic_compact.json")).unwrap();
        let response: Response = serde_json::from_value(value).unwrap();
        let normalized = normalize_response(response).unwrap();

        // The compaction item is not normalized into a ContentBlock.
        assert!(matches!(
            normalized.blocks[0],
            ContentBlock::Thinking { .. }
        ));
        assert!(matches!(normalized.blocks[1], ContentBlock::Text { .. }));
        assert_eq!(normalized.blocks.len(), 2);
        assert_eq!(normalized.output_items.len(), 3);
        assert_eq!(normalized.output_items[2]["type"], "compaction");

        let update = normalized
            .provider_state_update(
                vec![serde_json::json!({"type": "message", "role": "user", "content": "old"})],
                "openai_responses",
                "https://api.openai.com/v1",
                "gpt-5.4-mini",
                &[
                    Message::new_text(Role::User, "old turn"),
                    Message::new_text(Role::Assistant, "prior answer"),
                    Message::new_text(Role::User, "current turn"),
                ],
            )
            .unwrap();
        let crate::ProviderStateUpdate::Replace(crate::ProviderConversationState::OpenAiResponses(
            state,
        )) = update
        else {
            panic!("expected a replacement state");
        };
        // The fixture contract: the single compaction item replaces the entire
        // prior baseline and is followed by the current response output items.
        assert_eq!(state.input_items.len(), 3);
        assert_eq!(state.input_items[0]["type"], "compaction");
        assert_eq!(state.input_items[0]["id"], "cmp_sanitized_01");
        assert_eq!(state.input_items[1]["type"], "reasoning");
        assert_eq!(state.input_items[2]["type"], "message");
        assert_eq!(state.compaction_id.as_deref(), Some("cmp_sanitized_01"));
        assert!(state.is_compacted);
        // The compaction item stands in for every prior turn and the output
        // items cover the current assistant response: the anchor covers the
        // full post-assistant logical context.
        let expected_post_assistant = vec![
            Message::new_text(Role::User, "old turn"),
            Message::new_text(Role::Assistant, "prior answer"),
            Message::new_text(Role::User, "current turn"),
            Message::new_blocks(Role::Assistant, normalized.blocks.clone()),
        ];
        assert_eq!(state.logical_message_count, 4);
        assert_eq!(
            state.logical_context_hash,
            crate::context_hash(&expected_post_assistant).unwrap()
        );
    }

    #[test]
    fn compact_resource_preserves_reported_usage() {
        let value: serde_json::Value =
            serde_json::from_str(include_str!("fixtures/explicit_compact.json")).unwrap();
        let parsed = parse_compact_resource(value).unwrap();
        let usage = parsed.usage.expect("fixture carries usage");
        assert_eq!(usage.prompt, 1200);
        assert_eq!(usage.completion, 340);
        assert_eq!(usage.total, 1540);
        assert_eq!(usage.prompt_cache_hit_tokens, 0);
        assert_eq!(usage.reasoning_tokens, 0);
    }

    fn compact_resource_with_usage(usage: serde_json::Value) -> serde_json::Value {
        serde_json::json!({
            "id": "cmp",
            "object": "response.compaction",
            "output": [{"type":"compaction","id":"cmp-item","encrypted_content":"opaque"}],
            "usage": usage
        })
    }

    #[test]
    fn compact_usage_overflow_is_rejected() {
        let value = compact_resource_with_usage(serde_json::json!({
            "input_tokens": u64::from(u32::MAX) + 1,
            "output_tokens": 1,
            "total_tokens": 1
        }));
        let error = parse_compact_resource(value).unwrap_err().to_string();
        assert!(error.contains("token") || error.contains("usage"));
    }

    #[test]
    fn compact_usage_required_output_tokens_overflow_is_rejected() {
        let value = compact_resource_with_usage(serde_json::json!({
            "input_tokens": 1,
            "output_tokens": u64::from(u32::MAX) + 1,
            "total_tokens": 1
        }));
        let error = parse_compact_resource(value).unwrap_err().to_string();
        assert!(error.contains("token") || error.contains("usage"));
    }

    #[test]
    fn compact_usage_required_total_tokens_overflow_is_rejected() {
        let value = compact_resource_with_usage(serde_json::json!({
            "input_tokens": 1,
            "output_tokens": 1,
            "total_tokens": u64::from(u32::MAX) + 1
        }));
        let error = parse_compact_resource(value).unwrap_err().to_string();
        assert!(error.contains("token") || error.contains("usage"));
    }

    #[test]
    fn compact_usage_cached_tokens_overflow_is_rejected() {
        let value = compact_resource_with_usage(serde_json::json!({
            "input_tokens": 1,
            "input_tokens_details": {"cached_tokens": u64::from(u32::MAX) + 1},
            "output_tokens": 1,
            "total_tokens": 1
        }));
        let error = parse_compact_resource(value).unwrap_err().to_string();
        assert!(error.contains("token") || error.contains("usage"));
    }

    #[test]
    fn compact_usage_reasoning_tokens_overflow_is_rejected() {
        let value = compact_resource_with_usage(serde_json::json!({
            "input_tokens": 1,
            "output_tokens": 1,
            "output_tokens_details": {"reasoning_tokens": u64::from(u32::MAX) + 1},
            "total_tokens": 1
        }));
        let error = parse_compact_resource(value).unwrap_err().to_string();
        assert!(error.contains("token") || error.contains("usage"));
    }

    #[test]
    fn compact_usage_invalid_required_token_type_is_rejected() {
        let value = compact_resource_with_usage(serde_json::json!({
            "input_tokens": "many",
            "output_tokens": 1,
            "total_tokens": 1
        }));
        let error = parse_compact_resource(value).unwrap_err().to_string();
        assert!(error.contains("token") || error.contains("usage"));
    }

    #[test]
    fn compact_usage_invalid_optional_token_type_is_rejected() {
        let value = compact_resource_with_usage(serde_json::json!({
            "input_tokens": 1,
            "input_tokens_details": {"cached_tokens": "many"},
            "output_tokens": 1,
            "total_tokens": 1
        }));
        let error = parse_compact_resource(value).unwrap_err().to_string();
        assert!(error.contains("token") || error.contains("usage"));
    }

    #[test]
    fn compact_usage_missing_required_field_is_rejected() {
        let value = compact_resource_with_usage(serde_json::json!({
            "output_tokens": 1,
            "total_tokens": 1
        }));
        let error = parse_compact_resource(value).unwrap_err().to_string();
        assert!(error.contains("input_tokens"));
    }

    #[test]
    fn compact_usage_absent_optional_fields_default_to_zero() {
        let value = compact_resource_with_usage(serde_json::json!({
            "input_tokens": 5,
            "output_tokens": 5,
            "total_tokens": 10
        }));
        let parsed = parse_compact_resource(value).unwrap();
        let usage = parsed.usage.expect("usage object present");
        assert_eq!(usage.prompt, 5);
        assert_eq!(usage.completion, 5);
        assert_eq!(usage.total, 10);
        assert_eq!(usage.prompt_cache_hit_tokens, 0);
        assert_eq!(usage.prompt_cache_miss_tokens, 5);
        assert_eq!(usage.reasoning_tokens, 0);
    }

    #[test]
    fn compact_resource_without_usage_yields_none() {
        let value = serde_json::json!({
            "id": "cmp",
            "object": "response.compaction",
            "output": [{"type":"compaction","id":"cmp-item","encrypted_content":"opaque"}]
        });
        let parsed = parse_compact_resource(value).unwrap();
        assert!(parsed.usage.is_none());
    }

    #[test]
    fn automatic_compact_fixture_normalizes_without_exposing_encrypted_content() {
        let value: serde_json::Value =
            serde_json::from_str(include_str!("fixtures/automatic_compact.json")).unwrap();
        let response: Response = serde_json::from_value(value).unwrap();
        let normalized = normalize_response(response).unwrap();

        let visible = normalized
            .blocks
            .iter()
            .filter_map(|block| match block {
                ContentBlock::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<String>();
        assert!(!visible.contains("encrypted"));
        assert!(!visible.contains("sanitized-encrypted"));
    }

    #[test]
    fn reasoning_encrypted_envelope_is_internal_signature_only() {
        // The reasoning encrypted envelope is permitted only inside the
        // internal, non-renderable `Thinking.signature`. It must never become
        // a renderable `ContentBlock::Text` (which the TUI shows verbatim).
        let response: Response = serde_json::from_value(completed_response_json()).unwrap();
        let normalized = normalize_response(response).unwrap();

        let mut signature_carriers = 0;
        for block in &normalized.blocks {
            match block {
                ContentBlock::Thinking {
                    thinking,
                    signature,
                } => {
                    assert!(
                        signature.contains("encrypted-plan"),
                        "Thinking.signature must carry the encrypted envelope"
                    );
                    assert!(
                        !thinking.contains("encrypted-plan"),
                        "visible thinking text must not carry the envelope"
                    );
                    signature_carriers += 1;
                }
                ContentBlock::Text { text } => {
                    assert!(
                        !text.contains("encrypted-plan"),
                        "ContentBlock::Text must never carry the encrypted envelope: {text}"
                    );
                }
                _ => {}
            }
        }
        assert_eq!(
            signature_carriers, 1,
            "exactly one internal signature must carry the envelope"
        );
    }

    #[test]
    fn reasoning_envelope_persistence_json_carries_it_only_inside_signature() {
        // The transcript/store serialization (what a log or the SQLite
        // message column sees) must carry the envelope exactly once, inside
        // the internal signature field — never in thinking or output text.
        let response: Response = serde_json::from_value(completed_response_json()).unwrap();
        let normalized = normalize_response(response).unwrap();
        let message = Message::new_blocks(Role::Assistant, normalized.blocks);
        let json = serde_json::to_string(&message).unwrap();
        assert_eq!(
            json.matches("encrypted-plan").count(),
            1,
            "persisted message JSON must contain the envelope exactly once: {json}"
        );

        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        let content = value["content"].as_array().unwrap();
        let thinking = content
            .iter()
            .find(|block| block["type"] == "thinking")
            .expect("thinking block");
        assert!(
            thinking["signature"]
                .as_str()
                .is_some_and(|signature| signature.contains("encrypted-plan")),
            "the envelope must live inside the signature field"
        );
        for block in content {
            if let Some(text) = block["text"].as_str() {
                assert!(
                    !text.contains("encrypted-plan"),
                    "serialized output text must not carry the envelope: {text}"
                );
            }
        }
    }

    #[test]
    fn reasoning_envelope_never_leaks_into_errors() {
        // An unrelated protocol error in the same response must not carry the
        // reasoning encrypted envelope.
        let mut value = completed_response_json();
        value["output"][2]["arguments"] = serde_json::json!("{");
        let error = normalize_response(serde_json::from_value(value).unwrap())
            .unwrap_err()
            .to_string();
        assert!(
            !error.contains("encrypted-plan"),
            "errors must never carry the reasoning envelope: {error}"
        );
    }
}
