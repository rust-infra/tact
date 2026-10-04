//! Helpers the live `reasoning_content` tests share.
//!
//! `test_deepseek_reasoning` and `test_kimi_reasoning` differ in two things,
//! and both are the point of having two files: which environment names the
//! endpoint, and which `thinking` shape goes on the request. Everything they do
//! to a *response* is the same — pull the assistant message out of a choice,
//! read `reasoning_content`, strip it, keep only the latest — and those eight
//! helpers had been copied verbatim into both files.
//!
//! Test-only, like the tests themselves: nothing here is compiled into the
//! library.

use serde_json::{Value, json};

pub(crate) fn date_tool() -> Value {
    json!({
        "type": "function",
        "function": {
            "name": "get_date",
            "description": "Get today's date as YYYY-mm-dd",
            "parameters": {
                "type": "object",
                "properties": {},
            }
        }
    })
}

pub(crate) async fn chat_completions(
    api_key: &str,
    base_url: &str,
    body: &Value,
) -> Result<(reqwest::StatusCode, Value), String> {
    let url = format!("{}/chat/completions", base_url.trim_end_matches('/'));
    let client = reqwest::Client::new();
    let resp = client
        .post(&url)
        .bearer_auth(api_key)
        .header("content-type", "application/json")
        .json(body)
        .send()
        .await
        .map_err(|e| format!("request failed: {e}"))?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    let json: Value = serde_json::from_str(&text).unwrap_or_else(|_| json!({ "raw": text }));
    Ok((status, json))
}

pub(crate) fn assistant_message(choice: &Value) -> Value {
    choice["choices"][0]["message"].clone()
}

pub(crate) fn reasoning_of(msg: &Value) -> Option<&str> {
    msg.get("reasoning_content")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
}

pub(crate) fn has_tool_calls(msg: &Value) -> bool {
    msg.get("tool_calls")
        .and_then(|v| v.as_array())
        .is_some_and(|a| !a.is_empty())
}

pub(crate) fn strip_reasoning(mut msg: Value) -> Value {
    if let Some(obj) = msg.as_object_mut() {
        obj.remove("reasoning_content");
    }
    msg
}

/// Keep `reasoning_content` only on the last assistant message that has it;
/// strip it from every earlier assistant message.
pub(crate) fn keep_latest_reasoning_only(messages: &mut [Value]) {
    let last = messages.iter().rposition(|m| {
        m.get("role").and_then(|r| r.as_str()) == Some("assistant") && reasoning_of(m).is_some()
    });
    for (i, msg) in messages.iter_mut().enumerate() {
        if Some(i) != last
            && let Some(obj) = msg.as_object_mut()
        {
            obj.remove("reasoning_content");
        }
    }
}

pub(crate) fn count_reasoning(messages: &[Value]) -> usize {
    messages
        .iter()
        .filter(|m| reasoning_of(m).is_some())
        .count()
}
