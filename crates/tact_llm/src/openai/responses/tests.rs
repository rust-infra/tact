//! Adapter-level Responses tests. The fixtures and the response-envelope
//! helpers are shared by every test in this module; keeping them together
//! avoids duplicating the protocol setup across files.

use super::stream::ResponsesStreamState;
use super::{normalize_stream_event_json, parse_stream_event, parse_stream_event_with_raw};
use crate::LlmError;
use crate::NoopProgressSink;
use crate::{
    ContentBlock, CreateMessageParams, LlmClient, Message, RequiredMessageParams, Role, StopReason,
    Thinking, ThinkingType, Tool,
};
use async_openai_responses::types::responses::{OutputItem, ResponseStreamEvent};

#[test]
fn opencode_config_headers_carry_the_session_id() {
    use async_openai_responses::config::Config;

    let mut config =
        super::ResponsesCompatConfig::new("https://opencode.ai/zen/go/v1".into(), None);
    // Simulate Agent::with_session → LlmProvider::set_user_id wiring.
    config.opencode_session = Some("tact-session-1".into());
    let headers = config.headers();
    assert_eq!(
        headers
            .get(crate::opencode::X_OPENCODE_SESSION)
            .and_then(|v| v.to_str().ok()),
        Some("tact-session-1"),
        "x-opencode-session must equal the session id (cache-distinguishing key)"
    );
    let ua = headers
        .get(reqwest13::header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    assert!(ua.starts_with("tact/"), "user agent identifies tact: {ua}");
}

#[test]
fn opencode_config_without_session_omits_session_header() {
    use async_openai_responses::config::Config;

    // No session id → no synthetic fallback token: the config only sends
    // the identifying User-Agent on OpenCode endpoints.
    let config = super::ResponsesCompatConfig::new("https://opencode.ai/zen/go/v1".into(), None);
    let headers = config.headers();
    assert!(
        headers.get(crate::opencode::X_OPENCODE_SESSION).is_none(),
        "no session header without a session id"
    );
    let ua = headers
        .get(reqwest13::header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    assert!(ua.starts_with("tact/"), "user agent identifies tact: {ua}");
}

#[test]
fn set_session_id_is_used_by_compact_and_sdk_headers() {
    // The adapter stores the session id and hands it to both the SDK
    // config (ordinary /responses) and the direct compact POST.
    let mut adapter =
        super::OpenAiResponsesAdapter::new("test-key", "https://opencode.ai/zen/go/v1", None);
    assert!(adapter.opencode_session().is_none());
    adapter.set_session_id("tact-session-2".into());
    assert_eq!(adapter.opencode_session(), Some("tact-session-2"));
}

#[test]
fn fills_missing_output_text_annotations_for_terminal_events() {
    let event = normalize_stream_event_json(serde_json::json!({
        "type": "response.completed",
        "response": {
            "output": [{
                "type": "message",
                "content": [{"type": "output_text", "text": "answer"}]
            }]
        }
    }));
    assert_eq!(
        event["response"]["output"][0]["content"][0]["annotations"],
        serde_json::json!([])
    );
}

#[test]
fn fills_fields_a_compatible_endpoint_omits() {
    // Measured on commandcode.ai (DeepSeek over `/responses`): a
    // `summary_text` part is announced with no `text`, an `output_text`
    // part with no `annotations`, and a `function_call` item with no
    // `arguments`. All three are required by the SDK types, so without
    // the fill the whole stream dies.
    for event_type in [
        "response.reasoning_summary_part.added",
        "response.reasoning_summary_part.done",
    ] {
        let event = parse_stream_event(serde_json::json!({
            "type": event_type,
            "sequence_number": 3,
            "item_id": "rs_1",
            "output_index": 0,
            "summary_index": 0,
            "part": {"type": "summary_text"}
        }))
        .unwrap_or_else(|error| panic!("{event_type} must parse: {error}"));
        assert!(event.is_some(), "{event_type} must not be dropped");
    }

    for event_type in ["response.content_part.added", "response.content_part.done"] {
        let event = parse_stream_event(serde_json::json!({
            "type": event_type,
            "sequence_number": 3,
            "item_id": "msg_1",
            "output_index": 1,
            "content_index": 0,
            "part": {"type": "output_text", "text": "answer"}
        }))
        .unwrap_or_else(|error| panic!("{event_type} must parse: {error}"));
        assert!(event.is_some(), "{event_type} must not be dropped");
    }

    // The announced call carries `call_id` and `name`; its arguments only
    // stream in afterwards.
    let event = parse_stream_event(serde_json::json!({
        "type": "response.output_item.added",
        "sequence_number": 10,
        "output_index": 1,
        "item": {
            "type": "function_call",
            "call_id": "call_1",
            "name": "ping",
            "id": "fc_1",
            "status": "in_progress"
        }
    }))
    .unwrap_or_else(|error| panic!("an announced function_call must parse: {error}"))
    .expect("an announced function_call must not be dropped");
    let ResponseStreamEvent::ResponseOutputItemAdded(event) = event else {
        panic!("expected an output_item.added event");
    };
    let OutputItem::FunctionCall(call) = &event.item else {
        panic!("expected a function_call item");
    };
    assert_eq!(call.name, "ping");
    assert_eq!(call.arguments, "", "the fill is the type's empty value");
}

#[test]
fn describes_the_request_shape_for_an_opaque_rejection() {
    let body = serde_json::to_vec(&serde_json::json!({
        "model": "deepseek/deepseek-v4.1-flash",
        "input": [
            {"type": "message", "role": "user", "content": "hi"},
            {"type": "function_call", "call_id": "c1", "name": "bash", "arguments": "{}"},
            {"type": "function_call_output", "call_id": "c1", "output": "ok"},
            {"type": "function_call", "call_id": "c2", "name": "bash", "arguments": "{}"}
        ],
        "tools": [{"type": "function"}, {"type": "web_search"}],
        "instructions": "abc",
        "max_output_tokens": 256000
    }))
    .unwrap();
    let shape = super::describe_request_shape(&body);
    for expected in [
        "model=deepseek/deepseek-v4.1-flash",
        "input=4 items (function_call 2, function_call_output 1, message 1)",
        "tools=2",
        "instructions=3 chars",
        "max_output_tokens=256000",
    ] {
        assert!(
            shape.contains(expected),
            "missing {expected:?} in {shape:?}"
        );
    }
    assert!(
        shape.starts_with(&format!("{} bytes, ", body.len())),
        "{shape:?} must lead with the body size"
    );
}

#[test]
fn an_opaque_api_error_keeps_its_text_and_gains_the_shape() {
    // What commandcode.ai answers when it dislikes a request: no field
    // names, no parameter, just a trace id. The failure must stay the
    // SDK's API error (so callers still see "openai responses error") and
    // gain the request shape.
    let response = async_openai_responses::error::ApiErrorResponse {
        status_code: reqwest13::StatusCode::BAD_REQUEST,
        api_error: async_openai_responses::error::ApiError {
            message: "invalid request error trace_id: 70a5949dc0447f578953eb0bf49804f3".to_string(),
            r#type: Some("invalid_request_error".to_string()),
            param: None,
            code: Some(String::new()),
            misalignment: None,
        },
    };
    let error = super::with_request_shape(
        LlmError::from(async_openai_responses::error::OpenAIError::ApiError(
            response,
        )),
        br#"{"model":"m","input":[{"type":"message"}]}"#,
    );
    let message = error.to_string();
    assert!(
        message.starts_with("openai responses error: 400"),
        "{message}"
    );
    assert!(
        message.contains("trace_id: 70a5949dc0447f578953eb0bf49804f3"),
        "{message}"
    );
    assert!(
        message.contains("[request: ") && message.contains("input=1 items (message 1)"),
        "{message}"
    );
}

#[test]
fn parses_content_part_events_without_deserializing_provider_specific_items() {
    let event = parse_stream_event(serde_json::json!({
        "type": "response.content_part.added",
        "sequence_number": 1,
        "item_id": "msg_1",
        "output_index": 0,
        "content_index": 0,
        "part": {"type": "output_text", "text": "answer", "annotations": []}
    }))
    .unwrap();

    assert!(event.is_some());
}

#[test]
fn parses_terminal_unknown_item_with_raw_output_metadata() {
    let mut response = super::normalize::tests::completed_response_json();
    response["output"].as_array_mut().unwrap().insert(
        0,
        serde_json::json!({
            "type": "future_item",
            "id": "future-1",
            "payload": {"x": 1}
        }),
    );
    let parsed = parse_stream_event_with_raw(serde_json::json!({
        "type": "response.completed",
        "sequence_number": 1,
        "response": response
    }))
    .unwrap()
    .unwrap();
    assert!(parsed.raw_output_items.is_some());
    assert_eq!(parsed.raw_output_items.unwrap()[0]["type"], "future_item");
}

#[test]
fn stream_finish_retains_unknown_terminal_items_for_state() {
    let mut response = super::normalize::tests::completed_response_json();
    response["output"].as_array_mut().unwrap().insert(
        0,
        serde_json::json!({
            "type": "future_item",
            "id": "future-1",
            "payload": {"x": 1}
        }),
    );
    let parsed = parse_stream_event_with_raw(serde_json::json!({
        "type": "response.completed",
        "sequence_number": 1,
        "response": response
    }))
    .unwrap()
    .unwrap();
    let mut state = ResponsesStreamState::default();
    state
        .apply_with_raw(parsed.event, parsed.raw_output_items)
        .unwrap();
    let normalized = state.finish().unwrap();
    assert_eq!(normalized.output_items[0]["type"], "future_item");
}

#[test]
fn parses_output_item_added_and_done_events() {
    for event_type in ["response.output_item.added", "response.output_item.done"] {
        let event = parse_stream_event(serde_json::json!({
            "type": event_type,
            "sequence_number": 1,
            "output_index": 0,
            "item": {
                "type": "message",
                "id": "msg_1",
                "role": "assistant",
                "status": "completed",
                "content": [{
                    "type": "output_text",
                    "text": "answer",
                    "annotations": []
                }]
            }
        }))
        .unwrap();

        assert!(event.is_some());
    }
}

#[test]
#[allow(deprecated)]
fn parses_web_search_call_item_with_queries_array() {
    // Compatible endpoints emit the search action with a `queries` array
    // instead of the singular `query`; the stream parser must normalize
    // the item before typed deserialization.
    for event_type in ["response.output_item.added", "response.output_item.done"] {
        let event = parse_stream_event(serde_json::json!({
            "type": event_type,
            "sequence_number": 1,
            "output_index": 0,
            "item": {
                "type": "web_search_call",
                "id": "ws-1",
                "status": "completed",
                "action": {"type": "search", "queries": ["Tokyo weather", "ws_call_id=ws-1"]}
            }
        }))
        .unwrap()
        .expect("event must parse");

        match event {
                async_openai_responses::types::responses::ResponseStreamEvent::ResponseOutputItemAdded(
                    ev,
                ) => {
                    let OutputItem::WebSearchCall(call) = &ev.item else {
                        panic!("expected WebSearchCall item");
                    };
                    assert_eq!(
                        call.action
                            .as_ref()
                            .and_then(|action| match action {
                                async_openai_responses::types::responses::WebSearchToolCallAction::Search(
                                    search,
                                ) => search.query.as_deref(),
                                _ => None,
                            }),
                        Some("Tokyo weather")
                    );
                }
                async_openai_responses::types::responses::ResponseStreamEvent::ResponseOutputItemDone(
                    ev,
                ) => {
                    let OutputItem::WebSearchCall(call) = &ev.item else {
                        panic!("expected WebSearchCall item");
                    };
                    assert_eq!(
                        call.action
                            .as_ref()
                            .and_then(|action| match action {
                                async_openai_responses::types::responses::WebSearchToolCallAction::Search(
                                    search,
                                ) => search.query.as_deref(),
                                _ => None,
                            }),
                        Some("Tokyo weather")
                    );
                }
                other => panic!("expected output item event, got {other:?}"),
            }
    }
}

#[test]
fn parses_output_item_done_with_a_compaction_item() {
    let event = parse_stream_event(serde_json::json!({
        "type": "response.output_item.done",
        "sequence_number": 1,
        "output_index": 2,
        "item": {
            "type": "compaction",
            "id": "cmp_1",
            "encrypted_content": "encrypted-compaction"
        }
    }))
    .unwrap();

    assert!(event.is_some());
}

#[test]
fn fills_missing_terminal_response_ids_before_deserializing() {
    let mut response = super::normalize::tests::completed_response_json();
    response.as_object_mut().unwrap().remove("id");
    response["output"][1].as_object_mut().unwrap().remove("id");

    let event = parse_stream_event(serde_json::json!({
        "type": "response.completed",
        "sequence_number": 1,
        "response": response
    }))
    .unwrap();

    assert!(event.is_some());
}

#[test]
fn infers_terminal_response_status_from_the_event_type() {
    let mut response = super::normalize::tests::completed_response_json();
    response.as_object_mut().unwrap().remove("status");
    response["output"][1]
        .as_object_mut()
        .unwrap()
        .remove("status");

    let event = parse_stream_event(serde_json::json!({
        "type": "response.completed",
        "sequence_number": 1,
        "response": response
    }))
    .unwrap();

    assert!(event.is_some());
}

/// The known-type set is parsed out of serde's unknown-variant error, so a
/// serde wording change would silently empty it — and an empty set means
/// every event is dropped, i.e. a silently dead stream. Pin the derivation
/// itself.
#[test]
fn sdk_event_types_are_derived_from_the_enum() {
    let types = super::sdk_event_types();
    assert!(
        types.len() > 20,
        "deriving the SDK event types from serde produced {} entries; \
             the unknown-variant message format likely changed",
        types.len()
    );
    // One member from each family the stream state machine depends on.
    for expected in [
        "response.created",
        "response.output_item.added",
        "response.output_text.delta",
        "response.completed",
        "response.failed",
        "error",
    ] {
        assert!(
            types.contains(expected),
            "{expected} missing from the derived SDK event types"
        );
    }
}

/// The behaviour that replaced the old 23-literal allowlist.
#[test]
fn unknown_event_types_are_dropped_and_known_ones_parsed() {
    // Not modelled by the SDK at all — a newer server, or a proxy.
    let name = "response.future.event";
    assert!(!super::sdk_knows_event(name));
    assert!(
        parse_stream_event(serde_json::json!({
            "type": name,
            "sequence_number": 1,
            "payload": {"ignored": true},
        }))
        .unwrap()
        .is_none(),
        "an event the SDK does not model must be dropped, not error"
    );

    // Modelled by the SDK: consumed even though Tact's state machine does
    // not act on this particular one. Under the old allowlist it was
    // dropped before deserialization.
    let name = "response.code_interpreter_call.in_progress";
    assert!(super::sdk_knows_event(name));
    assert!(
        parse_stream_event(serde_json::json!({
            "type": name,
            "sequence_number": 2,
            "item_id": "ci-1",
            "output_index": 0,
        }))
        .unwrap()
        .is_some(),
        "an event the SDK models must reach the state machine"
    );
}

/// A malformed payload for a type the SDK *does* model is a real error, and
/// must not be silently swallowed by the unknown-type tolerance.
#[test]
fn malformed_known_event_is_still_an_error() {
    let error = parse_stream_event(serde_json::json!({
        "type": "response.output_text.delta",
        "delta": 42,
    }))
    .expect_err("a malformed known event must error");
    assert!(
        matches!(error, LlmError::StreamParse(_)),
        "expected a StreamParse error, got {error:?}"
    );
}

#[test]
fn infers_completed_status_for_terminal_function_calls() {
    let mut response = super::normalize::tests::completed_response_json();
    response["output"][2]
        .as_object_mut()
        .unwrap()
        .remove("status");

    let event = normalize_stream_event_json(serde_json::json!({
        "type": "response.completed",
        "sequence_number": 1,
        "response": response
    }));

    assert_eq!(
        event["response"]["output"][2]["status"],
        serde_json::json!("completed")
    );
}

#[test]
fn parse_stream_event_accepts_terminal_compaction_item() {
    let mut response = super::normalize::tests::completed_response_json();
    response["output"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({
            "type": "compaction",
            "id": "cmp_1",
            "encrypted_content": "encrypted-compaction"
        }));

    let event = parse_stream_event(serde_json::json!({
        "type": "response.completed",
        "sequence_number": 1,
        "response": response
    }))
    .unwrap();

    assert!(event.is_some());
}

#[test]
fn accepts_supported_lifecycle_and_completion_events() {
    let response = super::normalize::tests::completed_response_json();
    let events = [
        serde_json::json!({
            "type": "response.created",
            "sequence_number": 1,
            "response": response
        }),
        serde_json::json!({
            "type": "response.queued",
            "sequence_number": 2,
            "response": super::normalize::tests::completed_response_json()
        }),
        serde_json::json!({
            "type": "response.in_progress",
            "sequence_number": 3,
            "response": super::normalize::tests::completed_response_json()
        }),
        serde_json::json!({
            "type": "response.content_part.added",
            "sequence_number": 4,
            "item_id": "msg",
            "output_index": 0,
            "content_index": 0,
            "part": {"type": "output_text", "text": "", "annotations": [], "logprobs": null}
        }),
        serde_json::json!({
            "type": "response.content_part.done",
            "sequence_number": 5,
            "item_id": "msg",
            "output_index": 0,
            "content_index": 0,
            "part": {"type": "output_text", "text": "done", "annotations": [], "logprobs": null}
        }),
        serde_json::json!({
            "type": "response.output_text.done",
            "sequence_number": 6,
            "item_id": "msg",
            "output_index": 0,
            "content_index": 0,
            "text": "done",
            "logprobs": []
        }),
        serde_json::json!({
            "type": "response.refusal.done",
            "sequence_number": 7,
            "item_id": "msg",
            "output_index": 0,
            "content_index": 0,
            "refusal": "no"
        }),
        serde_json::json!({
            "type": "response.reasoning_summary_part.added",
            "sequence_number": 8,
            "item_id": "rs",
            "output_index": 0,
            "summary_index": 0,
            "part": {"type": "summary_text", "text": ""}
        }),
        serde_json::json!({
            "type": "response.reasoning_summary_part.done",
            "sequence_number": 9,
            "item_id": "rs",
            "output_index": 0,
            "summary_index": 0,
            "part": {"type": "summary_text", "text": "done"}
        }),
        serde_json::json!({
            "type": "response.reasoning_summary_text.done",
            "sequence_number": 10,
            "item_id": "rs",
            "output_index": 0,
            "summary_index": 0,
            "text": "done"
        }),
        serde_json::json!({
            "type": "response.reasoning_text.done",
            "sequence_number": 11,
            "item_id": "rs",
            "output_index": 0,
            "content_index": 0,
            "text": "done"
        }),
        serde_json::json!({
            "type": "response.function_call_arguments.delta",
            "sequence_number": 12,
            "item_id": "fc",
            "output_index": 1,
            "delta": "{\"x\":"
        }),
        serde_json::json!({
            "type": "response.function_call_arguments.done",
            "sequence_number": 13,
            "item_id": "fc",
            "output_index": 1,
            "name": null,
            "arguments": "{\"x\":1}"
        }),
    ];

    for value in events {
        assert!(parse_stream_event(value).unwrap().is_some());
    }
}

fn adapter_with_state(
    base_url: &str,
    model: &str,
) -> (
    super::OpenAiResponsesAdapter,
    crate::provider_state::ResponsesConversationState,
) {
    let adapter = super::OpenAiResponsesAdapter::new("test-key", base_url, None);
    let state = crate::provider_state::ResponsesConversationState {
        version: 1,
        provider: "openai_responses".to_string(),
        base_url: base_url.to_string(),
        model: model.to_string(),
        input_items: vec![],
        compaction_id: None,
        is_compacted: false,
        logical_message_count: 0,
        logical_context_hash: String::new(),
    };
    (adapter, state)
}

#[test]
fn state_binding_accepts_matching_provider_and_base_url() {
    let (adapter, state) = adapter_with_state("https://api.openai.com/v1", "gpt-5");
    adapter
        .validate_state_binding(&state)
        .expect("matching binding is valid");
}

#[test]
fn state_binding_rejects_another_provider() {
    let (adapter, mut state) = adapter_with_state("https://api.openai.com/v1", "gpt-5");
    state.provider = "anthropic".to_string();
    let error = adapter
        .validate_state_binding(&state)
        .unwrap_err()
        .to_string();
    assert!(error.contains("anthropic"));
    assert!(error.contains("openai_responses"));
}

#[test]
fn state_binding_rejects_another_base_url() {
    let (adapter, mut state) = adapter_with_state("https://api.openai.com/v1", "gpt-5");
    state.base_url = "https://other.example.com/v1".to_string();
    let error = adapter
        .validate_state_binding(&state)
        .unwrap_err()
        .to_string();
    assert!(error.contains("other.example.com"));
}

#[test]
fn state_binding_allows_another_model_for_experiment() {
    let (adapter, mut state) = adapter_with_state("https://api.openai.com/v1", "gpt-5");
    state.model = "gpt-4o".to_string();
    adapter
        .validate_state_binding(&state)
        .expect("model mismatch should not be rejected by local state validation");
}

#[test]
fn replay_defaults_from_base_url_and_can_be_overridden() {
    // Official OpenAI replays historical reasoning by default.
    let official =
        super::OpenAiResponsesAdapter::new("test-key", "https://api.openai.com/v1", None);
    assert!(
        official.replay_prior_reasoning(),
        "official OpenAI must keep replaying prior reasoning items"
    );
    let azure = super::OpenAiResponsesAdapter::new("test-key", "https://my.openai.azure.com", None);
    assert!(azure.replay_prior_reasoning());

    // Compatible endpoints default to dropping the reasoning replay.
    for base_url in [
        "https://opencode.ai/zen/go/v1",
        "https://api.deepseek.com",
        "https://api.moonshot.cn/v1",
        "https://proxy.example.com/v1",
    ] {
        let compatible = super::OpenAiResponsesAdapter::new("test-key", base_url, None);
        assert!(
            !compatible.replay_prior_reasoning(),
            "compatible endpoint {base_url} must drop prior reasoning replay by default"
        );
    }

    // Explicit override wins.
    let compatible =
        super::OpenAiResponsesAdapter::new("test-key", "https://api.deepseek.com", None)
            .with_replay_prior_reasoning(true);
    assert!(compatible.replay_prior_reasoning());
    let official =
        super::OpenAiResponsesAdapter::new("test-key", "https://api.openai.com/v1", None)
            .with_replay_prior_reasoning(false);
    assert!(!official.replay_prior_reasoning());
}

#[test]
fn thinking_on_reasoning_model_forces_replay_on_compatible_base_url() {
    // DeepSeek-ish models in thinking mode require historical
    // `reasoning_text` to be passed back, even on a compatible base URL
    // (OpenCode Go, api.deepseek.com) whose default would drop the
    // replay. Without this the provider returns 400
    // ("reasoning_text in the thinking mode must be passed back to the API").
    for base_url in ["https://opencode.ai/zen/go/v1", "https://api.deepseek.com"] {
        let adapter = super::OpenAiResponsesAdapter::new("test-key", base_url, None);

        // Prior assistant turn carries a persisted reasoning signature.
        let mut request =
                CreateMessageParams::new(RequiredMessageParams {
                    model: "deepseek-v4-flash".to_string(),
                    max_tokens: 128,
                    messages: vec![
                        Message::new_text(Role::User, "inspect this"),
                        Message::new_blocks(
                            Role::Assistant,
                            vec![ContentBlock::Thinking {
                                thinking: "plan".to_string(),
                                signature: "openai-responses-v1:{\"reasoning\":{\"type\":\"reasoning\",\"id\":\"rs_1\",\"summary\":[],\"content\":[{\"type\":\"reasoning_text\",\"text\":\"full chain of thought\"}],\"encrypted_content\":\"opaque\"},\"function_call_item_ids\":{}}"
                                    .to_string(),
                            }],
                        ),
                    ],
                });
        request.thinking = Some(Thinking {
            type_: ThinkingType::Enabled,
            budget_tokens: 64_000,
        });

        let (body, _) = adapter.build_wire_request(&request, None).unwrap();
        let reasoning = body["input"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|item| item["type"] == "reasoning")
            .count();
        assert_eq!(
            reasoning,
            1,
            "thinking-mode {model} on {base_url} must replay the reasoning item: {body:?}",
            model = request.model
        );
    }
}

/// Run with:
/// `cargo test -p tact_llm live_responses_stream_handles_test_endpoint -- --ignored --nocapture`
#[ignore = "hits a real Responses endpoint and requires OPENAI_API_KEY_TEST and OPENAI_BASE_URL_TEST"]
#[tokio::test]
async fn live_responses_stream_handles_test_endpoint() {
    dotenvy::dotenv().ok();

    let api_key = std::env::var("OPENAI_API_KEY_TEST")
        .expect("OPENAI_API_KEY_TEST must be set for the live Responses test");
    let base_url = std::env::var("OPENAI_BASE_URL_TEST")
        .expect("OPENAI_BASE_URL_TEST must be set for the live Responses test");
    let model = std::env::var("OPENAI_MODEL_TEST").unwrap_or_else(|_| "gpt-5.4-mini".to_string());
    let first_user = Message::new_text(Role::User, "Reply with the single word: responses.");
    let request = CreateMessageParams::new(RequiredMessageParams {
        model,
        messages: vec![first_user.clone()],
        max_tokens: 128,
    });
    let adapter = super::OpenAiResponsesAdapter::new(api_key, base_url, None);

    let response = adapter
        .stream_message(&request, None, &NoopProgressSink)
        .await
        .expect("Responses stream request should succeed");
    let blocks = response.blocks;
    let stop_reason = response.stop_reason;
    let request_body = response.request_body;
    let visible_text = blocks
        .iter()
        .filter_map(|block| match block {
            crate::ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<String>();

    assert_eq!(stop_reason, Some(StopReason::EndTurn));
    assert!(
        !visible_text.trim().is_empty(),
        "expected visible response text"
    );
    assert!(
        request_body.is_some(),
        "expected serialized Responses request"
    );

    let follow_up = CreateMessageParams::new(RequiredMessageParams {
        model: request.model.clone(),
        messages: vec![
            first_user,
            Message::new_blocks(Role::Assistant, blocks),
            Message::new_text(Role::User, "Reply with the single word: followup."),
        ],
        max_tokens: 128,
    });
    let response = adapter
        .stream_message(&follow_up, None, &NoopProgressSink)
        .await
        .expect("second Responses stream request should succeed");
    let follow_up_blocks = response.blocks;
    let follow_up_stop_reason = response.stop_reason;

    assert_eq!(follow_up_stop_reason, Some(StopReason::EndTurn));
    assert!(follow_up_blocks.iter().any(|block| {
        matches!(block, crate::ContentBlock::Text { text } if !text.trim().is_empty())
    }));
}

/// Run with:
/// `cargo test -p tact_llm live_responses_stream_calls_tool_on_test_endpoint -- --ignored --nocapture`
#[ignore = "hits a real Responses endpoint and requires OPENAI_API_KEY_TEST and OPENAI_BASE_URL_TEST"]
#[tokio::test]
async fn live_responses_stream_calls_tool_on_test_endpoint() {
    dotenvy::dotenv().ok();

    let api_key = std::env::var("OPENAI_API_KEY_TEST")
        .expect("OPENAI_API_KEY_TEST must be set for the live Responses test");
    let base_url = std::env::var("OPENAI_BASE_URL_TEST")
        .expect("OPENAI_BASE_URL_TEST must be set for the live Responses test");
    let model = std::env::var("OPENAI_MODEL_TEST").unwrap_or_else(|_| "gpt-5.4-mini".to_string());
    let first_user = Message::new_text(Role::User, "commit");
    let system = "You are a coding agent. Complete the user's request instead of stopping after an \
             explanation. Before committing, use the bash tool to inspect repository status. \
             After receiving that result, use bash again to create the commit.";
    let tools = vec![Tool {
        name: "bash".into(),
        description: Some("Run a shell command in the repository".into()),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {"cmd": {"type": "string"}},
            "required": ["cmd"]
        }),
    }];
    let request = CreateMessageParams::new(RequiredMessageParams {
        model,
        messages: vec![first_user.clone()],
        max_tokens: 512,
    })
    .with_system(system)
    .with_tools(tools.clone());
    let adapter = super::OpenAiResponsesAdapter::new(api_key, base_url, None);

    let response = adapter
        .stream_message(&request, None, &NoopProgressSink)
        .await
        .expect("Responses stream request with a tool should succeed");
    let blocks = response.blocks;
    let stop_reason = response.stop_reason;

    assert_eq!(
        stop_reason,
        Some(StopReason::ToolUse),
        "blocks: {blocks:#?}"
    );
    assert!(
        blocks
            .iter()
            .any(|block| matches!(block, ContentBlock::ToolUse { name, .. } if name == "bash")),
        "expected a bash tool call, got: {blocks:#?}"
    );

    let tool_use_id = blocks
        .iter()
        .find_map(|block| match block {
            ContentBlock::ToolUse { id, .. } => Some(id.clone()),
            _ => None,
        })
        .expect("expected tool call id");
    let follow_up = CreateMessageParams::new(RequiredMessageParams {
        model: request.model.clone(),
        messages: vec![
            first_user,
            Message::new_blocks(Role::Assistant, blocks),
            Message::new_blocks(
                Role::User,
                vec![ContentBlock::ToolResult {
                    tool_use_id,
                    content: "## main\n M src/lib.rs".into(),
                }],
            ),
        ],
        max_tokens: 512,
    })
    .with_system(system)
    .with_tools(tools);

    let response = adapter
        .stream_message(&follow_up, None, &NoopProgressSink)
        .await
        .expect("Responses follow-up after a tool result should succeed");
    let follow_up_blocks = response.blocks;
    let follow_up_stop_reason = response.stop_reason;

    assert_eq!(
        follow_up_stop_reason,
        Some(StopReason::ToolUse),
        "blocks: {follow_up_blocks:#?}"
    );
    assert!(
        follow_up_blocks
            .iter()
            .any(|block| matches!(block, ContentBlock::ToolUse { name, .. } if name == "bash"))
    );
}

// ── Regression: configured `responses_compact_threshold` must reach
// ── ordinary `/responses` requests as `context_management` ──────────────

use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn simple_request() -> CreateMessageParams {
    CreateMessageParams::new(RequiredMessageParams {
        model: "gpt-5".to_string(),
        messages: vec![Message::new_text(Role::User, "hello")],
        max_tokens: 256,
    })
}

/// Minimal terminal `/responses` body accepted by `normalize_response`.
fn completed_response_value() -> serde_json::Value {
    serde_json::json!({
        "id": "resp_1",
        "object": "response",
        "created_at": 1754000001,
        "completed_at": 1754000002,
        "status": "completed",
        "model": "gpt-5",
        "output": [{
            "type": "message",
            "id": "msg_1",
            "role": "assistant",
            "status": "completed",
            "content": [{
                "type": "output_text",
                "annotations": [],
                "logprobs": null,
                "text": "hello"
            }]
        }],
        "usage": {
            "input_tokens": 10,
            "input_tokens_details": {"cached_tokens": 0},
            "output_tokens": 2,
            "output_tokens_details": {"reasoning_tokens": 0},
            "total_tokens": 12
        }
    })
}

/// Build the adapter through `ProviderInfo::build_client()` so the whole
/// configuration → adapter wiring (including the Responses threshold) is
/// exercised, then return the captured `/responses` request body.
async fn captured_request_body(
    server: &MockServer,
    compact_threshold: Option<u32>,
) -> serde_json::Value {
    let info = crate::ProviderInfo {
        api_key: "test-key".to_string(),
        base_url: server.uri(),
        model: "gpt-5".to_string(),
        provider: crate::ProviderKind::OpenAi,
        protocol: crate::OpenAiProtocol::Responses,
        responses_compact_threshold: compact_threshold,
    };
    let crate::LlmProvider::OpenAiResponses(adapter) = info.build_client().unwrap() else {
        panic!("expected OpenAiResponses adapter");
    };
    adapter
        .create_message(&simple_request(), None)
        .await
        .expect("ordinary Responses request should succeed");
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1, "exactly one ordinary /responses request");
    serde_json::from_slice(&requests[0].body).unwrap()
}

#[test]
fn unknown_output_fixture_is_well_formed() {
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/unknown_output_item.json")).unwrap();
    assert_eq!(fixture["output"][0]["type"], "future_item");
    assert_eq!(fixture["output"][1]["type"], "message");
}

#[tokio::test]
async fn ordinary_response_state_update_retains_unknown_output_item() {
    let server = MockServer::start().await;
    let mut response = completed_response_value();
    response["output"].as_array_mut().unwrap().insert(
        0,
        serde_json::json!({
            "type": "future_item",
            "id": "future-1",
            "payload": {"x": 1}
        }),
    );
    Mock::given(method("POST"))
        .and(path("/responses"))
        .respond_with(ResponseTemplate::new(200).set_body_json(response))
        .expect(1)
        .mount(&server)
        .await;

    let info = crate::ProviderInfo {
        api_key: "test-key".to_string(),
        base_url: server.uri(),
        model: "gpt-5".to_string(),
        provider: crate::ProviderKind::OpenAi,
        protocol: crate::OpenAiProtocol::Responses,
        responses_compact_threshold: None,
    };
    let crate::LlmProvider::OpenAiResponses(adapter) = info.build_client().unwrap() else {
        panic!("expected OpenAiResponses adapter");
    };
    let response = adapter
        .create_message(&simple_request(), None)
        .await
        .expect("unknown output item must not abort ordinary response");
    let crate::ProviderStateUpdate::Replace(crate::ProviderConversationState::OpenAiResponses(
        state,
    )) = response.state_update
    else {
        panic!("expected Responses state replacement");
    };
    assert_eq!(state.input_items[0]["type"], "message");
    assert_eq!(state.input_items[1]["type"], "future_item");
    server.verify().await;
}

#[tokio::test]
async fn ordinary_responses_request_includes_context_management_when_threshold_configured() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/responses"))
        .respond_with(ResponseTemplate::new(200).set_body_json(completed_response_value()))
        .expect(1)
        .mount(&server)
        .await;

    let body = captured_request_body(&server, Some(160_000)).await;

    assert_eq!(
        body["context_management"][0]["type"],
        serde_json::json!("compaction"),
        "ordinary /responses request must declare native compaction, got: {body}"
    );
    assert_eq!(
        body["context_management"][0]["compact_threshold"],
        serde_json::json!(160_000),
        "configured threshold must reach the wire request, got: {body}"
    );
    server.verify().await;
}

