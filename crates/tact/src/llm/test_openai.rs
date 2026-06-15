//! Debug tests for OpenAI-compatible provider connectivity.
//!
//! Run with:
//!   cargo test -p tact test_openai -- --nocapture
//!
//! Required env vars:
//!   OPENAI_API_KEY=<your-key>
//!   OPENAI_BASE_URL=https://api.kimi.com/coding/v1  (optional, defaults to OpenAI official)
//!   OPENAI_MODEL=moonshot-v1-8k

use super::openai::CompatibleConfig;
use async_openai::config::Config;
use async_openai::types::{
    ChatCompletionRequestMessage, ChatCompletionRequestUserMessage,
    ChatCompletionRequestUserMessageContent, CreateChatCompletionRequest, Role,
};

fn build_test_request(model: String) -> CreateChatCompletionRequest {
    CreateChatCompletionRequest {
        model,
        messages: vec![ChatCompletionRequestMessage::User(
            ChatCompletionRequestUserMessage {
                content: ChatCompletionRequestUserMessageContent::Text(
                    "Say hello in one word.".into(),
                ),
                role: Role::User,
                name: None,
            },
        )],
        frequency_penalty: None,
        logit_bias: None,
        logprobs: None,
        top_logprobs: None,
        max_tokens: Some(16),
        n: Some(1),
        presence_penalty: None,
        response_format: None,
        seed: None,
        stop: None,
        stream: Some(false),
        temperature: None,
        top_p: None,
        tools: None,
        tool_choice: None,
        user: None,
        function_call: None,
        functions: None,
    }
}

/// Test non-streaming chat completion with raw reqwest.
#[tokio::test]
async fn test_openai_raw_reqwest() {
    dotenvy::dotenv().ok();

    let api_key = std::env::var("OPENAI_API_KEY").expect("OPENAI_API_KEY not set");
    let base_url = std::env::var("OPENAI_BASE_URL")
        .unwrap_or_else(|_| "https://api.openai.com/v1".to_string());
    let model = std::env::var("OPENAI_MODEL").unwrap_or_else(|_| "gpt-3.5-turbo".to_string());

    let config = CompatibleConfig::new(&api_key, &base_url);
    let request = build_test_request(model);
    let body = serde_json::to_string(&request).unwrap();

    let url = config.url("/chat/completions");
    let headers = config.headers();

    println!("===== RAW REQWEST TEST =====");
    println!("URL: {}", url);
    println!("Headers: {:?}", headers);
    println!("Body:\n{}", serde_json::to_string_pretty(&request).unwrap());

    let client = reqwest::Client::new();
    let resp = client
        .post(&url)
        .headers(headers.clone())
        .body(body)
        .send()
        .await
        .expect("request failed");

    let status = resp.status();
    let body_text = resp.text().await.unwrap_or_default();

    println!("Status: {}", status);
    println!("Response body:\n{}", body_text);
    println!("===== END =====");

    assert!(
        status.is_success(),
        "Expected 2xx, got {}. Body: {}",
        status,
        body_text
    );
}

/// Test non-streaming chat completion through async-openai client.
#[tokio::test]
async fn test_openai_async_openai_non_stream() {
    dotenvy::dotenv().ok();

    let api_key = std::env::var("OPENAI_API_KEY").expect("OPENAI_API_KEY not set");
    let base_url = std::env::var("OPENAI_BASE_URL")
        .unwrap_or_else(|_| "https://api.openai.com/v1".to_string());
    let model = std::env::var("OPENAI_MODEL").unwrap_or_else(|_| "gpt-3.5-turbo".to_string());

    let config = CompatibleConfig::new(&api_key, &base_url);
    let client = async_openai::Client::with_config(config);
    let request = build_test_request(model);

    println!("===== ASYNC-OPENAI NON-STREAM TEST =====");
    println!("URL: {}/chat/completions", base_url);

    match client.chat().create(request).await {
        Ok(resp) => {
            println!("SUCCESS");
            println!("Model: {}", resp.model);
            if let Some(choice) = resp.choices.first() {
                println!("Content: {:?}", choice.message.content);
            }
        }
        Err(e) => {
            println!("ERROR: {:?}", e);
            panic!("async-openai non-stream failed: {}", e);
        }
    }
    println!("===== END =====");
}

/// Test streaming chat completion through async-openai client.
#[tokio::test]
async fn test_openai_async_openai_stream() {
    dotenvy::dotenv().ok();

    let api_key = std::env::var("OPENAI_API_KEY").expect("OPENAI_API_KEY not set");
    let base_url = std::env::var("OPENAI_BASE_URL")
        .unwrap_or_else(|_| "https://api.openai.com/v1".to_string());
    let model = std::env::var("OPENAI_MODEL").unwrap_or_else(|_| "gpt-3.5-turbo".to_string());

    let config = CompatibleConfig::new(&api_key, &base_url);
    let client = async_openai::Client::with_config(config);
    let mut request = build_test_request(model);
    request.stream = Some(true);

    println!("===== ASYNC-OPENAI STREAM TEST =====");
    println!("URL: {}/chat/completions", base_url);

    use futures_util::StreamExt;

    match client.chat().create_stream(request).await {
        Ok(mut stream) => {
            let mut chunks = 0;
            while let Some(result) = stream.next().await {
                match result {
                    Ok(chunk) => {
                        chunks += 1;
                        if let Some(choice) = chunk.choices.first() {
                            if let Some(ref content) = choice.delta.content {
                                print!("{}", content);
                            }
                        }
                    }
                    Err(e) => {
                        println!("\nSTREAM ERROR: {:?}", e);
                        panic!("async-openai stream failed: {}", e);
                    }
                }
            }
            println!("\nReceived {} chunks", chunks);
        }
        Err(e) => {
            println!("ERROR: {:?}", e);
            panic!("async-openai stream init failed: {}", e);
        }
    }
    println!("===== END =====");
}
