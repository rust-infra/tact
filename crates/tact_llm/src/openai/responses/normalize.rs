use std::collections::BTreeMap;

use async_openai_responses::types::responses::{
    OutputItem, OutputMessageContent, OutputStatus, Response, Status, SummaryPart,
};
use tact_protocol::TokenUsageInfo;

use super::history;
use crate::{ContentBlock, LlmError, StopReason};

#[derive(Debug)]
pub(crate) struct NormalizedResponse {
    pub blocks: Vec<ContentBlock>,
    pub stop_reason: Option<StopReason>,
    pub usage: Option<TokenUsageInfo>,
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
        },
        Status::Incomplete => {
            let reason = response.incomplete_details.as_ref().map(|details| details.reason.as_str());
            match reason {
                Some("max_output_tokens") => Ok(Some(StopReason::MaxTokens)),
                Some("content_filter") => Ok(Some(StopReason::StopSequence)),
                Some(other) => {
                    Err(LlmError::Other(format!("OpenAI Responses incomplete for unsupported reason '{other}'")))
                },
                None => Err(LlmError::Other("OpenAI Responses incomplete without a reason".to_string())),
            }
        },
        Status::Failed => {
            let detail = response
                .error
                .as_ref()
                .map(|error| format!("{}: {}", error.code, error.message))
                .unwrap_or_else(|| "response failed without error details".to_string());
            Err(LlmError::Other(format!("OpenAI Responses failed: {detail}")))
        },
        Status::Cancelled => Err(LlmError::Other("OpenAI Responses request cancelled".into())),
        Status::InProgress | Status::Queued => {
            Err(LlmError::Other(format!("OpenAI Responses ended with non-terminal status {:?}", response.status)))
        },
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
            OutputItem::FunctionCall(call) => call.id.as_ref().map(|item_id| (call.call_id.clone(), item_id.clone())),
            _ => None,
        })
        .collect();

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
                let signature = if reasoning.encrypted_content.as_ref().is_some_and(|encrypted| !encrypted.is_empty()) {
                    history::encode(reasoning.clone(), function_call_item_ids.clone())?
                } else {
                    String::new()
                };
                if !thinking.is_empty() || !signature.is_empty() {
                    blocks.push(ContentBlock::Thinking { thinking, signature });
                }
            },
            OutputItem::Message(message) => {
                for content in &message.content {
                    match content {
                        OutputMessageContent::OutputText(output) => {
                            if !output.text.is_empty() {
                                blocks.push(ContentBlock::Text { text: output.text.clone() });
                            }
                        },
                        OutputMessageContent::Refusal(refusal) => {
                            has_refusal = true;
                            if !refusal.refusal.is_empty() {
                                blocks.push(ContentBlock::Text { text: refusal.refusal.clone() });
                            }
                        },
                    }
                }
            },
            OutputItem::FunctionCall(call) => {
                if call.status != Some(OutputStatus::Completed) {
                    continue;
                }
                let input = serde_json::from_str(&call.arguments).map_err(|error| {
                    LlmError::Other(format!(
                        "parse arguments for OpenAI function '{}' call '{}': {error}",
                        call.name, call.call_id
                    ))
                })?;
                has_tools = true;
                blocks.push(ContentBlock::ToolUse { id: call.call_id.clone(), name: call.name.clone(), input });
            },
            _ => {},
        }
    }

    let stop_reason = terminal_stop_reason(&response, has_tools, has_refusal)?;
    let usage = response.usage.as_ref().map(|usage| TokenUsageInfo {
        prompt: usage.input_tokens,
        completion: usage.output_tokens,
        total: usage.total_tokens,
        prompt_cache_hit_tokens: usage.input_tokens_details.cached_tokens,
        prompt_cache_miss_tokens: usage.input_tokens.saturating_sub(usage.input_tokens_details.cached_tokens),
        reasoning_tokens: usage.output_tokens_details.reasoning_tokens,
    });

    Ok(NormalizedResponse { blocks, stop_reason, usage })
}

#[cfg(test)]
pub(crate) mod tests {
    use async_openai_responses::types::responses::Response;

    use super::normalize_response;
    use crate::{ContentBlock, CreateMessageParams, Message, RequiredMessageParams, Role, StopReason};

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
        value["incomplete_details"] =
            reason.map(|reason| serde_json::json!({"reason": reason})).unwrap_or(serde_json::Value::Null);
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
        let normalized = normalize_response(response_with_status("incomplete", Some("max_output_tokens"))).unwrap();
        assert_eq!(normalized.stop_reason, Some(StopReason::MaxTokens));
    }

    #[test]
    fn incomplete_max_output_tokens_with_tool_still_maps_to_max_tokens() {
        let mut value = completed_response_json();
        value["status"] = serde_json::json!("incomplete");
        value["incomplete_details"] = serde_json::json!({"reason": "max_output_tokens"});

        let normalized = normalize_response(serde_json::from_value(value).unwrap()).unwrap();
        assert!(normalized.blocks.iter().any(|block| matches!(block, ContentBlock::ToolUse { .. })));
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

        let error = normalize_response(serde_json::from_value(value).unwrap()).unwrap_err().to_string();
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
        assert!(normalized.blocks.iter().all(|block| !matches!(block, ContentBlock::ToolUse { .. })));
        assert_eq!(normalized.stop_reason, Some(StopReason::MaxTokens));
    }

    #[test]
    fn unknown_incomplete_reason_is_an_error() {
        let error = normalize_response(response_with_status("incomplete", Some("new_reason"))).unwrap_err().to_string();
        assert!(error.contains("new_reason"));
    }

    #[test]
    fn malformed_function_arguments_return_contextual_error() {
        let mut value = completed_response_json();
        value["output"][2]["arguments"] = serde_json::json!("{");
        let error = normalize_response(serde_json::from_value(value).unwrap()).unwrap_err().to_string();
        assert!(error.contains("bash"));
        assert!(error.contains("call_1"));
    }

    #[test]
    fn response_output_item_ids_survive_tact_history_round_trip() {
        let mut value = completed_response_json();
        value["output"].as_array_mut().unwrap().push(serde_json::json!({
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

        let body =
            serde_json::to_value(super::super::convert::create_response(&request, None).expect("round-trip request"))
                .unwrap();
        let input = body["input"].as_array().unwrap();
        let reasoning = input.iter().find(|item| item["type"] == "reasoning").unwrap();
        let function_call = input.iter().find(|item| item["type"] == "function_call").unwrap();
        assert_eq!(reasoning["id"], "rs_1");
        assert_eq!(function_call["id"], "fc_1");
        let second_call = input.iter().find(|item| item["call_id"] == "call_2").unwrap();
        assert_eq!(second_call["id"], "fc_2");
    }
}
