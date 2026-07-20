//! Shared Chat Completions body field helpers.

use serde_json::Value;

use crate::CreateMessageParams;
use crate::openai::reasoning_effort_from_budget;

/// Inject `user_id` into the request body for KV cache isolation.
pub(crate) fn inject_user_id(body: &mut Value, user_id: Option<&str>) {
    if let Some(uid) = user_id {
        body["user_id"] = Value::String(uid.to_owned());
    }
}

/// Echo historical `reasoning_content` on assistant messages.
///
/// Required for Kimi tool-call / Preserved Thinking turns (otherwise 400).
/// DeepSeek deliberately does **not** call this: live API accepts tool turns
/// without echo, and omitting it keeps the prompt prefix stable for KV cache.
pub(crate) fn inject_reasoning_content(body: &mut Value, reasoning: &[Option<String>]) {
    let Some(messages) = body["messages"].as_array_mut() else {
        return;
    };
    for (i, msg) in messages.iter_mut().enumerate() {
        if let Some(Some(r)) = reasoning.get(i)
            && msg.get("role").and_then(|v| v.as_str()) == Some("assistant")
        {
            msg["reasoning_content"] = Value::String(r.clone());
        }
    }
}

/// Budget tokens when thinking is enabled and maps to a non-empty effort band.
pub(crate) fn thinking_budget_enabled(request: &CreateMessageParams) -> Option<usize> {
    let thinking = request.thinking.as_ref()?;
    reasoning_effort_from_budget(thinking.budget_tokens).map(|_| thinking.budget_tokens)
}

/// OpenAI-style bands: `low` / `medium` / `high`.
pub(crate) fn inject_openai_reasoning_effort(
    body: &mut Value,
    request: &CreateMessageParams,
    configured: Option<crate::OpenAiReasoningEffort>,
) {
    let Some(thinking) = &request.thinking else {
        if let Some(effort) = configured {
            body["reasoning_effort"] = Value::String(effort.as_str().to_owned());
        }
        return;
    };
    if let Some(effort) = crate::effective_reasoning_effort(configured, thinking.budget_tokens) {
        body["reasoning_effort"] = Value::String(effort.as_str().to_owned());
    }
}
