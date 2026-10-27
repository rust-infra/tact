use std::{
    collections::VecDeque,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

use async_trait::async_trait;
use tact_agent_api::{
    AgentContentBlock, AgentMessage, AgentRole, CompactionKind, ContextView, MessageId,
    MessageOrigin, ModelCapabilities, ModelError, ModelErrorKind, ModelRequest, ModelResponse,
    ModelStream, SessionRef, StopReason,
};
use tact_llm::ProviderKind;
use tact_protocol::TokenUsageInfo;

use super::*;
use crate::{
    mcp::{MCPToolRouter, McpCatalogHandle},
    permission::{PermissionManager, PermissionMode},
    runtime::permission_authority::PermissionAuthority,
    stats::SessionStats,
};

struct ScriptedModel {
    capabilities: ModelCapabilities,
    complete: Mutex<VecDeque<Result<ModelResponse, ModelError>>>,
    compact: Mutex<VecDeque<Result<ModelResponse, ModelError>>>,
}

impl ScriptedModel {
    fn new(native: bool) -> Self {
        Self {
            capabilities: ModelCapabilities {
                model: "mock".into(),
                context_window: 200_000,
                max_output_tokens: 2_000,
                supports_streaming: true,
                supports_native_compaction: native,
            },
            complete: Mutex::new(VecDeque::new()),
            compact: Mutex::new(VecDeque::new()),
        }
    }

    fn push_complete(&self, response: Result<ModelResponse, ModelError>) {
        self.complete.lock().unwrap().push_back(response);
    }

    fn push_native(&self, response: Result<ModelResponse, ModelError>) {
        self.compact.lock().unwrap().push_back(response);
    }
}

#[async_trait]
impl ModelPort for ScriptedModel {
    fn capabilities(&self) -> ModelCapabilities {
        self.capabilities.clone()
    }

    async fn stream(
        &self,
        _request: ModelRequest,
        _cursor: Option<&ModelCursor>,
        _cancel: CancelToken,
    ) -> Result<ModelStream, ModelError> {
        Err(ModelError::unsupported("stream"))
    }

    async fn complete(
        &self,
        _request: ModelRequest,
        _cursor: Option<&ModelCursor>,
        _cancel: CancelToken,
    ) -> Result<ModelResponse, ModelError> {
        self.complete
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| {
                Err(ModelError::new(
                    ModelErrorKind::Permanent,
                    "no scripted complete response",
                ))
            })
    }

    async fn compact(
        &self,
        _request: ModelRequest,
        _cursor: Option<&ModelCursor>,
        _cancel: CancelToken,
    ) -> Result<ModelResponse, ModelError> {
        self.compact.lock().unwrap().pop_front().unwrap_or_else(|| {
            Err(ModelError::new(
                ModelErrorKind::Permanent,
                "no scripted native response",
            ))
        })
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

fn settings() -> AgentSettings {
    let mut settings = crate::config::test_support::resolved_config().agent;
    settings.model_context_window = 200_000;
    settings.max_tokens = 2_000;
    settings.micro_compact_enabled = true;
    settings
}

fn transcript() -> Vec<AgentMessage> {
    vec![
        AgentMessage::text(AgentRole::User, MessageOrigin::User, "task"),
        AgentMessage::text(AgentRole::Assistant, MessageOrigin::Assistant, "work"),
    ]
}

fn response(text: &str, cursor: CursorUpdate) -> ModelResponse {
    ModelResponse {
        message_id: MessageId::new(format!("message-{text}")),
        message: AgentMessage::text(AgentRole::Assistant, MessageOrigin::Assistant, text),
        stop_reason: StopReason::EndTurn,
        usage: None,
        request_body: None,
        cursor,
    }
}

fn compactor(
    services: Arc<TactSharedServices>,
    work_dir: &std::path::Path,
    native: bool,
    hooks: &[Hook],
) -> TactCompactor {
    TactCompactor::new(
        services,
        Arc::new(SystemPromptAssembler::static_text("system")),
        "session",
        ModelIdentity {
            name: "mock".into(),
        },
        work_dir.to_path_buf(),
        settings(),
        Vec::new(),
        ProviderKind::OpenAi,
        native,
        false,
        SessionStartSource::Startup,
        CompactState::default(),
        hooks,
    )
}

#[tokio::test]
async fn replacement_commits_and_posts_only_after_kernel_acknowledgement() {
    let dir = tempfile::tempdir().unwrap();
    let posts = Arc::new(AtomicUsize::new(0));
    let posts_for_hook = posts.clone();
    let hooks = vec![Hook::PostCompact(Arc::new(
        move |_view: &crate::LoopState<'_>, trigger: CompactTrigger| {
            let posts = posts_for_hook.clone();
            Box::pin(async move {
                assert_eq!(trigger, CompactTrigger::Auto);
                posts.fetch_add(1, Ordering::AcqRel);
                Ok(HookControl::Continue)
            })
        },
    ))];
    let compactor = compactor(services(), dir.path(), false, &hooks);
    let model = ScriptedModel::new(false);
    model.push_complete(Ok(response(
        "summary",
        CursorUpdate::Replace { cursor: None },
    )));
    let transcript = transcript();
    let capabilities = model.capabilities();
    let session = SessionRef::new("session");
    let state = ContextView {
        session: &session,
        transcript: &transcript,
        cursor: None,
        total_turns: 1,
        max_total_turns: None,
        last_token_total: Some(100),
        pending_inputs: &[],
        capabilities: &capabilities,
        remaining_turns: None,
    };
    let request = CompactionRequest::Auto { focus: None };

    let outcome = Compactor::compact(
        &compactor,
        &state,
        request.clone(),
        &model,
        CancelToken::new(),
    )
    .await
    .unwrap();
    let CompactionOutcome::Replaced(ContextReplacement { transcript, cursor }) = outcome else {
        panic!("expected a replacement");
    };
    assert!(transcript.is_some());
    assert_eq!(cursor, CursorUpdate::Replace { cursor: None });
    assert!(!compactor.compact_state().has_compacted);
    assert!(!compactor.session_start_pending());
    assert_eq!(posts.load(Ordering::Acquire), 0);

    compactor.commit_pending(&state, &request).await.unwrap();
    assert!(compactor.compact_state().has_compacted);
    assert_eq!(
        compactor.compact_state().last_summary.as_deref(),
        Some("summary")
    );
    assert!(compactor.session_start_pending());
    assert_eq!(
        compactor.session_start_source(),
        SessionStartSource::Compact
    );
    assert_eq!(posts.load(Ordering::Acquire), 1);

    compactor.commit_pending(&state, &request).await.unwrap();
    assert_eq!(posts.load(Ordering::Acquire), 1);
}

#[tokio::test]
async fn every_full_compaction_kind_reaches_precompact_with_its_trigger() {
    let dir = tempfile::tempdir().unwrap();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let seen_for_hook = seen.clone();
    let hooks = vec![Hook::PreCompact(Arc::new(
        move |_view: &crate::LoopState<'_>, trigger: CompactTrigger| {
            let seen = seen_for_hook.clone();
            Box::pin(async move {
                seen.lock().unwrap().push(trigger);
                Ok(HookControl::Block("veto".into()))
            })
        },
    ))];
    let compactor = compactor(services(), dir.path(), false, &hooks);
    let model = ScriptedModel::new(false);
    let transcript = transcript();
    let capabilities = model.capabilities();
    let session = SessionRef::new("session");
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

    for request in [
        CompactionRequest::Auto { focus: None },
        CompactionRequest::Manual { focus: None },
        CompactionRequest::Recovery { focus: None },
        CompactionRequest::Command { focus: None },
    ] {
        let outcome = Compactor::compact(&compactor, &state, request, &model, CancelToken::new())
            .await
            .unwrap();
        assert!(matches!(outcome, CompactionOutcome::Skipped { .. }));
    }

    assert_eq!(
        *seen.lock().unwrap(),
        [
            CompactTrigger::Auto,
            CompactTrigger::Manual,
            CompactTrigger::Recovery,
            CompactTrigger::Command,
        ]
    );
    assert!(!compactor.compact_state().has_compacted);
    assert!(!compactor.session_start_pending());
}

