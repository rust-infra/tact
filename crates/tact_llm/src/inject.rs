//! Shared Chat Completions body field helpers.

use serde_json::Value;

use crate::CreateMessageParams;

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

/// Budget tokens when thinking is enabled (budget > 0).
pub(crate) fn thinking_budget_enabled(request: &CreateMessageParams) -> Option<usize> {
    let thinking = request.thinking.as_ref()?;
    (thinking.budget_tokens > 0).then_some(thinking.budget_tokens)
}

/// Inject `reasoning_effort` from the per-request explicit effort.
///
/// `None` (unconfigured) omits the field — the provider default applies
/// (e.g. OpenAI medium). No budget-band fallback.
pub(crate) fn inject_openai_reasoning_effort(body: &mut Value, request: &CreateMessageParams) {
    if let Some(effort) = request.reasoning_effort {
        body["reasoning_effort"] = Value::String(effort.as_str().to_owned());
    }
}
