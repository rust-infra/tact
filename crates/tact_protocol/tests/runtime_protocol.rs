use tact_protocol::*;

#[test]
fn ids_are_string_serializable_and_reject_empty_construction() {
    let id = PluginId::new("demo").unwrap();
    assert_eq!(serde_json::to_string(&id).unwrap(), "\"demo\"");
    assert_eq!(serde_json::from_str::<PluginId>("\"demo\"").unwrap(), id);
    assert!(PluginId::new(" ").is_err());
    assert!(serde_json::from_str::<PluginId>("\" \"").is_err());
    assert!(serde_json::from_str::<PluginId>("\"\\u0000\"").is_err());
}

#[test]
fn request_envelope_round_trips_optional_context() {
    let request = RequestEnvelope {
        protocol_version: ProtocolVersion::CURRENT,
        request_id: RequestId::from("req-1"),
        plugin_id: PluginId::from("plugin-a"),
        session_id: Some(SessionId::from("session-1")),
        run_id: None,
        trajectory_id: None,
        deadline: Some("2030-01-01T00:00:00Z".into()),
        request: PluginRequest::Invoke {
            capability: "demo.echo".into(),
            input: serde_json::json!({"x": 1}),
        },
    };
    let json = serde_json::to_string(&request).unwrap();
    let decoded: RequestEnvelope = serde_json::from_str(&json).unwrap();
    assert_eq!(decoded.request_id, RequestId::from("req-1"));
    assert!(matches!(decoded.request, PluginRequest::Invoke { .. }));
}

#[test]
fn unknown_runtime_variant_is_rejected() {
    let value = serde_json::json!({"type":"future_event","x":1});
    assert!(serde_json::from_value::<RuntimeEvent>(value).is_err());
}

#[test]
fn plugin_events_require_namespaced_non_reserved_types() {
    let good = RuntimeEvent::Plugin {
        plugin_id: PluginId::from("demo"),
        origin: "plugin".into(),
        event_type: "plugin.demo.progress".into(),
        payload: serde_json::json!({}),
    };
    assert!(good.validate_plugin_event("demo").is_ok());
    let dotted_plugin = RuntimeEvent::Plugin {
        plugin_id: PluginId::from("fixture.wasm"),
        origin: "plugin".into(),
        event_type: "plugin.fixture.wasm.progress".into(),
        payload: serde_json::json!({}),
    };
    assert!(dotted_plugin.validate_plugin_event("fixture.wasm").is_ok());
    let wrong_owner = RuntimeEvent::Plugin {
        plugin_id: PluginId::from("demo"),
        origin: "plugin".into(),
        event_type: "plugin.other.progress".into(),
        payload: serde_json::json!({}),
    };
    assert!(wrong_owner.validate_plugin_event("demo").is_err());
    let reserved = RuntimeEvent::Plugin {
        plugin_id: PluginId::from("demo"),
        origin: "plugin".into(),
        event_type: "plugin.demo.permission.granted".into(),
        payload: serde_json::json!({}),
    };
    assert!(reserved.validate_plugin_event("demo").is_err());
    let missing_origin = RuntimeEvent::Plugin {
        plugin_id: PluginId::from("demo"),
        origin: "".into(),
        event_type: "plugin.demo.progress".into(),
        payload: serde_json::json!({}),
    };
    assert!(missing_origin.validate_plugin_event("demo").is_err());
    assert!(
        RuntimeEvent::RunStarted {
            run_id: RunId::from("run")
        }
        .validate_plugin_event("demo")
        .is_err()
    );
}

#[test]
fn capability_validation_and_error_round_trip() {
    let capability = CapabilityDeclaration {
        name: "demo.echo".into(),
        kind: CapabilityKind::Tool,
        version: "1.0".into(),
        description: None,
        input_schema: None,
        output_schema: None,
        risk: CapabilityRisk::ReadOnly,
    };
    capability.validate().unwrap();
    let error = ProtocolError::new(ErrorCategory::Timeout, "deadline exceeded", "host", true);
    let decoded: ProtocolError =
        serde_json::from_str(&serde_json::to_string(&error).unwrap()).unwrap();
    assert_eq!(decoded.category, ErrorCategory::Timeout);
    assert!(decoded.retryable);
    let response = PluginResponse::Error { error };
    let response_json = serde_json::to_string(&response).unwrap();
    assert!(serde_json::from_str::<PluginResponse>(&response_json).is_ok());
}

