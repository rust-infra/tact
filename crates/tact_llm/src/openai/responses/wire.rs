use async_openai_responses::types::responses::{OutputItem, Response};
use serde_json::Value;

use crate::LlmError;

pub(crate) struct RawResponseEnvelope {
    pub(crate) value: Value,
    pub(crate) typed: Response,
    pub(crate) output_items: Vec<Value>,
    pub(crate) unknown_output_items: Vec<Value>,
}

pub(crate) fn raw_output_items(value: &Value) -> Result<Vec<Value>, LlmError> {
    let object = value.as_object().ok_or_else(|| {
        LlmError::Unsupported("OpenAI Responses response must be a JSON object".to_string())
    })?;
    let output = object.get("output").ok_or_else(|| {
        LlmError::Unsupported("OpenAI Responses response is missing output".to_string())
    })?;
    let output = output.as_array().ok_or_else(|| {
        LlmError::Unsupported("OpenAI Responses response output must be an array".to_string())
    })?;
    Ok(output.clone())
}

fn known_output_type(type_name: &str) -> bool {
    matches!(
        type_name,
        "message"
            | "file_search_call"
            | "function_call"
            | "function_call_output"
            | "web_search_call"
            | "computer_call"
            | "computer_call_output"
            | "reasoning"
            | "compaction"
            | "image_generation_call"
            | "code_interpreter_call"
            | "local_shell_call"
            | "shell_call"
            | "shell_call_output"
            | "apply_patch_call"
            | "apply_patch_call_output"
            | "mcp_call"
            | "mcp_list_tools"
            | "mcp_approval_request"
            | "custom_tool_call"
            | "custom_tool_call_output"
            | "tool_search_call"
            | "tool_search_output"
    )
}

pub(crate) fn parse_response_envelope(value: Value) -> Result<RawResponseEnvelope, LlmError> {
    let output_items = raw_output_items(&value)?;
    let mut typed_items = Vec::with_capacity(output_items.len());
    let mut unknown_output_items = Vec::new();

    for item in &output_items {
        let type_name = item.get("type").and_then(Value::as_str).ok_or_else(|| {
            LlmError::Unsupported("OpenAI Responses output item is missing type".to_string())
        })?;
        match serde_json::from_value::<OutputItem>(item.clone()) {
            Ok(parsed) => typed_items.push(parsed),
            Err(_error) if !known_output_type(type_name) => {
                unknown_output_items.push(item.clone());
            }
            Err(error) => {
                return Err(LlmError::Unsupported(format!(
                    "deserialize known OpenAI Responses output item '{type_name}': {error}"
                )));
            }
        }
    }

    let mut typed_value = value.clone();
    typed_value["output"] = Value::Array(
        output_items
            .iter()
            .filter(|item| {
                item.get("type")
                    .and_then(Value::as_str)
                    .is_some_and(known_output_type)
            })
            .cloned()
            .collect(),
    );
    let typed: Response = serde_json::from_value(typed_value).map_err(|error| {
        LlmError::Unsupported(format!(
            "deserialize OpenAI Responses response envelope: {error}"
        ))
    })?;

    // Keep the parsed values alive in the same order as the raw wire response;
    // the typed surrogate is only for known-item normalization.
    debug_assert_eq!(typed.output.len(), typed_items.len());
    Ok(RawResponseEnvelope {
        value,
        typed,
        output_items,
        unknown_output_items,
    })
}

#[cfg(test)]
mod tests {
    use super::parse_response_envelope;
    use async_openai_responses::types::responses::OutputItem;

    fn response_with_unknown_output_item() -> serde_json::Value {
        serde_json::json!({
            "created_at": 1,
            "completed_at": 2,
            "id": "resp_unknown_item",
            "object": "response",
            "status": "completed",
            "model": "gpt-5",
            "output": [
                {"type": "future_item", "id": "future-1", "payload": {"x": 1}},
                {"type": "message", "id": "msg-1", "status": "completed", "role": "assistant",
                 "content": [{"type": "output_text", "text": "hello", "annotations": []}]}
            ],
            "usage": {"input_tokens": 1, "input_tokens_details": {"cached_tokens": 0},
                      "output_tokens": 1, "output_tokens_details": {"reasoning_tokens": 0},
                      "total_tokens": 2}
        })
    }

    #[test]
    fn unknown_output_item_is_retained_before_typed_normalization() {
        let parsed = parse_response_envelope(response_with_unknown_output_item()).unwrap();
        assert_eq!(parsed.value["id"], "resp_unknown_item");
        assert_eq!(parsed.output_items.len(), 2);
        assert_eq!(
            parsed.unknown_output_items,
            vec![serde_json::json!({
                "type": "future_item", "id": "future-1", "payload": {"x": 1}
            })]
        );
        assert!(matches!(parsed.typed.output[0], OutputItem::Message(_)));
    }
}
