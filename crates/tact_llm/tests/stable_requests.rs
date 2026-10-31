#![cfg(any(
    feature = "anthropic",
    feature = "openai-chat",
    feature = "openai-responses"
))]

use tact_agent_api::{
    AgentContentBlock, AgentRole, CancelToken, ModelCallOptions, ModelMessage, ModelPort,
    ModelRequest,
};
use tact_llm::ModelLimits;
#[cfg(feature = "openai-chat")]
use tact_llm::SessionId;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{method, path},
};

fn request(model: &str) -> ModelRequest {
    ModelRequest {
        model: model.into(),
        system: "Be concise.".into(),
        messages: vec![ModelMessage {
            role: AgentRole::User,
            content: vec![AgentContentBlock::Text {
                text: "hello".into(),
            }],
        }],
        tools: Vec::new(),
        max_output_tokens: 100,
        options: ModelCallOptions::default(),
    }
}

fn limits() -> ModelLimits {
    ModelLimits::new(Some(10_000), 1_000).unwrap()
}

fn assert_golden(name: &str, body: &[u8]) {
    let value: serde_json::Value = serde_json::from_slice(body).unwrap();
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/goldens")
        .join(name);
    if std::env::var_os("UPDATE_GOLDENS").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let actual = format!("{}\n", serde_json::to_string_pretty(&value).unwrap());
        std::fs::write(&path, actual).unwrap();
    }
    let expected = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    let expected: serde_json::Value = serde_json::from_str(&expected).unwrap();
    assert_eq!(value, expected, "request golden {}", path.display());
}

fn assert_headers_golden(name: &str, request: &wiremock::Request) {
    let golden: serde_json::Value =
        serde_json::from_str(include_str!("goldens/headers.json")).unwrap();
    let expected = golden[name].as_object().unwrap();
    let actual: serde_json::Map<String, serde_json::Value> = expected
        .keys()
        .map(|header| {
            let value = request
                .headers
                .get(header)
                .unwrap_or_else(|| panic!("missing header {header}"))
                .to_str()
                .unwrap()
                .to_string();
            (header.clone(), serde_json::Value::String(value))
        })
        .collect();
    assert_eq!(
        serde_json::Value::Object(actual),
        serde_json::Value::Object(expected.clone()),
        "header golden {name}"
    );
}

#[cfg(feature = "openai-chat")]
#[tokio::test]
async fn chat_provider_uses_the_fixed_suffix_and_bearer_header() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": "chat_1",
            "object": "chat.completion",
            "created": 1,
            "model": "gpt-test",
            "choices": [{
                "index": 0,
                "message": {"role": "assistant", "content": "ok"},
                "finish_reason": "stop"
            }],
            "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}
        })))
        .expect(1)
        .mount(&server)
        .await;

    let provider = tact_llm::openai_chat("gpt-test")
        .api_key("sk-secret")
        .unwrap()
        .base_url(server.uri())
        .limits(limits())
        .session_id(SessionId::new("session-1").unwrap())
        .build()
        .unwrap();
    let response = provider
        .complete(request("gpt-test"), None, CancelToken::new())
        .await
        .unwrap();
    assert!(response.request_body.is_none());

    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests[0].url.path(), "/chat/completions");
    assert_eq!(
        requests[0].headers.get("authorization").unwrap(),
        "Bearer sk-secret"
    );
    assert_headers_golden("openai-chat", &requests[0]);
    let body: serde_json::Value = serde_json::from_slice(&requests[0].body).unwrap();
    assert_eq!(body["model"], "gpt-test");
    assert_eq!(body["messages"][0]["content"], "Be concise.");
    assert_eq!(body["messages"][1]["content"][0]["text"], "hello");
    assert_golden("openai-chat.json", &requests[0].body);
}