#[tokio::test]
async fn ordinary_responses_request_omits_context_management_without_threshold() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/responses"))
        .respond_with(ResponseTemplate::new(200).set_body_json(completed_response_value()))
        .expect(1)
        .mount(&server)
        .await;

    let body = captured_request_body(&server, None).await;

    assert!(
        body.get("context_management").is_none(),
        "no threshold must mean no context_management (no default injection), got: {body}"
    );
    server.verify().await;
}

#[tokio::test]
async fn streamed_responses_request_includes_context_management_when_threshold_configured() {
    let server = MockServer::start().await;
    let sse = format!(
        "data: {}\n\n",
        serde_json::json!({
            "type": "response.completed",
            "sequence_number": 1,
            "response": completed_response_value()
        })
    );
    Mock::given(method("POST"))
        .and(path("/responses"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(sse, "text/event-stream"))
        .expect(1)
        .mount(&server)
        .await;

    let info = crate::ProviderInfo {
        api_key: "test-key".to_string(),
        base_url: server.uri(),
        model: "gpt-5".to_string(),
        provider: crate::ProviderKind::OpenAi,
        protocol: crate::OpenAiProtocol::Responses,
        responses_compact_threshold: Some(160_000),
    };
    let crate::LlmProvider::OpenAiResponses(adapter) = info.build_client().unwrap() else {
        panic!("expected OpenAiResponses adapter");
    };
    adapter
        .stream_message(&simple_request(), None, &NoopProgressSink)
        .await
        .expect("streamed Responses request should succeed");

    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1, "exactly one streamed /responses request");
    let body: serde_json::Value = serde_json::from_slice(&requests[0].body).unwrap();
    assert_eq!(
        body["context_management"][0]["compact_threshold"],
        serde_json::json!(160_000),
        "streamed ordinary request must carry the configured threshold, got: {body}"
    );
    server.verify().await;
}

