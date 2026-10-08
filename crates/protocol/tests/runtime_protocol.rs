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