#[tokio::test]
async fn native_compaction_returns_only_a_replacement_cursor() {
    let dir = tempfile::tempdir().unwrap();
    let compactor = compactor(services(), dir.path(), true, &[]);
    let model = ScriptedModel::new(true);
    let replacement = ModelCursor {
        provider: "mock".into(),
        version: 1,
        payload: serde_json::json!({"id": "cmp-1"}),
    };
    model.push_native(Ok(response(
        "ignored summary",
        CursorUpdate::Replace {
            cursor: Some(replacement.clone()),
        },
    )));
    let transcript = transcript();
    let capabilities = model.capabilities();
    let session = SessionRef::new("session");
    let state = ContextView {
        session: &session,
        transcript: &transcript,
        cursor: None,
        total_turns: 1,
        max_total_turns: None,
        last_token_total: Some(100),
        pending_inputs: &[],
        capabilities: &capabilities,
        remaining_turns: None,
    };

    let request = CompactionRequest::Command { focus: None };
    let outcome = Compactor::compact(
        &compactor,
        &state,
        request.clone(),
        &model,
        CancelToken::new(),
    )
    .await
    .unwrap();
    let CompactionOutcome::Replaced(ContextReplacement { transcript, cursor }) = outcome else {
        panic!("expected a native replacement");
    };
    assert!(transcript.is_none());
    assert_eq!(
        cursor,
        CursorUpdate::Replace {
            cursor: Some(replacement)
        }
    );
    assert!(!compactor.compact_state().has_compacted);
}