#[test]
fn nested_events_and_interactions_are_json_serializable() {
    let event = PluginResponse::Event {
        event: RuntimeEvent::InteractionRequested {
            request: InteractionRequest::Confirm {
                request_id: RequestId::from("req"),
                prompt: "Continue?".into(),
            },
        },
    };
    let json = serde_json::to_string(&event).unwrap();
    assert!(json.contains("interaction_requested"));
    let decoded: PluginResponse = serde_json::from_str(&json).unwrap();
    assert!(matches!(decoded, PluginResponse::Event { .. }));
}

#[test]
fn plugin_host_calls_round_trip_with_nested_correlation_ids() {
    let request = PluginRequest::HostCallResult {
        host_request_id: RequestId::from("host-call-1"),
        output: Some(serde_json::json!({ "now": "123" })),
        error: None,
    };
    let encoded = serde_json::to_string(&request).unwrap();
    let decoded: PluginRequest = serde_json::from_str(&encoded).unwrap();
    assert!(matches!(
        decoded,
        PluginRequest::HostCallResult { host_request_id, .. }
            if host_request_id == RequestId::from("host-call-1")
    ));

    let response = PluginResponse::HostCall {
        host_request_id: RequestId::from("host-call-2"),
        capability: "clock.read".into(),
        input: serde_json::Value::Null,
    };
    let encoded = serde_json::to_string(&response).unwrap();
    let decoded: PluginResponse = serde_json::from_str(&encoded).unwrap();
    assert!(matches!(
        decoded,
        PluginResponse::HostCall { capability, .. } if capability == "clock.read"
    ));
}

#[test]
fn agent_streaming_events_round_trip_as_runtime_messages() {
    let events = vec![
        RuntimeEvent::Thinking {
            run_id: Some(RunId::from("run-1")),
            chunk: ThinkingChunk::Delta("reasoning".into()),
        },
        RuntimeEvent::ToolProgress {
            run_id: Some(RunId::from("run-1")),
            tool_id: "tool-1".into(),
            chunks: vec![ToolOutputChunk::stderr("warning")],
        },
        RuntimeEvent::ModelInfo {
            run_id: Some(RunId::from("run-1")),
            params: ModelCallParams {
                model: "test-model".into(),
                max_tokens: 100,
                thinking_budget: Some(20),
                reasoning_effort: Some("low".into()),
                extra_body: None,
            },
        },
        RuntimeEvent::TokenUsage {
            run_id: Some(RunId::from("run-1")),
            usage: TokenUsageInfo {
                prompt: 10,
                completion: 5,
                total: 15,
                prompt_cache_hit_tokens: 2,
                prompt_cache_miss_tokens: 8,
                reasoning_tokens: 1,
            },
        },
        RuntimeEvent::TurnStats {
            run_id: Some(RunId::from("run-1")),
            turns_taken: 2,
            max_turns: Some(10),
        },
        RuntimeEvent::InteractionRequested {
            request: InteractionRequest::MultiSelect {
                request_id: RequestId::from("choose-many"),
                prompt: "Pick items".into(),
                options: vec!["one".into(), "two".into()],
            },
        },
    ];
    for event in events {
        let encoded = serde_json::to_value(&event).unwrap();
        let decoded: RuntimeEvent = serde_json::from_value(encoded.clone()).unwrap();
        assert_eq!(serde_json::to_value(decoded).unwrap(), encoded);
    }
}

#[test]
fn runtime_step_started_round_trips_rich_tool_card_data() {
    let event = tact_protocol::RuntimeEvent::StepStarted {
        run_id: Some(tact_protocol::RunId::from("run-view")),
        idx: 3,
        tool_id: "call-3".into(),
        tool_name: "write_file".into(),
        arg_summary: "src/main.rs".into(),
        arg_full: "src/main.rs: 2 lines".into(),
        presentation: tact_protocol::ToolPresentationInfo {
            visual_kind: tact_protocol::ToolVisualKind::FileWrite,
            display_name: "Write File".into(),
            keep_full_live_output: true,
            detail: tact_protocol::ToolDetailKind::Result,
            popup: tact_protocol::ToolPopupKind::None,
            compact_result_to_meta: false,
            keep_live: false,
        },
    };

    let json = serde_json::to_value(&event).unwrap();
    let decoded: tact_protocol::RuntimeEvent = serde_json::from_value(json).unwrap();
    assert!(matches!(
        decoded,
        tact_protocol::RuntimeEvent::StepStarted { idx: 3, tool_id, presentation, .. }
            if tool_id == "call-3"
                && presentation.visual_kind == tact_protocol::ToolVisualKind::FileWrite
    ));
}
