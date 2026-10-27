use std::{
    collections::VecDeque,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering as AtomicOrdering},
    },
};

use async_trait::async_trait;
use tact_agent_api::{
    AgentContentBlock, CompactionOutcome, CompactionRequest, ContextError, ContextReplacement,
    ContextView, CursorUpdate, MessageOrigin, ModelCapabilities, ModelPort, RunId, RunInput,
    SessionRef,
};
use tact_llm::{LlmProvider, MockClient};

use super::*;
use crate::{
    compact::CompactState,
    hook::SessionStartSource,
    mcp::{MCPToolRouter, McpCatalogHandle},
    permission::{PermissionManager, PermissionMode},
    runtime::compaction_adapter::TactCompactor,
    runtime::model_adapter::TactModelPort,
    runtime::permission_authority::PermissionAuthority,
    stats::SessionStats,
};

struct ScriptedCompactor {
    micro_enabled: AtomicBool,
    outcomes: Mutex<VecDeque<CompactionOutcome>>,
    session_start_pending: AtomicBool,
    session_start_source: Mutex<SessionStartSource>,
}

impl ScriptedCompactor {
    fn new(micro_enabled: bool, session_start_pending: bool) -> Self {
        Self {
            micro_enabled: AtomicBool::new(micro_enabled),
            outcomes: Mutex::new(VecDeque::new()),
            session_start_pending: AtomicBool::new(session_start_pending),
            session_start_source: Mutex::new(SessionStartSource::Startup),
        }
    }

    fn push_outcome(&self, outcome: CompactionOutcome) {
        self.outcomes.lock().unwrap().push_back(outcome);
    }
}

#[async_trait]
impl Compactor for ScriptedCompactor {
    async fn compact(
        &self,
        _state: &ContextView<'_>,
        _request: CompactionRequest,
        _model: &dyn ModelPort,
        _cancel: CancelToken,
    ) -> Result<CompactionOutcome, ContextError> {
        self.outcomes
            .lock()
            .unwrap()
            .pop_front()
            .ok_or_else(|| ContextError::Compaction("scripted compactor has no outcome".into()))
    }

    async fn commit_pending(
        &self,
        _state: &ContextView<'_>,
        _request: &CompactionRequest,
    ) -> Result<(), ContextError> {
        Ok(())
    }

    fn session_start_pending(&self) -> bool {
        self.session_start_pending.load(AtomicOrdering::Acquire)
    }

    fn take_session_start_pending(&self) -> bool {
        self.session_start_pending
            .swap(false, AtomicOrdering::AcqRel)
    }

    fn session_start_source(&self) -> SessionStartSource {
        *self.session_start_source.lock().unwrap()
    }

    fn micro_compaction_enabled(&self) -> bool {
        self.micro_enabled.load(AtomicOrdering::Acquire)
    }
}

fn services() -> Arc<TactSharedServices> {
    Arc::new(TactSharedServices::new(
        tact_protocol::UpdateSink::default(),
        Arc::new(std::sync::RwLock::new(SessionStats::default())),
        PermissionAuthority::new(PermissionManager::try_new(PermissionMode::Default).unwrap()),
        Arc::new(Mutex::new(VecDeque::new())),
        Arc::new(Mutex::new(VecDeque::new())),
        Arc::new(std::sync::atomic::AtomicU64::new(1)),
        McpCatalogHandle::new(MCPToolRouter::new()),
    ))
}

fn model() -> TactModelPort {
    TactModelPort::from_provider(
        LlmProvider::Mock(MockClient::new(Vec::new())),
        ModelCapabilities {
            model: "mock".into(),
            context_window: 1_000,
            max_output_tokens: 100,
            supports_streaming: true,
            supports_native_compaction: false,
        },
    )
}