#[tokio::test]
async fn failed_compaction_does_not_stage_a_commit_or_post_hook() {
    let dir = tempfile::tempdir().unwrap();
    let posts = Arc::new(AtomicUsize::new(0));
    let posts_for_hook = posts.clone();
    let hooks = vec![Hook::PostCompact(Arc::new(
        move |_view: &crate::LoopState<'_>, _trigger: CompactTrigger| {
            let posts = posts_for_hook.clone();
            Box::pin(async move {
                posts.fetch_add(1, Ordering::AcqRel);
                Ok(HookControl::Continue)
            })
        },
    ))];
    let compactor = compactor(services(), dir.path(), false, &hooks);
    let model = ScriptedModel::new(false);
    model.push_complete(Err(ModelError::new(
        ModelErrorKind::Permanent,
        "summary failed",
    )));
    let transcript = transcript();
    let capabilities = model.capabilities();
    let session = SessionRef::new("session");
    let state = ContextView {
        session: &session,
        transcript: &transcript,
        cursor: None,
        total_turns: 1,
        max_total_turns: None,
        last_token_total: Some(100),
        pending_inputs: &[],
        capabilities: &capabilities,
        remaining_turns: None,
    };
    let request = CompactionRequest::Recovery { focus: None };

    let error = Compactor::compact(
        &compactor,
        &state,
        request.clone(),
        &model,
        CancelToken::new(),
    )
    .await
    .unwrap_err();
    assert!(error.to_string().contains("summary failed"));
    compactor.commit_pending(&state, &request).await.unwrap();
    assert!(!compactor.compact_state().has_compacted);
    assert!(!compactor.session_start_pending());
    assert_eq!(posts.load(Ordering::Acquire), 0);
}

