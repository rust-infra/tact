//! Live DeepSeek tests for `reasoning_content` echo rules.
//!
//! Official docs: https://api-docs.deepseek.com/guides/thinking_mode
//!
//! Scenarios covered:
//! 1. Plain multi-turn **without** echoing `reasoning_content` (docs: ignored / optional).
//! 2. Plain multi-turn **with** echoing `reasoning_content` (docs: ignored if present).
//! 3. Tool-call turn **without** echoing `reasoning_content` (docs: should 400).
//! 4. Tool-call turn **with** echoing `reasoning_content` (docs: required).
//! 5. Tool history with **only the latest** `reasoning_content` kept.
//!
//! Skips when `DEEPSEEK_API_KEY` is unset or empty.
//! Optional: `DEEPSEEK_BASE_URL` (default `https://api.deepseek.com`),
//! `DEEPSEEK_MODEL` (default `deepseek-v4-flash`).
//!
//!   cargo test -p tact_llm deepseek_reasoning -- --nocapture

use serde_json::{Map, Value, json};

fn skip_unless_api_key() -> Option<(String, String, String)> {
    dotenvy::dotenv().ok();
    let api_key = match std::env::var("DEEPSEEK_API_KEY") {
        Ok(k) if !k.trim().is_empty() => k,
        _ => {
            eprintln!("skipping: DEEPSEEK_API_KEY not set");
            return None;
        }
    };
    let base_url = std::env::var("DEEPSEEK_BASE_URL")
        .unwrap_or_else(|_| "https://api.deepseek.com".to_string());
    let model = std::env::var("DEEPSEEK_MODEL").unwrap_or_else(|_| "deepseek-v4-flash".to_string());
    Some((api_key, base_url, model))
}

fn thinking_enabled() -> Map<String, Value> {
    json!({
        "thinking": { "type": "enabled" },
        "reasoning_effort": "high",
    })
    .as_object()
    .cloned()
    .expect("thinking object")
}

fn with_thinking(mut body: Value) -> Value {
    let obj = body.as_object_mut().expect("request body object");
    obj.extend(thinking_enabled());
    body
}

