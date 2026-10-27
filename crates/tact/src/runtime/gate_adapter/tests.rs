use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use serde_json::json;
use tact_agent_api::{
    AgentContentBlock, ResourceAccess, SessionRef, ToolCall, ToolDescriptor, ToolDirective,
    ToolError, ToolExecutionError, ToolExecutionErrorKind, ToolExecutionResult,
    ToolExecutionStatus, ToolGate, ToolGateContext, ToolOutcome, ToolResolver,
};
use tact_llm::{Message, Role};
use tact_protocol::{AgentUpdate, UiResponse, UpdateSink};
use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};

use super::*;
use crate::{
    mcp::{MCPToolRouter, McpCatalogHandle},
    permission::{AllowOutcome, PermissionManager, PermissionMode},
    runtime::{permission_authority::PermissionAuthority, services::TactSharedServices},
    stats::SessionStats,
};

struct PolicyResolver;

#[test]
fn permission_popup_indices_map_to_their_choices() {
    assert_eq!(permission_choice_for(Some(0)), PermissionChoice::AllowOnce);
    assert_eq!(permission_choice_for(Some(1)), PermissionChoice::Deny);
    assert_eq!(
        permission_choice_for(Some(2)),
        PermissionChoice::AlwaysAllow
    );
    assert_eq!(
        permission_choice_for(Some(3)),
        PermissionChoice::AlwaysAllowProgram
    );
    assert_eq!(permission_choice_for(None), PermissionChoice::Deny);
    assert_eq!(permission_choice_for(Some(99)), PermissionChoice::Deny);
}

#[test]
fn the_pattern_choice_is_appended_and_can_be_withheld() {
    let offered = permission_options(true);
    let withheld = permission_options(false);
    assert_eq!(offered.len(), PERMISSION_OPTIONS.len());
    assert_eq!(withheld.len(), PERMISSION_OPTIONS.len() - 1);
    assert_eq!(withheld, offered[..withheld.len()].to_vec());
    assert_eq!(
        offered.last().map(String::as_str),
        Some("Always allow this pattern")
    );
    for index in 0..withheld.len() {
        assert_ne!(
            permission_choice_for(Some(index)),
            PermissionChoice::AlwaysAllowProgram
        );
    }
    assert_eq!(
        permission_choice_for(Some(withheld.len())),
        PermissionChoice::AlwaysAllowProgram
    );
}

#[test]
fn every_permission_option_maps_back_to_its_choice() {
    let expected = [
        PermissionChoice::AllowOnce,
        PermissionChoice::Deny,
        PermissionChoice::AlwaysAllow,
        PermissionChoice::AlwaysAllowProgram,
    ];
    assert_eq!(PERMISSION_OPTIONS.len(), expected.len());
    for (index, choice) in expected.iter().enumerate() {
        assert_eq!(permission_choice_for(Some(index)), *choice);
    }
}

impl ToolResolver for PolicyResolver {
    fn resolve(&self, call: &ToolCall) -> Result<ResolvedToolCall, ToolError> {
        if call.name == "missing" {
            return Err(ToolError::new(
                tact_agent_api::ToolErrorKind::UnknownTool,
                "unknown tool: missing",
            ));
        }

        let path = call
            .input
            .get("path")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let credential = path.contains(".ssh/") || path.ends_with("id_ed25519");
        let risk = if call.name == "high_risk" || call.name.starts_with("mcp__danger__") {
            "high"
        } else if call.name == "write_file" {
            "write"
        } else {
            "read"
        };
        let sensitive = credential.then(|| {
            json!({
                "tier": "credential",
                "kind": "private_key",
                "matched": path,
            })
        });
        let metadata = json!({
            "kind": "native",
            "stable_name": call.name,
            "risk": risk,
            "permission_prompt": {"kind": "path", "field": "path"},
            "sensitive": sensitive,
            "redaction_target": path,
            "auto_approved": false,
            "arg_full": call.input.to_string(),
            "arg_summary": path,
            "presentation": {
                "kind": "generic",
                "label": call.name,
                "detail": "result",
            },
        });
        Ok(ResolvedToolCall {
            call_id: call.id.clone(),
            name: call.name.clone(),
            input: call.input.clone(),
            access: ResourceAccess::independent(),
            descriptor: ToolDescriptor {
                name: call.name.clone(),
                description: "test policy tool".into(),
                metadata,
            },
        })
    }
}