fn policy(
    services: Arc<TactSharedServices>,
    hooks: &[Hook],
    session_start_pending: bool,
) -> TactContextPolicy {
    static NEXT_CONTEXT: AtomicUsize = AtomicUsize::new(1);
    let context_name = format!(
        "context-adapter-{}",
        NEXT_CONTEXT.fetch_add(1, AtomicOrdering::Relaxed)
    );
    let context = crate::tool::test_support::test_context(&context_name);
    let prompt = Arc::new(SystemPromptAssembler::static_text("system"));
    let compactor = Arc::new(TactCompactor::new(
        services.clone(),
        prompt.clone(),
        "session",
        ModelIdentity {
            name: "mock".into(),
        },
        context.work_dir.clone(),
        crate::config::test_support::resolved_config().agent,
        Vec::new(),
        tact_llm::ProviderKind::OpenAi,
        false,
        session_start_pending,
        SessionStartSource::Startup,
        CompactState::default(),
        hooks,
    ));
    TactContextPolicy::new(
        services,
        "session",
        prompt,
        compactor,
        ModelIdentity {
            name: "mock".into(),
        },
        Vec::new(),
        hooks,
        ModelCallOptions::default(),
        false,
    )
}

fn policy_with_compactor(
    services: Arc<TactSharedServices>,
    hooks: &[Hook],
    compactor: Arc<ScriptedCompactor>,
) -> TactContextPolicy {
    TactContextPolicy::new(
        services,
        "session",
        Arc::new(SystemPromptAssembler::static_text("system")),
        compactor,
        ModelIdentity {
            name: "mock".into(),
        },
        Vec::new(),
        hooks,
        ModelCallOptions::default(),
        false,
    )
}

#[tokio::test]
async fn successful_session_start_and_user_prompt_context_keep_non_user_origins() {
    let services = services();
    let hooks = vec![
        Hook::SessionStart(Arc::new(
            |view: &crate::LoopState<'_>, context: &mut SessionStartContext| {
                let _ = view;
                Box::pin(async move {
                    context.push_additional_context(Some("test"), "session context");
                    Ok(HookControl::Continue)
                })
            },
        )),
        Hook::UserPromptSubmit(Arc::new(
            |_view: &crate::LoopState<'_>, prompt: &mut String| {
                prompt.push_str("\nuser hook context");
                Box::pin(async move { Ok(HookControl::Continue) })
            },
        )),
    ];
    let policy = policy(services.clone(), &hooks, true);
    let session = SessionRef::new("session");
    let model = model();
    let capabilities = model.capabilities();
    let state = ContextView {
        session: &session,
        transcript: &[],
        cursor: None,
        total_turns: 0,
        max_total_turns: None,
        last_token_total: None,
        pending_inputs: &[],
        capabilities: &capabilities,
        remaining_turns: None,
    };
    let input = RunInput {
        run_id: RunId::new("run-1"),
        session: session.clone(),
        inputs: vec![AgentMessage::text(
            AgentRole::User,
            MessageOrigin::User,
            "question",
        )],
        max_total_turns: None,
    };
    let decision = policy
        .prepare_run(&input, &state, &model, CancelToken::new())
        .await
        .unwrap();
    let PreparationDecision::Continue(preparation) = decision else {
        panic!("expected continue");
    };
    assert_eq!(preparation.inputs.len(), 2);
    assert!(matches!(
        preparation.inputs[0].origin,
        MessageOrigin::InjectedContext { .. }
    ));
    assert_eq!(preparation.inputs[1].origin, MessageOrigin::User);
    assert!(
        preparation.inputs[1]
            .text_content()
            .contains("user hook context")
    );
    assert!(!policy.session_start_pending());
}

#[tokio::test]
async fn queued_context_is_appended_once_before_the_request() {
    let services = services();
    services
        .pending_hook_context
        .lock()
        .unwrap()
        .push_back(HookContextChunk::new(Some("tool"), "queued"));
    services
        .pending_subagent_results
        .lock()
        .unwrap()
        .push_back(SubagentResult {
            child_id: "child".into(),
            success: true,
            summary: "done".into(),
        });
    let policy = policy(services.clone(), &[], false);
    let session = SessionRef::new("session");
    let transcript = vec![AgentMessage::text(
        AgentRole::User,
        MessageOrigin::User,
        "question",
    )];
    let model = model();
    let capabilities = model.capabilities();
    let state = ContextView {
        session: &session,
        transcript: &transcript,
        cursor: None,
        total_turns: 1,
        max_total_turns: None,
        last_token_total: None,
        pending_inputs: &[],
        capabilities: &capabilities,
        remaining_turns: None,
    };
    let decision = policy
        .prepare_request(&state, &model, CancelToken::new())
        .await
        .unwrap();
    let PreparationDecision::Continue(plan) = decision else {
        panic!("expected continue");
    };
    assert_eq!(plan.appended_messages.len(), 2);
    assert!(matches!(
        plan.appended_messages[0].origin,
        MessageOrigin::SubagentResult
    ));
    assert!(matches!(
        plan.appended_messages[1].origin,
        MessageOrigin::InjectedContext { .. }
    ));
    assert_eq!(plan.request.messages.len(), 3);
    assert!(services.pending_hook_context.lock().unwrap().is_empty());
    assert!(services.pending_subagent_results.lock().unwrap().is_empty());
}

