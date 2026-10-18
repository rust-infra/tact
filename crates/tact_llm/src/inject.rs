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
/// Turns without stored thinking stay untouched; DeepSeek needs the stronger
/// [`inject_reasoning_content_backfill`].
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

/// Echo `reasoning_content` on DeepSeek tool-call turns: the stored text on the
/// **trailing** turn, `""` on every earlier one.
///
/// DeepSeek V4 thinking mode (on by default, and whenever `thinking` is sent)
/// rejects a request whose pending tool-call turn lacks the field with
/// `The reasoning_content in the thinking mode must be passed back to the API`.
/// Only some upstreams behind a gateway enforce it, so the identical request
/// succeeds or 400s depending on routing — omitting the field is not an option.
///
/// Only the turn next to the pending tool results is validated (a tool turn
/// further back in the history is accepted without the field, measured against
/// the strict upstream), and presence is what is checked, not content — `""` is
/// accepted. So history carries `""`: a field the API wants, at no token cost.
/// Replaying every turn's thinking would add ~100k prompt tokens on a long
/// session. The trailing turn keeps its real thinking — it is the one the model
/// is continuing from, and it is small.
pub(crate) fn inject_reasoning_content_backfill(body: &mut Value, reasoning: &[Option<String>]) {
    let Some(messages) = body["messages"].as_array_mut() else {
        return;
    };
    let has_tool_calls = |msg: &Value| {
        msg.get("tool_calls")
            .and_then(|v| v.as_array())
            .is_some_and(|calls| !calls.is_empty())
    };
    let trailing = messages.iter().rposition(|msg| {
        msg.get("role").and_then(|v| v.as_str()) == Some("assistant") && has_tool_calls(msg)
    });
    for (i, msg) in messages.iter_mut().enumerate() {
        if msg.get("role").and_then(|v| v.as_str()) != Some("assistant") || !has_tool_calls(msg) {
            continue;
        }
        let stored = reasoning.get(i).and_then(|r| r.as_deref());
        msg["reasoning_content"] = match (Some(i) == trailing, stored) {
            (true, Some(r)) => Value::String(r.to_owned()),
            _ => Value::String(String::new()),
        };
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