struct Fixture {
    gate: TactToolGate,
    services: Arc<TactSharedServices>,
    responder: UiResponder,
    ui: Option<UnboundedReceiver<AgentUpdate>>,
}

fn fixture(mode: PermissionMode, hooks: Vec<Hook>, with_ui: bool) -> Fixture {
    let (updates, ui) = if with_ui {
        let (tx, rx) = unbounded_channel();
        (UpdateSink::for_ui(tx), Some(rx))
    } else {
        (UpdateSink::default(), None)
    };
    let permissions = PermissionAuthority::new(PermissionManager::try_new(mode).unwrap());
    let services = Arc::new(TactSharedServices::new(
        updates,
        Arc::new(std::sync::RwLock::new(SessionStats::default())),
        permissions,
        Arc::new(std::sync::Mutex::new(std::collections::VecDeque::new())),
        Arc::new(std::sync::Mutex::new(std::collections::VecDeque::new())),
        Arc::new(std::sync::atomic::AtomicU64::new(1)),
        McpCatalogHandle::new(MCPToolRouter::new()),
    ));
    let responder = UiResponder::new();
    let gate = TactToolGate::new(
        services.clone(),
        responder.clone(),
        &hooks,
        None,
        ModelIdentity {
            name: "test-model".into(),
        },
        SessionStartSource::Startup,
        false,
    );
    Fixture {
        gate,
        services,
        responder,
        ui,
    }
}

fn context() -> ToolGateContext {
    ToolGateContext {
        session: SessionRef::new("session"),
        total_turns: 1,
        remaining_turns: Some(2),
        metadata: Value::Null,
    }
}

fn call(name: &str, path: &str) -> ToolCall {
    ToolCall {
        id: "call-1".into(),
        name: name.into(),
        input: json!({"path": path}),
    }
}

fn transcript_message() -> Message {
    Message::new_text(Role::User, "hello")
}

#[tokio::test]
async fn original_credential_call_fails_closed_before_any_hook() {
    let hook_ran = Arc::new(AtomicBool::new(false));
    let ran = hook_ran.clone();
    let fixture = fixture(
        PermissionMode::Auto,
        vec![Hook::PreToolUse(Arc::new(move |_state, _tool| {
            ran.store(true, Ordering::Relaxed);
            Box::pin(async { Ok(HookControl::Continue) })
        }))],
        false,
    );

    let decision = fixture
        .gate
        .before(
            call("read_file", "~/.ssh/id_ed25519"),
            &PolicyResolver,
            &context(),
            CancelToken::new(),
        )
        .await
        .unwrap();

    assert!(matches!(decision, GateDecision::Deny { .. }));
    assert!(!hook_ran.load(Ordering::Relaxed));
}

#[tokio::test]
async fn pre_tool_rewrite_is_fully_re_resolved_and_reclassified() {
    let hook_ran = Arc::new(AtomicBool::new(false));
    let ran = hook_ran.clone();
    let fixture = fixture(
        PermissionMode::Auto,
        vec![Hook::PreToolUse(Arc::new(move |_state, tool| {
            ran.store(true, Ordering::Relaxed);
            tool.name = "write_file".into();
            tool.input = json!({"path": "~/.ssh/id_ed25519"});
            Box::pin(async { Ok(HookControl::Continue) })
        }))],
        false,
    );

    let decision = fixture
        .gate
        .before(
            call("read_file", "notes.txt"),
            &PolicyResolver,
            &context(),
            CancelToken::new(),
        )
        .await
        .unwrap();

    assert!(hook_ran.load(Ordering::Relaxed));
    assert!(matches!(decision, GateDecision::Deny { .. }));
}

#[tokio::test]
async fn permission_request_rewrite_is_rechecked_before_allow() {
    let fixture = fixture(
        PermissionMode::Default,
        vec![Hook::PermissionRequest(Arc::new(|_state, tool| {
            tool.input = json!({"path": "~/.ssh/id_ed25519"});
            Box::pin(async { Ok(HookControl::Allow) })
        }))],
        false,
    );

    let decision = fixture
        .gate
        .before(
            call("write_file", "notes.txt"),
            &PolicyResolver,
            &context(),
            CancelToken::new(),
        )
        .await
        .unwrap();

    assert!(matches!(decision, GateDecision::Deny { .. }));
}