fn date_tool() -> Value {
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

async fn chat_completions(
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

fn assistant_message(choice: &Value) -> Value {
    choice["choices"][0]["message"].clone()
}

fn reasoning_of(msg: &Value) -> Option<&str> {
    msg.get("reasoning_content")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
}

fn has_tool_calls(msg: &Value) -> bool {
    msg.get("tool_calls")
        .and_then(|v| v.as_array())
        .is_some_and(|a| !a.is_empty())
}

fn strip_reasoning(mut msg: Value) -> Value {
    if let Some(obj) = msg.as_object_mut() {
        obj.remove("reasoning_content");
    }
    msg
}

/// Keep `reasoning_content` only on the last assistant message that has it;
/// strip it from every earlier assistant message.
fn keep_latest_reasoning_only(messages: &mut [Value]) {
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

fn count_reasoning(messages: &[Value]) -> usize {
    messages
        .iter()
        .filter(|m| reasoning_of(m).is_some())
        .count()
}

/// Scenario 1+2: plain multi-turn with and without echoing `reasoning_content`.
#[tokio::test]
async fn deepseek_reasoning_plain_multiturn_echo_optional() {
    let Some((api_key, base_url, model)) = skip_unless_api_key() else {
        return;
    };

    // Turn 1: simple question → expect reasoning_content in response.
    let turn1_body = with_thinking(json!({
        "model": model,
        "messages": [{"role": "user", "content": "Reply with exactly one word: ping"}],
        "max_tokens": 256,
        "stream": false,
    }));

    let (status1, resp1) = chat_completions(&api_key, &base_url, &turn1_body)
        .await
        .expect("turn1 request");
    assert!(status1.is_success(), "turn1 failed: {status1} {resp1}");
    let asst1 = assistant_message(&resp1);
    let rc1 = reasoning_of(&asst1);
    println!(
        "turn1 ok; reasoning_content present={} len={}",
        rc1.is_some(),
        rc1.map(str::len).unwrap_or(0)
    );
    assert!(
        rc1.is_some(),
        "expected reasoning_content in thinking mode: {asst1}"
    );

    // Turn 2a: omit reasoning_content (docs: should be fine / ignored).
    let turn2_omit = with_thinking(json!({
        "model": model,
        "messages": [
            {"role": "user", "content": "Reply with exactly one word: ping"},
            strip_reasoning(asst1.clone()),
            {"role": "user", "content": "Now reply with exactly one word: pong"},
        ],
        "max_tokens": 256,
        "stream": false,
    }));

    let (status_omit, resp_omit) = chat_completions(&api_key, &base_url, &turn2_omit)
        .await
        .expect("turn2 omit request");
    println!("plain omit reasoning_content → {status_omit}");
    assert!(
        status_omit.is_success(),
        "plain multiturn without reasoning_content should succeed: {status_omit} {resp_omit}"
    );

    // Turn 2b: echo reasoning_content (docs: ignored but accepted).
    let turn2_echo = with_thinking(json!({
        "model": model,
        "messages": [
            {"role": "user", "content": "Reply with exactly one word: ping"},
            asst1,
            {"role": "user", "content": "Now reply with exactly one word: pong"},
        ],
        "max_tokens": 256,
        "stream": false,
    }));

    let (status_echo, resp_echo) = chat_completions(&api_key, &base_url, &turn2_echo)
        .await
        .expect("turn2 echo request");
    println!("plain echo reasoning_content → {status_echo}");
    assert!(
        status_echo.is_success(),
        "plain multiturn with reasoning_content should succeed: {status_echo} {resp_echo}"
    );
}

/// Scenario 3+4: tool-call turns with and without echoing `reasoning_content`.
#[tokio::test]
async fn deepseek_reasoning_tool_call_echo_required_or_not() {
    let Some((api_key, base_url, model)) = skip_unless_api_key() else {
        return;
    };

    // Turn 1: ask for a tool call (thinking mode rejects forced tool_choice).
    let turn1_body = with_thinking(json!({
        "model": model,
        "messages": [{
            "role": "user",
            "content": "Call the get_date tool now. Do not answer from memory — only via the tool."
        }],
        "tools": [date_tool()],
        "tool_choice": "auto",
        "max_tokens": 512,
        "stream": false,
    }));

    let (status1, resp1) = chat_completions(&api_key, &base_url, &turn1_body)
        .await
        .expect("tool turn1 request");
    assert!(status1.is_success(), "tool turn1 failed: {status1} {resp1}");
    let asst1 = assistant_message(&resp1);
    let rc1 = reasoning_of(&asst1);
    let tool_calls = asst1
        .get("tool_calls")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    println!(
        "tool turn1 ok; reasoning_content present={} len={} tool_calls={}",
        rc1.is_some(),
        rc1.map(str::len).unwrap_or(0),
        tool_calls.len()
    );
    if tool_calls.is_empty() {
        eprintln!(
            "skipping tool-echo scenarios: model did not emit tool_calls \
             (thinking mode + auto tool_choice). response={asst1}"
        );
        return;
    }
    assert!(
        rc1.is_some(),
        "expected reasoning_content alongside tool_calls: {asst1}"
    );

    let tool_call_id = tool_calls[0]["id"].as_str().unwrap_or("call_0");
    let tool_result = json!({
        "role": "tool",
        "tool_call_id": tool_call_id,
        "content": "2026-07-16",
    });
    let user0 = json!({
        "role": "user",
        "content": "Call the get_date tool now. Do not answer from memory — only via the tool."
    });

    // Follow-up WITHOUT reasoning_content (docs claim 400; historical tact data
    // succeeded without it — assert the observed status and print it).
    let without_rc = with_thinking(json!({
        "model": model,
        "messages": [user0.clone(), strip_reasoning(asst1.clone()), tool_result.clone()],
        "tools": [date_tool()],
        "max_tokens": 512,
        "stream": false,
    }));

    let (status_without, resp_without) = chat_completions(&api_key, &base_url, &without_rc)
        .await
        .expect("tool followup without rc");
    println!(
        "tool followup WITHOUT reasoning_content → {status_without} body={}",
        serde_json::to_string(&resp_without).unwrap_or_default()
    );

    // Follow-up WITH reasoning_content (docs: required; must succeed).
    let with_rc = with_thinking(json!({
        "model": model,
        "messages": [user0, asst1, tool_result],
        "tools": [date_tool()],
        "max_tokens": 512,
        "stream": false,
    }));

    let (status_with, resp_with) = chat_completions(&api_key, &base_url, &with_rc)
        .await
        .expect("tool followup with rc");
    println!("tool followup WITH reasoning_content → {status_with}");
    assert!(
        status_with.is_success(),
        "tool followup with reasoning_content must succeed: {status_with} {resp_with}"
    );

    // Document the without-rc behavior as an explicit assertion so CI records
    // the contract we observed. Prefer: without_rc fails XOR both succeed —
    // never "with fails while without succeeds".
    if status_without.is_success() {
        println!(
            "NOTE: DeepSeek accepted tool followup WITHOUT reasoning_content \
             (docs say 400; live API currently allows it)."
        );
    } else {
        assert!(
            status_without.as_u16() == 400,
            "expected HTTP 400 when omitting reasoning_content on tool turns, got {status_without}: {resp_without}"
        );
        let err = resp_without.to_string().to_lowercase();
        assert!(
            err.contains("reasoning_content") || err.contains("400"),
            "expected reasoning_content-related error: {resp_without}"
        );
    }
}

/// Scenario 5: after two tool rounds, keep only the latest assistant's thinking.
#[tokio::test]
async fn deepseek_reasoning_tool_call_echo_latest_only() {
    let Some((api_key, base_url, model)) = skip_unless_api_key() else {
        return;
    };

    let user0 = json!({
        "role": "user",
        "content": "Call the get_date tool now. Do not answer from memory — only via the tool."
    });

    // Round 1: force a tool call, then feed the tool result to get a final answer.
    let round1 = with_thinking(json!({
        "model": model,
        "messages": [user0.clone()],
        "tools": [date_tool()],
        "tool_choice": "auto",
        "max_tokens": 512,
        "stream": false,
    }));
    let (s1, r1) = chat_completions(&api_key, &base_url, &round1)
        .await
        .expect("round1");
    assert!(s1.is_success(), "round1 failed: {s1} {r1}");
    let asst1 = assistant_message(&r1);
    if !has_tool_calls(&asst1) || reasoning_of(&asst1).is_none() {
        eprintln!("skipping latest-only: round1 missing tool_calls/reasoning: {asst1}");
        return;
    }
    let tool1 = json!({
        "role": "tool",
        "tool_call_id": asst1["tool_calls"][0]["id"],
        "content": "2026-07-16",
    });

    let round1_finish = with_thinking(json!({
        "model": model,
        "messages": [user0.clone(), asst1.clone(), tool1.clone()],
        "tools": [date_tool()],
        "max_tokens": 512,
        "stream": false,
    }));
    let (s1b, r1b) = chat_completions(&api_key, &base_url, &round1_finish)
        .await
        .expect("round1 finish");
    assert!(s1b.is_success(), "round1 finish failed: {s1b} {r1b}");
    let asst2 = assistant_message(&r1b);
    println!(
        "round1 done; asst1_rc={} asst2_rc={} asst2_tools={}",
        reasoning_of(&asst1).map(str::len).unwrap_or(0),
        reasoning_of(&asst2).map(str::len).unwrap_or(0),
        has_tool_calls(&asst2)
    );

    // Round 2: another tool call on top of history.
    let user1 = json!({
        "role": "user",
        "content": "Call get_date again. Use the tool; do not reuse the previous answer."
    });
    let mut history = vec![user0, asst1, tool1, asst2, user1.clone()];
    let round2 = with_thinking(json!({
        "model": model,
        "messages": history.clone(),
        "tools": [date_tool()],
        "tool_choice": "auto",
        "max_tokens": 512,
        "stream": false,
    }));
    let (s2, r2) = chat_completions(&api_key, &base_url, &round2)
        .await
        .expect("round2");
    assert!(s2.is_success(), "round2 failed: {s2} {r2}");
    let asst3 = assistant_message(&r2);
    if !has_tool_calls(&asst3) || reasoning_of(&asst3).is_none() {
        eprintln!("skipping latest-only: round2 missing tool_calls/reasoning: {asst3}");
        return;
    }
    let tool2 = json!({
        "role": "tool",
        "tool_call_id": asst3["tool_calls"][0]["id"],
        "content": "2026-07-17",
    });
    history.push(asst3);
    history.push(tool2);

    let before = count_reasoning(&history);
    keep_latest_reasoning_only(&mut history);
    let after = count_reasoning(&history);
    println!("reasoning_content count before keep_latest={before} after={after}");
    assert_eq!(
        after, 1,
        "expected exactly one reasoning_content after keep_latest"
    );
    assert!(
        before >= 2,
        "need at least two historical thinkings to exercise latest-only (got {before})"
    );

    let latest_only = with_thinking(json!({
        "model": model,
        "messages": history,
        "tools": [date_tool()],
        "max_tokens": 512,
        "stream": false,
    }));
    let (status, resp) = chat_completions(&api_key, &base_url, &latest_only)
        .await
        .expect("latest-only followup");
    println!(
        "tool history with ONLY latest reasoning_content → {status} body={}",
        serde_json::to_string(&resp).unwrap_or_default()
    );
    assert!(
        status.is_success(),
        "echoing only the latest reasoning_content should succeed: {status} {resp}"
    );
}
