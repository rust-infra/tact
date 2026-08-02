//! Live DeepSeek Responses-protocol smoke test.
//!
//! Verifies that `protocol = "responses"` on the DeepSeek provider routes to
//! the generic Responses adapter and that the configured endpoint actually
//! accepts `/responses` requests.
//!
//! Skips when `DEEPSEEK_API_KEY` is unset or empty.
//! Optional: `DEEPSEEK_BASE_URL` (default `https://api.deepseek.com`),
//! `DEEPSEEK_MODEL` (default `deepseek-v4-flash`).
//!
//!   cargo test -p tact_llm deepseek_responses_smoke -- --ignored --nocapture

use crate::{
    ContentBlock, CreateMessageParams, LlmClient, Message, OpenAiProtocol, ProviderInfo,
    ProviderKind, RequiredMessageParams, Role,
};

#[tokio::test]
#[ignore]
async fn deepseek_responses_smoke() {
    dotenvy::dotenv().ok();
    let api_key = match std::env::var("DEEPSEEK_API_KEY") {
        Ok(k) if !k.trim().is_empty() => k,
        _ => {
            eprintln!("skipping: DEEPSEEK_API_KEY not set");
            return;
        }
    };
    let base_url = std::env::var("DEEPSEEK_BASE_URL")
        .unwrap_or_else(|_| "https://api.deepseek.com".to_string());
    let model =
        std::env::var("DEEPSEEK_MODEL").unwrap_or_else(|_| "deepseek-v4-flash".to_string());

    let provider = ProviderInfo {
        provider: ProviderKind::DeepSeek,
        protocol: OpenAiProtocol::Responses,
        reasoning_effort: None,
        responses_compact_threshold: None,
        api_key,
        base_url: base_url.clone(),
        model: model.clone(),
    };
    let client = provider.build_client().expect("build DeepSeek Responses client");

    let request = CreateMessageParams::new(RequiredMessageParams {
        model,
        max_tokens: 64,
        messages: vec![Message::new_text(Role::User, "Reply with exactly: pong")],
    });

    let response = client
        .create_message(&request, None)
        .await
        .expect("DeepSeek /responses request succeeded");

    let text = response
        .blocks
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text { text } => Some(text.clone()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");

    assert!(
        text.to_ascii_lowercase().contains("pong"),
        "unexpected DeepSeek Responses reply: {text:?}"
    );
}