#[tokio::test]
async fn unknown_stream_fixture_keeps_visible_text_and_state_item() {
    let server = MockServer::start().await;
    let sse = include_str!("fixtures/unknown_event_stream.jsonl");
    Mock::given(method("POST"))
        .and(path("/responses"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(sse, "text/event-stream"))
        .expect(1)
        .mount(&server)
        .await;
    let adapter = super::OpenAiResponsesAdapter::new("test-key", server.uri(), None);
    let response = adapter
        .stream_message(&simple_request(), None, &NoopProgressSink)
        .await
        .expect("unknown stream event must not abort response");
    assert!(
        response.blocks.iter().any(|block| {
            matches!(block, crate::ContentBlock::Text { text } if text == "hello")
        })
    );
    let crate::ProviderStateUpdate::Replace(crate::ProviderConversationState::OpenAiResponses(
        state,
    )) = response.state_update
    else {
        panic!("expected Responses state replacement");
    };
    assert!(
        state
            .input_items
            .iter()
            .any(|item| item["type"] == "future_item")
    );
    server.verify().await;
}

#[tokio::test]
async fn commandcode_stream_fixture_keeps_reasoning_and_answer() {
    // Captured from commandcode.ai (`deepseek/deepseek-v4.1-flash` over
    // `/responses`). The stream announces a reasoning summary part with
    // `{"type": "summary_text"}` and no `text` — the text arrives only at
    // `reasoning_summary_text.done`. An event Tact never reads must not be
    // able to abort the turn with `missing field `text``.
    let server = MockServer::start().await;
    let sse = include_str!("fixtures/commandcode_deepseek_stream.jsonl");
    Mock::given(method("POST"))
        .and(path("/responses"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(sse, "text/event-stream"))
        .expect(1)
        .mount(&server)
        .await;
    let adapter = super::OpenAiResponsesAdapter::new("test-key", server.uri(), None);
    let response = adapter
        .stream_message(&simple_request(), None, &NoopProgressSink)
        .await
        .expect("a text-less reasoning summary part must not abort the stream");
    assert!(
        response.blocks.iter().any(|block| {
            matches!(block, ContentBlock::Thinking { thinking, .. }
                    if thinking.starts_with("We need answer."))
        }),
        "reasoning summary must survive, got: {:?}",
        response.blocks
    );
    assert!(
        response
            .blocks
            .iter()
            .any(|block| { matches!(block, ContentBlock::Text { text } if text == "Hi") })
    );
    server.verify().await;
}

#[tokio::test]
async fn commandcode_tool_call_stream_fixture_keeps_the_call() {
    // Captured from commandcode.ai (`deepseek/deepseek-v4.1-flash` over
    // `/responses`, a tool-calling turn). `output_item.added` announces
    // the `function_call` with no `arguments` — the arguments stream in
    // afterwards and only the `output_item.done` / terminal item carries
    // them. The announced event must not abort the turn, and the call
    // must arrive with the arguments the provider did send.
    let server = MockServer::start().await;
    let sse = include_str!("fixtures/commandcode_deepseek_tool_call_stream.jsonl");
    Mock::given(method("POST"))
        .and(path("/responses"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(sse, "text/event-stream"))
        .expect(1)
        .mount(&server)
        .await;
    let adapter = super::OpenAiResponsesAdapter::new("test-key", server.uri(), None);
    let response = adapter
        .stream_message(&simple_request(), None, &NoopProgressSink)
        .await
        .expect("a function_call announced without arguments must not abort the stream");
    assert_eq!(
        response.stop_reason,
        Some(StopReason::ToolUse),
        "blocks: {:#?}",
        response.blocks
    );
    let call = response
        .blocks
        .iter()
        .find_map(|block| match block {
            ContentBlock::ToolUse { name, input, .. } => Some((name.clone(), input.clone())),
            _ => None,
        })
        .expect("the announced call must survive as a tool use block");
    assert_eq!(call.0, "ping");
    assert_eq!(
        call.1,
        serde_json::json!({}),
        "the arguments must come from the completed item, not the announcement"
    );
    server.verify().await;
}

#[tokio::test]
async fn an_opaque_400_from_the_endpoint_is_typed() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/responses"))
        .respond_with(ResponseTemplate::new(400).set_body_json(serde_json::json!({
            "error": {
                "message": "invalid request error trace_id: 70a5949dc0447f578953eb0bf49804f3",
                "type": "invalid_request_error"
            }
        })))
        .expect(1)
        .mount(&server)
        .await;
    let adapter = super::OpenAiResponsesAdapter::new("test-key", server.uri(), None);
    let error = adapter
        .stream_message(&simple_request(), None, &NoopProgressSink)
        .await
        .expect_err("a 400 must fail the turn");
    let message = error.to_string();
    assert!(message.contains("api error (400)"), "{message}");
    server.verify().await;
}

#[tokio::test]
async fn compact_preserves_the_resource_reported_usage() {
    let server = MockServer::start().await;
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/explicit_compact.json")).unwrap();
    Mock::given(method("POST"))
        .and(path("/responses/compact"))
        .respond_with(ResponseTemplate::new(200).set_body_json(fixture))
        .expect(1)
        .mount(&server)
        .await;

    let crate::LlmProvider::OpenAiResponses(adapter) = crate::ProviderInfo {
        api_key: "test-key".to_string(),
        base_url: server.uri(),
        model: "gpt-5.4-mini".to_string(),
        provider: crate::ProviderKind::OpenAi,
        protocol: crate::OpenAiProtocol::Responses,
        responses_compact_threshold: None,
    }
    .build_client()
    .unwrap() else {
        panic!("expected OpenAiResponses adapter");
    };
    let response = adapter
        .compact(&simple_request(), None)
        .await
        .expect("native compact should succeed");
    let usage = response
        .usage
        .expect("compact resource usage must be preserved");
    assert_eq!(usage.total, 1540);
    assert_eq!(usage.prompt, 1200);
    assert_eq!(usage.completion, 340);
    server.verify().await;
}

#[tokio::test]
async fn compact_reports_missing_endpoint_clearly() {
    let server = MockServer::start().await;
    // A compatible proxy that does not implement /responses/compact
    // returns 404 with an HTML page. The adapter must surface a clear
    // message instead of dumping the HTML body through the SDK's
    // JSON-deserialization error.
    Mock::given(method("POST"))
        .and(path("/responses/compact"))
        .respond_with(
            ResponseTemplate::new(404)
                .set_body_string("<!DOCTYPE html><html><title>Not Found | proxy</title></html>"),
        )
        .expect(1)
        .mount(&server)
        .await;

    let crate::LlmProvider::OpenAiResponses(adapter) = crate::ProviderInfo {
        api_key: "test-key".to_string(),
        base_url: server.uri(),
        model: "gpt-5.4-mini".to_string(),
        provider: crate::ProviderKind::OpenAi,
        protocol: crate::OpenAiProtocol::Responses,
        responses_compact_threshold: None,
    }
    .build_client()
    .unwrap() else {
        panic!("expected OpenAiResponses adapter");
    };
    let error = adapter
        .compact(&simple_request(), None)
        .await
        .expect_err("missing /responses/compact must fail");
    let message = error.to_string();
    assert!(
        message.contains("does not support POST /responses/compact"),
        "expected a clear unsupported-endpoint message, got: {message}"
    );
    assert!(
        !message.contains("<!DOCTYPE html>"),
        "the HTML body must not leak into the error, got: {message}"
    );
    server.verify().await;
}
