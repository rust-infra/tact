//! Live DeepSeek Responses-protocol smoke tests.
//!
//! Verify that `protocol = "responses"` on the DeepSeek provider routes to
//! the generic Responses adapter and that the configured endpoint actually
//! accepts the full `/responses` surface:
//! 1. ordinary non-streaming requests;
//! 2. streaming (`stream_message`);
//! 3. `context_management` native compaction on ordinary requests;
//! 4. `reasoning.effort` derived from `thinking_budget`;
//! 5. multi-turn continuation with replayed provider state;
//! 6. explicit `/responses/compact` (currently unsupported by the DeepSeek
//!    endpoint; the error surfaces instead of falling back).
//!
//! Each test skips when `DEEPSEEK_API_KEY` is unset or empty.
//! Optional: `DEEPSEEK_BASE_URL` (default `https://api.deepseek.com`),
//! `DEEPSEEK_MODEL` (default `deepseek-v4-flash`).
//!
//!   cargo test -p tact_llm test_deepseek_responses -- --ignored --nocapture

use crate::{
    ContentBlock, CreateMessageParams, LlmClient, LlmResponse, Message, OpenAiProtocol,
    ProviderConversationState, ProviderInfo, ProviderKind, ProviderStateUpdate,
    RequiredMessageParams, Role, Thinking, ThinkingType,
};