#[tokio::test]
async fn micro_replacement_keeps_recent_results_without_full_post_state() {
    let dir = tempfile::tempdir().unwrap();
    let posts = Arc::new(AtomicUsize::new(0));
    let posts_for_hook = posts.clone();
    let hooks = vec![Hook::PostCompact(Arc::new(
        move |_view: &crate::LoopState<'_>, _trigger: CompactTrigger| {
            let posts = posts_for_hook.clone();
            Box::pin(async move {
                posts.fetch_add(1, Ordering::AcqRel);
                Ok(HookControl::Continue)
            })
        },
    ))];
    let compactor = compactor(services(), dir.path(), false, &hooks);
    let model = ScriptedModel::new(false);
    let transcript = vec![AgentMessage::new(
        AgentRole::User,
        MessageOrigin::ToolResult,
        (0..13)
            .map(|index| AgentContentBlock::ToolResult {
                tool_use_id: format!("call-{index}"),
                content: "x".repeat(160),
            })
            .collect(),
    )];
    let capabilities = model.capabilities();
    let session = SessionRef::new("session");
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
    let request = CompactionRequest::Micro;

    let outcome = Compactor::compact(
        &compactor,
        &state,
        request.clone(),
        &model,
        CancelToken::new(),
    )
    .await
    .unwrap();
    let CompactionOutcome::Replaced(ContextReplacement { transcript, cursor }) = outcome else {
        panic!("expected a micro replacement");
    };
    assert_eq!(cursor, CursorUpdate::Unchanged);
    let transcript = transcript.expect("micro compaction replaces the transcript");
    let compacted = &transcript[0].content;
    assert!(matches!(
        &compacted[0],
        AgentContentBlock::ToolResult { content, .. } if content == COMPACTED_TOOL_RESULT
    ));
    for block in &compacted[1..] {
        assert!(matches!(
            block,
            AgentContentBlock::ToolResult { content, .. } if content == &"x".repeat(160)
        ));
    }

    compactor.commit_pending(&state, &request).await.unwrap();
    assert!(!compactor.compact_state().has_compacted);
    assert!(!compactor.session_start_pending());
    assert_eq!(posts.load(Ordering::Acquire), 0);
}

#[test]
fn compaction_kinds_map_to_stable_hook_triggers() {
    assert_eq!(trigger_for(CompactionKind::Auto), CompactTrigger::Auto);
    assert_eq!(trigger_for(CompactionKind::Manual), CompactTrigger::Manual);
    assert_eq!(
        trigger_for(CompactionKind::Recovery),
        CompactTrigger::Recovery
    );
    assert_eq!(
        trigger_for(CompactionKind::Command),
        CompactTrigger::Command
    );
    assert_eq!(trigger_for(CompactionKind::Micro), CompactTrigger::Auto);
}

#[test]
fn compact_response_note_renders_the_providers_numbers() {
    let usage = TokenUsageInfo {
        prompt: 23_470,
        completion: 4_000,
        total: 27_470,
        prompt_cache_hit_tokens: 0,
        prompt_cache_miss_tokens: 23_470,
        reasoning_tokens: 4_000,
    };
    assert_eq!(
        compact_response_note(Some(&StopReason::MaxTokens), Some(&usage)),
        "stop=max_tokens, prompt 23470, completion 4000 (reasoning 4000), cache 0/23470"
    );
    assert_eq!(
        compact_response_note(Some(&StopReason::EndTurn), None),
        "stop=end_turn, no usage reported"
    );
    assert_eq!(
        compact_response_note(None, None),
        "stop=none, no usage reported"
    );
    assert_eq!(
        compact_response_note(
            Some(&StopReason::Unknown {
                raw: "paused".into()
            }),
            None
        ),
        "stop=unknown(paused), no usage reported"
    );
}

#[test]
fn compact_truncation_note_labels_both_units() {
    assert_eq!(
        compact_truncation_note(Some(4_000), 15_314),
        "4000 reasoning tokens, thinking block 15314 bytes"
    );
    assert_eq!(
        compact_truncation_note(None, 15_314),
        "thinking block 15314 bytes"
    );
}