#[tokio::test]
async fn skipped_prepare_run_compaction_does_not_replay_stale_inputs() {
    let services = services();
    let hook_runs = Arc::new(AtomicUsize::new(0));
    let hook_runs_for_hook = hook_runs.clone();
    let hooks = vec![Hook::SessionStart(Arc::new(
        move |_view: &crate::LoopState<'_>, _context: &mut SessionStartContext| {
            let hook_runs = hook_runs_for_hook.clone();
            Box::pin(async move {
                hook_runs.fetch_add(1, AtomicOrdering::AcqRel);
                Ok(HookControl::Continue)
            })
        },
    ))];
    let compactor = Arc::new(ScriptedCompactor::new(false, true));
    compactor.push_outcome(CompactionOutcome::Skipped {
        reason: "veto".into(),
    });
    let policy = policy_with_compactor(services, &hooks, compactor);
    let model = model();
    let capabilities = model.capabilities();
    let session = SessionRef::new("session");
    let first_input = RunInput {
        run_id: RunId::new("run-1"),
        session: session.clone(),
        inputs: vec![AgentMessage::text(
            AgentRole::User,
            MessageOrigin::User,
            "first",
        )],
        max_total_turns: None,
    };
    let due_state = ContextView {
        session: &session,
        transcript: &[],
        cursor: None,
        total_turns: 0,
        max_total_turns: None,
        last_token_total: Some(900),
        pending_inputs: &first_input.inputs,
        capabilities: &capabilities,
        remaining_turns: None,
    };
    let decision = policy
        .prepare_run(&first_input, &due_state, &model, CancelToken::new())
        .await
        .unwrap();
    let PreparationDecision::Compact { request, .. } = decision else {
        panic!("expected auto compaction");
    };
    let outcome = policy
        .compact(&due_state, request, &model, CancelToken::new())
        .await
        .unwrap();
    assert!(matches!(outcome, CompactionOutcome::Skipped { .. }));

    let second_input = RunInput {
        run_id: RunId::new("run-2"),
        session: session.clone(),
        inputs: vec![AgentMessage::text(
            AgentRole::User,
            MessageOrigin::User,
            "second",
        )],
        max_total_turns: None,
    };
    let idle_state = ContextView {
        session: &session,
        transcript: &[],
        cursor: None,
        total_turns: 0,
        max_total_turns: None,
        last_token_total: None,
        pending_inputs: &second_input.inputs,
        capabilities: &capabilities,
        remaining_turns: None,
    };
    let decision = policy
        .prepare_run(&second_input, &idle_state, &model, CancelToken::new())
        .await
        .unwrap();
    let PreparationDecision::Continue(preparation) = decision else {
        panic!("expected the fallback prepared without compaction");
    };
    assert_eq!(preparation.inputs.len(), 1);
    assert_eq!(preparation.inputs[0].text_content(), "second");
    assert_eq!(hook_runs.load(AtomicOrdering::Acquire), 1);
}

