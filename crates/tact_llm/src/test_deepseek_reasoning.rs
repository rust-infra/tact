//! Live DeepSeek tests for `reasoning_content` echo rules.
//!
//! Official docs: https://api-docs.deepseek.com/guides/thinking_mode
//!
//! Scenarios covered:
//! 1. Plain multi-turn **without** echoing `reasoning_content` (docs: ignored / optional).
//! 2. Plain multi-turn **with** echoing `reasoning_content` (docs: ignored if present).
//! 3. Tool-call turn **without** echoing `reasoning_content` (docs: should 400).
//! 4. Tool-call turn **with** echoing `reasoning_content` (docs: required).
//!
//! Skips when `DEEPSEEK_API_KEY` is unset or empty.
//! Optional: `DEEPSEEK_BASE_URL` (default `https://api.deepseek.com`),
//! `DEEPSEEK_MODEL` (default `deepseek-v4-flash`).
//!
//!   cargo test -p tact_llm deepseek_reasoning -- --nocapture

use serde_json::{Value, json};

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
    let model =
        std::env::var("DEEPSEEK_MODEL").unwrap_or_else(|_| "deepseek-v4-flash".to_string());
    Some((api_key, base_url, model))
}

fn thinking_enabled() -> Value {
    json!({
        "thinking": { "type": "enabled" },
        "reasoning_effort": "high",
    })
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
    let url = format!(
        "{}/chat/completions",
        base_url.trim_end_matches('/')
    );
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

fn strip_reasoning(mut msg: Value) -> Value {
    if let Some(obj) = msg.as_object_mut() {
        obj.remove("reasoning_content");
    }
    msg
}

/// Scenario 1+2: plain multi-turn with and without echoing `reasoning_content`.
#[tokio::test]
async fn deepseek_reasoning_plain_multiturn_echo_optional() {
    let Some((api_key, base_url, model)) = skip_unless_api_key() else {
        return;
    };

    // Turn 1: simple question → expect reasoning_content in response.
    let turn1_body = json!({
        "model": model,
        "messages": [{"role": "user", "content": "Reply with exactly one word: ping"}],
        "max_tokens": 256,
        "stream": false,
    })
    .as_object()
    .cloned()
    .map(|mut m| {
        m.extend(thinking_enabled().as_object().cloned().unwrap());
        Value::Object(m)
    })
    .unwrap();

    let (status1, resp1) = chat_completions(&api_key, &base_url, &turn1_body)
        .await
        .expect("turn1 request");
    assert!(
        status1.is_success(),
        "turn1 failed: {status1} {resp1}"
    );
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
    let turn2_omit = json!({
        "model": model,
        "messages": [
            {"role": "user", "content": "Reply with exactly one word: ping"},
            strip_reasoning(asst1.clone()),
            {"role": "user", "content": "Now reply with exactly one word: pong"},
        ],
        "max_tokens": 256,
        "stream": false,
    })
    .as_object()
    .cloned()
    .map(|mut m| {
        m.extend(thinking_enabled().as_object().cloned().unwrap());
        Value::Object(m)
    })
    .unwrap();

    let (status_omit, resp_omit) = chat_completions(&api_key, &base_url, &turn2_omit)
        .await
        .expect("turn2 omit request");
    println!("plain omit reasoning_content → {status_omit}");
    assert!(
        status_omit.is_success(),
        "plain multiturn without reasoning_content should succeed: {status_omit} {resp_omit}"
    );

    // Turn 2b: echo reasoning_content (docs: ignored but accepted).
    let turn2_echo = json!({
        "model": model,
        "messages": [
            {"role": "user", "content": "Reply with exactly one word: ping"},
            asst1,
            {"role": "user", "content": "Now reply with exactly one word: pong"},
        ],
        "max_tokens": 256,
        "stream": false,
    })
    .as_object()
    .cloned()
    .map(|mut m| {
        m.extend(thinking_enabled().as_object().cloned().unwrap());
        Value::Object(m)
    })
    .unwrap();

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
    let turn1_body = json!({
        "model": model,
        "messages": [{
            "role": "user",
            "content": "Call the get_date tool now. Do not answer from memory — only via the tool."
        }],
        "tools": [date_tool()],
        "tool_choice": "auto",
        "max_tokens": 512,
        "stream": false,
    })
    .as_object()
    .cloned()
    .map(|mut m| {
        m.extend(thinking_enabled().as_object().cloned().unwrap());
        Value::Object(m)
    })
    .unwrap();

    let (status1, resp1) = chat_completions(&api_key, &base_url, &turn1_body)
        .await
        .expect("tool turn1 request");
    assert!(
        status1.is_success(),
        "tool turn1 failed: {status1} {resp1}"
    );
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
    let without_rc = json!({
        "model": model,
        "messages": [user0.clone(), strip_reasoning(asst1.clone()), tool_result.clone()],
        "tools": [date_tool()],
        "max_tokens": 512,
        "stream": false,
    })
    .as_object()
    .cloned()
    .map(|mut m| {
        m.extend(thinking_enabled().as_object().cloned().unwrap());
        Value::Object(m)
    })
    .unwrap();

    let (status_without, resp_without) = chat_completions(&api_key, &base_url, &without_rc)
        .await
        .expect("tool followup without rc");
    println!(
        "tool followup WITHOUT reasoning_content → {status_without} body={}",
        serde_json::to_string(&resp_without).unwrap_or_default()
    );

    // Follow-up WITH reasoning_content (docs: required; must succeed).
    let with_rc = json!({
        "model": model,
        "messages": [user0, asst1, tool_result],
        "tools": [date_tool()],
        "max_tokens": 512,
        "stream": false,
    })
    .as_object()
    .cloned()
    .map(|mut m| {
        m.extend(thinking_enabled().as_object().cloned().unwrap());
        Value::Object(m)
    })
    .unwrap();

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
