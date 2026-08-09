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

/// Normalizes a `web_search_call` item so the typed parser accepts it even
/// when a compatible endpoint emits the search action with a `queries` array
/// instead of the singular `query` string that async-openai 0.41.x models
/// (e.g. DeepSeek Responses returns `action.search.queries`). When `query`
/// is absent, it is filled from the first entry of `queries`; items that
/// already carry a `query` are left untouched. This is a compatibility shim
/// only — the raw item JSON is preserved for replay, so the provider still
/// receives its own wire shape on follow-up turns.
pub(crate) fn normalize_web_search_call_query(item: &mut Value) {
    if item.get("type").and_then(Value::as_str) != Some("web_search_call") {
        return;
    }
    let Some(action) = item.get_mut("action") else {
        return;
    };
    if action.get("type").and_then(Value::as_str) != Some("search") {
        return;
    }
    if action
        .get("query")
        .and_then(Value::as_str)
        .is_some_and(|query| !query.is_empty())
    {
        return;
    }
    if let Some(first) = action
        .get("queries")
        .and_then(Value::as_array)
        .and_then(|queries| queries.iter().find_map(Value::as_str))
    {
        action["query"] = Value::String(first.to_string());
    }
}

pub(crate) fn parse_response_envelope(value: Value) -> Result<RawResponseEnvelope, LlmError> {
    let raw_items = raw_output_items(&value)?;
    let mut typed_items = Vec::with_capacity(raw_items.len());
    let mut unknown_output_items = Vec::new();
    // Known items after wire normalization: used for the typed surrogate so
    // the `Response` envelope deserializes even when a compatible endpoint
    // emitted `queries` instead of `query` on a web-search action.
    let mut known_items = Vec::with_capacity(raw_items.len());

    for item in &raw_items {
        let mut item = item.clone();
        let type_name = item
            .get("type")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| {
                LlmError::Unsupported("OpenAI Responses output item is missing type".to_string())
            })?;
        normalize_web_search_call_query(&mut item);
        match serde_json::from_value::<OutputItem>(item.clone()) {
            Ok(parsed) => {
                typed_items.push(parsed);
                known_items.push(item);
            }
            Err(_error) if !known_output_type(&type_name) => {
                unknown_output_items.push(item);
            }
            Err(error) => {
                return Err(LlmError::Unsupported(format!(
                    "deserialize known OpenAI Responses output item '{type_name}': {error}"
                )));
            }
        }
    }

    let mut typed_value = value.clone();
    typed_value["output"] = Value::Array(known_items);
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
        output_items: raw_items,
        unknown_output_items,
    })
}

#[cfg(test)]
mod tests {
    use super::{normalize_web_search_call_query, parse_response_envelope};
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

    fn response_with_queries_web_search_call() -> serde_json::Value {
        serde_json::json!({
            "created_at": 1,
            "completed_at": 2,
            "id": "resp_ws",
            "object": "response",
            "status": "completed",
            "model": "gpt-5",
            "output": [
                {"type": "web_search_call", "id": "ws-1", "status": "completed",
                 "action": {"type": "search", "queries": ["Tokyo weather", "ws_call_id=ws-1"]}}
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

    #[test]
    fn web_search_call_with_queries_is_normalized_for_typed_parse() {
        let parsed = parse_response_envelope(response_with_queries_web_search_call()).unwrap();
        // The typed envelope parses (query filled from `queries`), so the
        // known web_search_call item is not treated as unknown.
        assert!(parsed.unknown_output_items.is_empty());
        let OutputItem::WebSearchCall(call) = &parsed.typed.output[0] else {
            panic!("expected WebSearchCall output item");
        };
        assert_eq!(call.id, "ws-1");
        assert_eq!(
            call.action.as_ref().and_then(|action| match action {
                async_openai_responses::types::responses::WebSearchToolCallAction::Search(
                    search,
                ) => Some(search.query.as_str()),
                _ => None,
            }),
            Some("Tokyo weather")
        );
        // The raw wire items are preserved verbatim (queries, not query) so
        // follow-up turns replay the provider's own shape.
        assert_eq!(
            parsed.output_items[0]["action"]["queries"][0],
            "Tokyo weather"
        );
        assert!(parsed.output_items[0]["action"].get("query").is_none());
    }

    #[test]
    fn normalize_web_search_call_query_keeps_existing_query() {
        let mut item = serde_json::json!({
            "type": "web_search_call",
            "id": "ws-1",
            "status": "completed",
            "action": {"type": "search", "query": "Rust async", "queries": ["ignored"]}
        });
        normalize_web_search_call_query(&mut item);
        assert_eq!(item["action"]["query"], "Rust async");
    }

    #[test]
    fn normalize_web_search_call_query_ignores_non_search_actions() {
        let mut item = serde_json::json!({
            "type": "web_search_call",
            "id": "ws-1",
            "status": "completed",
            "action": {"type": "open_page", "url": "https://example.com"}
        });
        normalize_web_search_call_query(&mut item);
        assert!(item["action"].get("query").is_none());
    }
}