/// Reads `DEEPSEEK_API_KEY` / `DEEPSEEK_BASE_URL` / `DEEPSEEK_MODEL` and
/// returns `(api_key, base_url, model)`, or `None` to skip the test.
fn env_config() -> Option<(String, String, String)> {
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

fn deepseek_provider(
    api_key: String,
    base_url: String,
    model: String,
    responses_compact_threshold: Option<u32>,
) -> ProviderInfo {
    ProviderInfo {
        provider: ProviderKind::DeepSeek,
        protocol: OpenAiProtocol::Responses,
        responses_compact_threshold,
        api_key,
        base_url,
        model,
    }
}

fn simple_request(model: String) -> CreateMessageParams {
    CreateMessageParams::new(RequiredMessageParams {
        model,
        max_tokens: 64,
        messages: vec![Message::new_text(Role::User, "Reply with exactly: pong")],
    })
}

fn thinking_request(model: String) -> CreateMessageParams {
    let mut request = simple_request(model);
    request.thinking = Some(Thinking {
        type_: ThinkingType::Enabled,
        budget_tokens: 64_000,
    });
    request
}

fn text_of(response: &LlmResponse) -> String {
    response
        .blocks
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text { text } => Some(text.clone()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn assert_has_text(response: &LlmResponse, label: &str) -> String {
    let text = text_of(response);
    assert!(
        !text.trim().is_empty(),
        "{label}: expected a text block, got: {text:?}"
    );
    text
}

fn request_body(response: &LlmResponse) -> serde_json::Value {
    let bytes = response
        .request_body
        .as_deref()
        .expect("responses request body should be recorded");
    serde_json::from_slice(bytes).expect("request body is valid JSON")
}

#[tokio::test]
#[ignore]
async fn deepseek_responses_smoke() {
    let Some((api_key, base_url, model)) = env_config() else {
        return;
    };
    let client = deepseek_provider(api_key, base_url, model.clone(), None)
        .build_client()
        .expect("build DeepSeek Responses client");

    let response = client
        .create_message(&simple_request(model), None)
        .await
        .expect("DeepSeek /responses request succeeded");

    assert_has_text(&response, "basic non-streaming");
}

#[tokio::test]
#[ignore]
async fn deepseek_responses_stream_smoke() {
    let Some((api_key, base_url, model)) = env_config() else {
        return;
    };
    let client = deepseek_provider(api_key, base_url, model.clone(), None)
        .build_client()
        .expect("build DeepSeek Responses client");

    let response = client
        .stream_message(&simple_request(model), None, None)
        .await
        .expect("DeepSeek /responses streaming request succeeded");

    assert_has_text(&response, "streaming");
}

#[tokio::test]
#[ignore]
async fn deepseek_responses_context_management_smoke() {
    let Some((api_key, base_url, model)) = env_config() else {
        return;
    };
    let client = deepseek_provider(api_key, base_url, model.clone(), Some(64_000))
        .build_client()
        .expect("build DeepSeek Responses client");

    let response = client
        .create_message(&simple_request(model), None)
        .await
        .expect("DeepSeek /responses request with context_management succeeded");

    assert_has_text(&response, "context_management");
    let body = request_body(&response);
    assert_eq!(body["context_management"][0]["type"], "compaction");
    assert_eq!(body["context_management"][0]["compact_threshold"], 64_000);
}

#[tokio::test]
#[ignore]
async fn deepseek_responses_reasoning_effort_smoke() {
    let Some((api_key, base_url, model)) = env_config() else {
        return;
    };
    let client = deepseek_provider(api_key, base_url, model.clone(), None)
        .build_client()
        .expect("build DeepSeek Responses client");

    let response = client
        .create_message(&thinking_request(model), None)
        .await
        .expect("DeepSeek /responses request with reasoning succeeded");

    assert_has_text(&response, "reasoning effort");
    let body = request_body(&response);
    let effort = body["reasoning"]["effort"]
        .as_str()
        .expect("wire request should carry reasoning.effort");
    assert!(!effort.is_empty(), "reasoning.effort must not be empty");
}

#[tokio::test]
#[ignore]
async fn deepseek_responses_state_continuation_smoke() {
    let Some((api_key, base_url, model)) = env_config() else {
        return;
    };
    let client = deepseek_provider(api_key, base_url, model.clone(), None)
        .build_client()
        .expect("build DeepSeek Responses client");

    let request = simple_request(model);
    let first = client
        .create_message(&request, None)
        .await
        .expect("first DeepSeek /responses request succeeded");
    assert_has_text(&first, "first turn");

    let state = match &first.state_update {
        ProviderStateUpdate::Replace(ProviderConversationState::OpenAiResponses(state)) => {
            ProviderConversationState::OpenAiResponses(state.clone())
        }
        _ => panic!("first turn must produce a Replace Responses state update"),
    };

    // The Responses baseline covers the logical history up to and including
    // the first assistant turn, so the replayed request must carry both
    // messages (user + assistant reply).
    assert_has_text(&first, "first turn");
    let second_request = CreateMessageParams::new(RequiredMessageParams {
        model: request.model.clone(),
        max_tokens: 64,
        messages: vec![
            Message::new_text(Role::User, "Reply with exactly: pong"),
            // The state hash covers the assistant turn as blocks, matching
            // how the agent appends response blocks to its logical history.
            Message::new_blocks(Role::Assistant, first.blocks.clone()),
        ],
    });
    let second = client
        .create_message(&second_request, Some(&state))
        .await
        .expect("second DeepSeek /responses request with replayed state succeeded");
    assert_has_text(&second, "second turn");
}

#[tokio::test]
#[ignore]
async fn deepseek_responses_explicit_compact_unsupported() {
    let Some((api_key, base_url, model)) = env_config() else {
        return;
    };
    let client = deepseek_provider(api_key, base_url, model.clone(), None)
        .build_client()
        .expect("build DeepSeek Responses client");

    let request = simple_request(model);
    let first = client
        .create_message(&request, None)
        .await
        .expect("first DeepSeek /responses request succeeded");

    let state = match &first.state_update {
        ProviderStateUpdate::Replace(ProviderConversationState::OpenAiResponses(state)) => {
            ProviderConversationState::OpenAiResponses(state.clone())
        }
        _ => panic!("first turn must produce a Replace Responses state update"),
    };

    assert_has_text(&first, "first turn");
    let compact_request = CreateMessageParams::new(RequiredMessageParams {
        model: request.model.clone(),
        max_tokens: 64,
        messages: vec![
            Message::new_text(Role::User, "Reply with exactly: pong"),
            Message::new_blocks(Role::Assistant, first.blocks.clone()),
        ],
    });
    let compacted = client.compact(&compact_request, Some(&state)).await;

    // Live-verified 2026-08-02: the DeepSeek /responses endpoint accepts
    // ordinary requests (including `context_management`) but its
    // `/responses/compact` does not return a parseable compact resource, so
    // the error must surface clearly instead of hanging or silently falling
    // back to local compaction. Flip this to a success assertion if DeepSeek
    // ships a compatible `/responses/compact`.
    match compacted {
        Ok(_) => {
            panic!("DeepSeek explicit compact unexpectedly succeeded; update this test");
        }
        Err(error) => {
            eprintln!("explicit /responses/compact unsupported as expected: {error}");
        }
    }
}