#[cfg(feature = "openai-responses")]
#[tokio::test]
async fn responses_provider_uses_the_fixed_suffix_and_bearer_header() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/responses"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": "resp_1",
            "object": "response",
            "created_at": 1,
            "completed_at": 2,
            "status": "completed",
            "model": "gpt-test",
            "output": [{
                "type": "message",
                "id": "msg_1",
                "role": "assistant",
                "status": "completed",
                "content": [{
                    "type": "output_text",
                    "annotations": [],
                    "logprobs": null,
                    "text": "ok"
                }]
            }],
            "usage": {
                "input_tokens": 1,
                "input_tokens_details": {"cached_tokens": 0},
                "output_tokens": 1,
                "output_tokens_details": {"reasoning_tokens": 0},
                "total_tokens": 2
            }
        })))
        .expect(1)
        .mount(&server)
        .await;

    let provider = tact_llm::openai_responses("gpt-test")
        .api_key("sk-secret")
        .unwrap()
        .base_url(server.uri())
        .limits(limits())
        .build()
        .unwrap();
    let response = provider
        .complete(request("gpt-test"), None, CancelToken::new())
        .await
        .unwrap();
    assert!(response.request_body.is_none());

    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests[0].url.path(), "/responses");
    assert_eq!(
        requests[0].headers.get("authorization").unwrap(),
        "Bearer sk-secret"
    );
    assert_headers_golden("openai-responses", &requests[0]);
    let body: serde_json::Value = serde_json::from_slice(&requests[0].body).unwrap();
    assert_eq!(body["model"], "gpt-test");
    assert_eq!(body["input"][0]["content"][0]["text"], "hello");
    assert_golden("openai-responses.json", &requests[0].body);
}

#[cfg(feature = "anthropic")]
#[tokio::test]
async fn anthropic_provider_uses_messages_and_x_api_key() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": "msg_1",
            "type": "message",
            "role": "assistant",
            "content": [{"type": "text", "text": "ok"}],
            "model": "claude-test",
            "stop_reason": "end_turn",
            "stop_sequence": null,
            "usage": {"input_tokens": 1, "output_tokens": 1}
        })))
        .expect(1)
        .mount(&server)
        .await;

    let provider = tact_llm::anthropic("claude-test")
        .api_key("sk-ant-secret")
        .unwrap()
        .base_url(server.uri())
        .limits(limits())
        .build()
        .unwrap();
    let response = provider
        .complete(request("claude-test"), None, CancelToken::new())
        .await
        .unwrap();
    assert!(response.request_body.is_none());

    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests[0].url.path(), "/messages");
    assert_eq!(
        requests[0].headers.get("x-api-key").unwrap(),
        "sk-ant-secret"
    );
    assert_eq!(
        requests[0].headers.get("anthropic-version").unwrap(),
        "2023-06-01"
    );
    assert_headers_golden("anthropic", &requests[0]);
    let body: serde_json::Value = serde_json::from_slice(&requests[0].body).unwrap();
    assert_eq!(body["model"], "claude-test");
    assert_eq!(body["messages"][0]["content"][0]["text"], "hello");
    assert_golden("anthropic.json", &requests[0].body);
}

#[cfg(feature = "openai-chat")]
#[tokio::test]
async fn anonymous_custom_chat_provider_sends_no_authorization_header() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "choices": [{
                "index": 0,
                "message": {"role": "assistant", "content": "ok"},
                "finish_reason": "stop"
            }]
        })))
        .expect(1)
        .mount(&server)
        .await;

    let provider = tact_llm::custom_chat("local", "local-model", server.uri())
        .anonymous()
        .limits(limits())
        .build()
        .unwrap();
    provider
        .complete(request("local-model"), None, CancelToken::new())
        .await
        .unwrap();

    let requests = server.received_requests().await.unwrap();
    assert!(requests[0].headers.get("authorization").is_none());
    assert_headers_golden("custom-chat", &requests[0]);
    assert_golden("custom-chat.json", &requests[0].body);
}

#[cfg(feature = "openai-responses")]
#[tokio::test]
async fn stable_errors_do_not_expose_response_bodies_or_request_payloads() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/responses"))
        .respond_with(
            ResponseTemplate::new(400).set_body_string("SECRET-RESPONSE-MARKER: invalid request"),
        )
        .expect(1)
        .mount(&server)
        .await;

    let provider = tact_llm::openai_responses("gpt-test")
        .api_key("SECRET-API-KEY")
        .unwrap()
        .base_url(server.uri())
        .limits(limits())
        .build()
        .unwrap();
    let error = provider
        .complete(request("gpt-test"), None, CancelToken::new())
        .await
        .expect_err("the mock endpoint rejects the request");
    let debug = format!("{error:?}");
    let display = error.to_string();
    let source = std::error::Error::source(&error)
        .map(ToString::to_string)
        .unwrap_or_default();
    for text in [&debug, &display, &source] {
        assert!(!text.contains("SECRET-RESPONSE-MARKER"), "{text}");
        assert!(!text.contains("SECRET-API-KEY"), "{text}");
        assert!(!text.contains("hello"), "{text}");
    }
}