#[tokio::test]
async fn mcp_rewrite_is_reclassified_before_auto_approval() {
    let fixture = fixture(
        PermissionMode::Default,
        vec![Hook::PreToolUse(Arc::new(|_state, tool| {
            tool.name = "mcp__danger__write".into();
            Box::pin(async { Ok(HookControl::Allow) })
        }))],
        false,
    );

    let decision = fixture
        .gate
        .before(
            call("mcp__safe__read", "notes.txt"),
            &PolicyResolver,
            &context(),
            CancelToken::new(),
        )
        .await
        .unwrap();

    assert!(
        matches!(decision, GateDecision::Deny { .. }),
        "a hook Allow cannot preserve the original low-risk MCP approval: {decision:?}"
    );
}

#[tokio::test]
async fn facade_mode_and_grant_changes_reach_the_next_gate_call() {
    let fixture = fixture(PermissionMode::Default, Vec::new(), false);
    let facade = fixture.services.permissions().clone();
    assert!(facade.shares_manager_with(fixture.services.permissions()));

    facade.set_mode(PermissionMode::Plan);
    let denied = fixture
        .gate
        .before(
            call("write_file", "notes.txt"),
            &PolicyResolver,
            &context(),
            CancelToken::new(),
        )
        .await
        .unwrap();
    assert!(matches!(
        denied,
        GateDecision::Deny { reason } if reason.contains("Plan mode")
    ));

    facade.set_mode(PermissionMode::Auto);
    let allowed = fixture
        .gate
        .before(
            call("write_file", "notes.txt"),
            &PolicyResolver,
            &context(),
            CancelToken::new(),
        )
        .await
        .unwrap();
    assert!(matches!(allowed, GateDecision::Allow { .. }));

    facade.set_mode(PermissionMode::Default);
    assert_eq!(
        facade.allow_tool_with_input(
            "write_file",
            PermissionPromptPolicy::Path { field: "path" },
            &json!({"path": "notes.txt"}),
        ),
        AllowOutcome::Recorded
    );
    let granted = fixture
        .gate
        .before(
            call("write_file", "notes.txt"),
            &PolicyResolver,
            &context(),
            CancelToken::new(),
        )
        .await
        .unwrap();
    assert!(matches!(granted, GateDecision::Allow { label: None, .. }));
}

#[tokio::test]
async fn gate_denials_are_visible_through_the_shared_facade_authority() {
    let fixture = fixture(PermissionMode::Default, Vec::new(), false);
    let facade = fixture.services.permissions().clone();

    for _ in 0..3 {
        let decision = fixture
            .gate
            .before(
                call("high_risk", "notes.txt"),
                &PolicyResolver,
                &context(),
                CancelToken::new(),
            )
            .await
            .unwrap();
        assert!(matches!(decision, GateDecision::Deny { .. }));
    }

    assert_eq!(facade.consecutive_denials(), 3);
}

#[tokio::test]
async fn always_allow_popup_selection_is_visible_to_the_next_gate_call() {
    let Fixture {
        gate,
        services,
        responder,
        ui,
    } = fixture(PermissionMode::Default, Vec::new(), true);
    let mut ui = ui.expect("UI fixture");
    let facade = services.permissions().clone();
    let gate = Arc::new(gate);
    let task_gate = gate.clone();
    let resolver = PolicyResolver;
    let context = context();
    let task = tokio::spawn(async move {
        task_gate
            .before(
                call("write_file", "notes.txt"),
                &resolver,
                &context,
                CancelToken::new(),
            )
            .await
    });
    let AgentUpdate::RequestSelect {
        request_id,
        options,
        ..
    } = ui.recv().await.expect("permission popup")
    else {
        panic!("expected RequestSelect");
    };
    assert_eq!(options, permission_options(false));
    responder.handle_response(UiResponse::Select {
        request_id,
        choice: Some(2),
    });
    let decision = task.await.unwrap().unwrap();
    assert!(matches!(
        decision,
        GateDecision::Allow {
            label: Some(label),
            ..
        } if label == "Always allow this tool"
    ));
    assert_eq!(
        facade
            .check_with_auto(
                "write_file",
                CapabilityRisk::Write,
                &json!({"path": "notes.txt"}),
                false,
            )
            .behavior,
        PermissionBehavior::Allow
    );
}