#[tokio::test]
async fn replaced_prepare_run_compaction_reuses_hook_prepared_inputs_once() {
    let services = services();
    let hook_runs = Arc::new(AtomicUsize::new(0));
    let hook_runs_for_hook = hook_runs.clone();
    let hooks = vec![Hook::SessionStart(Arc::new(
        move |_view: &crate::LoopState<'_>, context: &mut SessionStartContext| {
            let hook_runs = hook_runs_for_hook.clone();
            Box::pin(async move {
                hook_runs.fetch_add(1, AtomicOrdering::AcqRel);
                context.push_additional_context(Some("test"), "brief");
                Ok(HookControl::Continue)
            })
        },
    ))];
    let compactor = Arc::new(ScriptedCompactor::new(false, true));
    compactor.push_outcome(CompactionOutcome::Replaced(ContextReplacement {
        transcript: Some(Vec::new()),
        cursor: CursorUpdate::Unchanged,
    }));
    let policy = policy_with_compactor(services, &hooks, compactor);
    let model = model();
    let capabilities = model.capabilities();
    let session = SessionRef::new("session");
    let input = RunInput {
        run_id: RunId::new("run-1"),
        session: session.clone(),
        inputs: vec![AgentMessage::text(
            AgentRole::User,
            MessageOrigin::User,
            "question",
        )],
        max_total_turns: None,
    };
    let state = ContextView {
        session: &session,
        transcript: &[],
        cursor: None,
        total_turns: 0,
        max_total_turns: None,
        last_token_total: Some(900),
        pending_inputs: &input.inputs,
        capabilities: &capabilities,
        remaining_turns: None,
    };
    let decision = policy
        .prepare_run(&input, &state, &model, CancelToken::new())
        .await
        .unwrap();
    let PreparationDecision::Compact { request, .. } = decision else {
        panic!("expected auto compaction");
    };
    let outcome = policy
        .compact(&state, request, &model, CancelToken::new())
        .await
        .unwrap();
    assert!(matches!(outcome, CompactionOutcome::Replaced(_)));

    let decision = policy
        .prepare_run(&input, &state, &model, CancelToken::new())
        .await
        .unwrap();
    let PreparationDecision::Continue(preparation) = decision else {
        panic!("expected cached preparation");
    };
    assert_eq!(preparation.inputs.len(), 2);
    assert_eq!(hook_runs.load(AtomicOrdering::Acquire), 1);
}

#[tokio::test]
async fn micro_replacement_still_allows_auto_before_queued_context() {
    let services = services();
    services
        .pending_hook_context
        .lock()
        .unwrap()
        .push_back(HookContextChunk::new(Some("tool"), "queued once"));
    let compactor = Arc::new(ScriptedCompactor::new(true, false));
    compactor.push_outcome(CompactionOutcome::Replaced(ContextReplacement {
        transcript: Some(Vec::new()),
        cursor: CursorUpdate::Unchanged,
    }));
    compactor.push_outcome(CompactionOutcome::Replaced(ContextReplacement {
        transcript: Some(Vec::new()),
        cursor: CursorUpdate::Unchanged,
    }));
    let policy = policy_with_compactor(services, &[], compactor);
    let model = model();
    let capabilities = model.capabilities();
    let session = SessionRef::new("session");
    let mut transcript = vec![AgentMessage::new(
        AgentRole::User,
        MessageOrigin::ToolResult,
        (0..13)
            .map(|index| tact_agent_api::AgentContentBlock::ToolResult {
                tool_use_id: format!("call-{index}"),
                content: "x".repeat(160),
            })
            .collect(),
    )];
    let state = ContextView {
        session: &session,
        transcript: &transcript,
        cursor: None,
        total_turns: 1,
        max_total_turns: None,
        last_token_total: Some(900),
        pending_inputs: &[],
        capabilities: &capabilities,
        remaining_turns: None,
    };
    let decision = policy
        .prepare_request(&state, &model, CancelToken::new())
        .await
        .unwrap();
    let PreparationDecision::Compact { request, .. } = decision else {
        panic!("expected micro compaction");
    };
    assert_eq!(request, CompactionRequest::Micro);
    let outcome = policy
        .compact(&state, request, &model, CancelToken::new())
        .await
        .unwrap();
    assert!(matches!(outcome, CompactionOutcome::Replaced(_)));

    transcript.clear();
    let compacted_state = ContextView {
        session: &session,
        transcript: &transcript,
        cursor: None,
        total_turns: 1,
        max_total_turns: None,
        last_token_total: Some(900),
        pending_inputs: &[],
        capabilities: &capabilities,
        remaining_turns: None,
    };
    let idle_state = ContextView {
        session: &session,
        transcript: &transcript,
        cursor: None,
        total_turns: 1,
        max_total_turns: None,
        last_token_total: None,
        pending_inputs: &[],
        capabilities: &capabilities,
        remaining_turns: None,
    };
    let decision = policy
        .prepare_request(&compacted_state, &model, CancelToken::new())
        .await
        .unwrap();
    let PreparationDecision::Compact {
        request,
        if_skipped,
    } = decision
    else {
        panic!("micro replacement must not suppress the following auto check");
    };
    assert!(matches!(request, CompactionRequest::Auto { .. }));
    assert_eq!(if_skipped.appended_messages.len(), 1);
    let outcome = policy
        .compact(&compacted_state, request, &model, CancelToken::new())
        .await
        .unwrap();
    assert!(matches!(outcome, CompactionOutcome::Replaced(_)));

    let decision = policy
        .prepare_request(&compacted_state, &model, CancelToken::new())
        .await
        .unwrap();
    let PreparationDecision::Continue(plan) = decision else {
        panic!("full compaction must suppress an immediate repeat");
    };
    assert_eq!(plan.appended_messages.len(), 1);
    assert!(
        plan.appended_messages[0]
            .text_content()
            .contains("queued once")
    );

    let decision = policy
        .prepare_request(&idle_state, &model, CancelToken::new())
        .await
        .unwrap();
    let PreparationDecision::Continue(plan) = decision else {
        panic!("expected the next request");
    };
    assert!(plan.appended_messages.is_empty());
}

#[tokio::test]
async fn skipped_request_compaction_does_not_replay_queued_context() {
    let services = services();
    services
        .pending_hook_context
        .lock()
        .unwrap()
        .push_back(HookContextChunk::new(Some("tool"), "queued"));
    let compactor = Arc::new(ScriptedCompactor::new(false, false));
    compactor.push_outcome(CompactionOutcome::Skipped {
        reason: "veto".into(),
    });
    let policy = policy_with_compactor(services, &[], compactor);
    let model = model();
    let capabilities = model.capabilities();
    let session = SessionRef::new("session");
    let transcript = Vec::new();
    let due_state = ContextView {
        session: &session,
        transcript: &transcript,
        cursor: None,
        total_turns: 1,
        max_total_turns: None,
        last_token_total: Some(900),
        pending_inputs: &[],
        capabilities: &capabilities,
        remaining_turns: None,
    };
    let decision = policy
        .prepare_request(&due_state, &model, CancelToken::new())
        .await
        .unwrap();
    let PreparationDecision::Compact { request, .. } = decision else {
        panic!("expected auto compaction");
    };
    let outcome = policy
        .compact(&due_state, request, &model, CancelToken::new())
        .await
        .unwrap();
    assert!(matches!(outcome, CompactionOutcome::Skipped { .. }));

    let idle_state = ContextView {
        session: &session,
        transcript: &transcript,
        cursor: None,
        total_turns: 1,
        max_total_turns: None,
        last_token_total: None,
        pending_inputs: &[],
        capabilities: &capabilities,
        remaining_turns: None,
    };
    let decision = policy
        .prepare_request(&idle_state, &model, CancelToken::new())
        .await
        .unwrap();
    let PreparationDecision::Continue(plan) = decision else {
        panic!("expected continue");
    };
    assert!(plan.appended_messages.is_empty());
}

#[test]
fn oversized_hook_context_is_spilled_and_previewed() {
    const LIMIT: usize = 2_500;
    let session = "spill-test";
    assert_eq!(spill_hook_context("brief", session), "brief");

    let full = "graph node ".repeat(4 * LIMIT);
    let text = spill_hook_context(&full, session);
    assert!(text.starts_with("graph node "));
    assert!(text.contains("Full hook output saved to: "));
    assert!(
        crate::compact::approx_text_tokens(&text) <= LIMIT + 32,
        "preview must stay inside the hook-context budget"
    );

    let path = text
        .split("Full hook output saved to: ")
        .nth(1)
        .expect("path footer")
        .trim();
    assert_eq!(std::fs::read_to_string(path).unwrap(), full);
    std::fs::remove_file(path).ok();
}

#[test]
fn first_text_target_only_mutates_the_first_text_block() {
    let mut message = AgentMessage::new(
        AgentRole::User,
        MessageOrigin::User,
        vec![
            AgentContentBlock::Text {
                text: "first".into(),
            },
            AgentContentBlock::ToolUse {
                id: "call-1".into(),
                name: "read_file".into(),
                input: serde_json::json!({"path": "a.txt"}),
            },
            AgentContentBlock::Text {
                text: "second".into(),
            },
        ],
    );
    first_text_target(&mut message)
        .expect("first text block")
        .push_str("-edited");
    assert_eq!(
        message
            .content
            .iter()
            .filter_map(|block| match block {
                AgentContentBlock::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>(),
        ["first-edited", "second"]
    );
}