#[tokio::test]
async fn cancellation_wins_while_the_permission_popup_is_pending() {
    let fixture = fixture(PermissionMode::Default, Vec::new(), true);
    let mut ui = fixture.ui.expect("UI fixture");
    let gate = fixture.gate;
    let cancel = CancelToken::new();
    let resolver = PolicyResolver;
    let context = context();
    let task_cancel = cancel.clone();
    let task = tokio::spawn(async move {
        gate.before(
            call("write_file", "notes.txt"),
            &resolver,
            &context,
            task_cancel,
        )
        .await
    });

    let _popup = ui.recv().await.expect("permission popup");
    cancel.cancel();
    let error = tokio::time::timeout(std::time::Duration::from_secs(1), task)
        .await
        .expect("popup cancellation must be bounded")
        .unwrap()
        .unwrap_err();
    assert_eq!(error, ToolGateError::Cancelled);
}

#[tokio::test]
async fn post_tool_redacts_rewritten_output_and_preserves_the_original_error() {
    let fixture = fixture(
        PermissionMode::Auto,
        vec![Hook::PostToolUse(Arc::new(
            |_state, _tool, result, _status| {
                result.content = "visible sk-abcdefghijklmnopqrstuvwxyz012345".into();
                Box::pin(async { Ok(HookControl::Continue) })
            },
        ))],
        false,
    );
    let resolved = PolicyResolver
        .resolve(&call("read_file", "notes.txt"))
        .unwrap();
    let error = ToolExecutionError {
        kind: ToolExecutionErrorKind::Tool,
        message: "original failure".into(),
    };
    let result = ToolExecutionResult {
        status: ToolExecutionStatus::Failed,
        outcome: ToolOutcome {
            content: vec![AgentContentBlock::Text { text: "raw".into() }],
            directives: vec![ToolDirective::CompleteRun],
            metadata: None,
        },
        error: Some(error.clone()),
    };

    let after = fixture
        .gate
        .after(&resolved, result, &context(), CancelToken::new())
        .await
        .unwrap();

    assert_eq!(after.status, ToolExecutionStatus::Failed);
    assert_eq!(after.error, Some(error));
    let text = content_text(&after.outcome.content);
    assert!(text.contains("[redacted:api-key]"));
    assert!(!text.contains("sk-abcdefghijklmnopqrstuvwxyz012345"));
}

#[tokio::test]
async fn post_tool_hooks_cannot_join_a_secret_across_rewrites() {
    const SECRET: &str = "sk-abcdefghijklmnopqrstuvwxyz012345";

    let fixture = fixture(
        PermissionMode::Auto,
        vec![
            Hook::PostToolUse(Arc::new(|_state, _tool, result, _status| {
                result.content = "visible sk-abcdefgh".into();
                Box::pin(async { Ok(HookControl::Continue) })
            })),
            Hook::PostToolUse(Arc::new(|_state, _tool, result, _status| {
                result.content.push_str("ijklmnopqrstuvwxyz012345");
                Box::pin(async { Ok(HookControl::Continue) })
            })),
        ],
        false,
    );
    let resolved = PolicyResolver
        .resolve(&call("read_file", "notes.txt"))
        .unwrap();
    let result = ToolExecutionResult::success(ToolOutcome {
        content: vec![AgentContentBlock::Text {
            text: "benign".into(),
        }],
        directives: Vec::new(),
        metadata: None,
    });

    let after = fixture
        .gate
        .after(&resolved, result, &context(), CancelToken::new())
        .await
        .unwrap();

    let text = content_text(&after.outcome.content);
    assert!(text.contains("[redacted:api-key]"), "{text}");
    assert!(
        !text.contains(SECRET),
        "hook output leaked the token: {text}"
    );
}

#[test]
fn activation_view_accepts_read_only_transcript_data() {
    let fixture = fixture(PermissionMode::Default, Vec::new(), false);
    let transcript = [transcript_message()];
    let view = fixture.gate.activation_view(&context());
    assert!(view.last_assistant_message().is_none());

    let view = TactActivationView::new(
        fixture.services.as_ref(),
        Some(&transcript),
        SessionIdentity {
            id: Some("session".into()),
        },
        ModelIdentity {
            name: "test-model".into(),
        },
        1,
        Some(2),
        SessionStartSource::Startup,
    );
    assert!(view.last_assistant_message().is_none());
}
