//! Agent runtime: conversation loop, tool dispatch, and session state.

mod tool_dispatch;
pub(crate) mod tool_schedule;

use std::path::Path;

use anyhow::{Context, Result};
use chrono::Utc;
use tact_llm::{
    ContentBlock, CreateMessageParams, LlmClient, LlmProvider, Message, MessageContent,
    OpenAiReasoningEffort, ProviderConversationState, ProviderKind, ProviderStateUpdate,
    RequiredMessageParams, Role, StopReason, Thinking, ThinkingType,
};
use tact_protocol::{AgentUpdate, TokenUsageInfo};

use crate::{
    ToolSpec,
    compact::{
        CompactState, approx_text_tokens, build_compacted_history, collect_user_messages,
        compact_rebuild_headroom_tokens, compacted_context, estimate_context_tokens,
        estimate_message_tokens, micro_compact, recent_messages_for_summary,
        retained_user_message_token_budget, should_auto_compact, write_transcript,
    },
    config::{self, AgentSettings},
    hook::{Hook, HookControl, HookTypes, PostToolUseFn, PreToolUseFn, SessionStartFn},
    invoke_hooks,
    mcp::MCPToolRouter,
    memory::MEMORY_GUIDANCE,
    permission::PermissionManager,
    prompt::{SystemPrompt, responses_prompt_template},
    recovery::{
        MAX_COMPACT_ATTEMPTS, MAX_COMPACT_SUMMARY_RETRY_ATTEMPTS, MAX_CONTINUATION_ATTEMPTS,
        MAX_TRANSPORT_ATTEMPTS, RecoveryState, backoff_delay, continuation_message, error_summary,
        is_prompt_too_long_error, is_transient_transport_error,
    },
    stats::SessionStats,
    store::DynSessionStore,
    tool::{ToolContext, ToolRouter},
};

enum CompactRebuildMode {
    /// Retain recent real user messages + handoff summary (Codex-style).
    CodexStyle,
    /// Replace the entire context with a single summary user message.
    LegacySingleSummary,
}

const COMPACT_SUMMARY_MAX_TOKENS: u32 = 2_000;
const COMPACT_SUMMARY_OUTPUT_PERCENT: usize = 20;
const COMPACT_SUMMARY_HEADROOM_PERCENT: usize = 10;
/// Auto-compact threshold percentage used for the Responses usage-only
/// trigger. Must stay in sync with `crate::compact::should_auto_compact`
/// (its threshold constant is private to that module).
const RESPONSES_AUTO_COMPACT_THRESHOLD_PERCENT: usize = 80;
const COMPACT_SUMMARY_INSTRUCTIONS: &str = "Summarize this coding-agent conversation so work can continue.\n\
Preserve:\n\
1. The current goal and what has been accomplished\n\
2. Important findings, decisions, and architectural insights\n\
3. Files read or changed (with key code structures like types, signatures, APIs if relevant)\n\
4. Remaining work and next steps\n\
5. User constraints and preferences\n\
6. Any errors encountered and their causes\n\
Be compact but concrete. Preserve exact file paths, function names, and type signatures when they are important for continuing the work.";

/// Shared state for a running agent session.
///
/// Holds the LLM client, conversation context, compaction and recovery
/// state, the permission manager, and an optional TUI update channel.
pub struct AgentRuntime {
    pub client: LlmProvider,
    pub context: Vec<Message>,
    pub compact_state: CompactState,
    pub recovery_state: RecoveryState,
    pub permission_manager: PermissionManager,
    pub stats: SessionStats,
    pub ui_tx: Option<tokio::sync::mpsc::UnboundedSender<AgentUpdate>>,
    pub cancel_flag: std::sync::Arc<std::sync::atomic::AtomicBool>,
    pub session_store: Option<DynSessionStore>,
    /// Set together with [`Self::session_store`] via [`Agent::with_session`] at startup.
    pub session_id: Option<String>,
    /// DB row id of the first message persisted for the current LLM-call window.
    pub first_message_db_id: i64,
    /// DB row id of the last message persisted for the current LLM-call window.
    pub last_message_db_id: i64,
    /// `last_message_db_id` at the time the most recent LLM call was persisted
    /// (before the assistant response row is written). Used to attach tool schedules.
    pub llm_call_last_message_id: i64,
    /// Cached project-directory snapshot, computed once per session so the
    /// deterministic output doesn't churn the DeepSeek prefix KV-cache.
    pub cached_dir_snapshot: Option<String>,
    /// Cached `CLAUDE.md` assembly (once per session) for a stable prompt prefix.
    pub cached_claude_md: Option<String>,
    /// Cached `AGENTS.md` assembly (once per session) for a stable prompt prefix.
    pub cached_agents_md: Option<String>,
    /// Total tokens from the most recent LLM usage report (`0` = none yet).
    pub last_token_total: u32,
    /// Provider-specific conversation state (currently only the OpenAI
    /// Responses protocol baseline). Loaded in [`Agent::ensure_session`],
    /// committed after every LLM response, and passed into every LLM call.
    pub provider_state: Option<ProviderConversationState>,
}

/// How the agent builds its system prompt.
///
/// - `Dynamic`: rendered from a Tera template with live context (skills, memory, etc.).
/// - `Static(String)`: uses a fixed string (used for sub-agents).
pub enum AgentSystemPrompt {
    Dynamic,
    Static(String),
}

/// The main agent struct.
///
/// Owns the runtime state, tool router (native), MCP router (external tools),
/// hooks list, and system prompt configuration.
pub struct Agent {
    pub runtime: AgentRuntime,
    pub tool_context: ToolContext,
    pub tools: ToolRouter,
    pub mcp_router: MCPToolRouter,
    pub hooks: Vec<Hook>,
    pub system_prompt: AgentSystemPrompt,
    pub tool_use_counter: usize,
    /// Snapshot of agent settings at construction; avoids parallel tests racing on global config.
    agent_settings: AgentSettings,
    /// Provider kind captured at construction (or overridden via
    /// [`Self::with_provider_kind`]); lets Responses routing distinguish
    /// OpenAI (native compaction) from DeepSeek (local summary fallback)
    /// without reading process-global provider state.
    provider_kind: ProviderKind,
    cached_tool_specs: Vec<ToolSpec>,
}

impl Agent {
    pub fn new(
        client: LlmProvider,
        mut tool_context: ToolContext,
        tools: ToolRouter,
        mcp_router: MCPToolRouter,
        permission_manager: PermissionManager,
        system_prompt: AgentSystemPrompt,
    ) -> Self {
        // Responses models do not expose Tact's local `compact` tool: the
        // user `/compact` command and automatic triggers dispatch to the
        // native `/responses/compact` endpoint instead. MCP tools are kept
        // unchanged.
        let provider_kind = ProviderKind::OpenAi;
        let native_specs = if matches!(client, LlmProvider::OpenAiResponses(_)) {
            tools
                .tool_specs()
                .into_iter()
                .filter(|spec| spec.name != "compact")
                .collect()
        } else {
            tools.tool_specs()
        };
        let cached_tool_specs: Vec<ToolSpec> = native_specs
            .into_iter()
            .chain(mcp_router.all_tools())
            .collect();
        let cancel_flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        tool_context.cancel_flag = cancel_flag.clone();
        Self {
            runtime: AgentRuntime {
                client,
                context: Vec::new(),
                compact_state: CompactState::default(),
                recovery_state: RecoveryState::default(),
                permission_manager,
                stats: SessionStats::default(),
                ui_tx: None,
                cancel_flag,
                session_store: None,
                session_id: None,
                first_message_db_id: 0,
                last_message_db_id: 0,
                llm_call_last_message_id: 0,
                cached_dir_snapshot: None,
                cached_claude_md: None,
                cached_agents_md: None,
                last_token_total: 0,
                provider_state: None,
            },
            tool_context,
            tools,
            mcp_router,
            hooks: Vec::new(),
            system_prompt,
            tool_use_counter: 0,
            agent_settings: crate::config::settings().agent.clone(),
            provider_kind,
            cached_tool_specs,
        }
    }

    /// Override the provider kind used for Responses compaction routing
    /// (OpenAI → native `/responses/compact`; DeepSeek → local summary
    /// fallback because its endpoint lacks the endpoint).
    pub fn with_provider_kind(mut self, provider_kind: ProviderKind) -> Self {
        self.provider_kind = provider_kind;
        self
    }

    /// Override agent-loop settings (used by integration tests with custom config).
    pub fn with_agent_settings(mut self, settings: AgentSettings) -> Self {
        self.agent_settings = settings;
        self
    }

    fn model_context_window(&self) -> usize {
        self.agent_settings.model_context_window
    }

    fn max_tokens(&self) -> u32 {
        self.agent_settings.max_tokens
    }

    /// Whether the auto-compact trigger should fire before the next LLM call.
    ///
    /// For OpenAI Responses the provider-reported usage reflects the actual
    /// wire input size (the compacted protocol baseline), so it is the only
    /// authoritative trigger: native compaction resets the wire baseline
    /// without shrinking the logical context, so the logical-context estimate
    /// must never drive client-side auto compaction (it would re-fire on an
    /// already-compacted context forever). Other providers keep the existing
    /// estimate-based trigger.
    fn auto_compact_due(&self, incoming_tokens: usize) -> bool {
        if self.is_openai_responses() {
            // Usage-only trigger: no usage yet means there is nothing to
            // compact; a max_tokens-dominated fallback must not fire, because
            // it would loop on small windows where max_tokens alone crosses
            // the threshold.
            if self.model_context_window() == 0 || self.runtime.last_token_total == 0 {
                return false;
            }
            let threshold = self
                .model_context_window()
                .saturating_mul(RESPONSES_AUTO_COMPACT_THRESHOLD_PERCENT)
                .div_ceil(100);
            let projected = (self.runtime.last_token_total as usize)
                .saturating_add(incoming_tokens)
                .saturating_add(self.max_tokens() as usize);
            return projected >= threshold;
        }
        should_auto_compact(
            self.runtime.last_token_total,
            self.model_context_window(),
            estimate_context_tokens(&self.runtime.context),
            incoming_tokens,
            self.max_tokens() as usize,
        )
    }

    /// If thinking budget is active (>0) and `max_tokens` is not strictly larger,
    /// auto-expand `max_tokens` and return a warning message suitable for the UI.
    fn ensure_max_tokens_gt_thinking_budget(
        max_tokens: &mut u32,
        thinking_budget: usize,
    ) -> Option<String> {
        if thinking_budget == 0 {
            return None;
        }
        let mt = *max_tokens as usize;
        if mt > thinking_budget {
            return None;
        }
        let new_max: u32 = (thinking_budget + 1).try_into().unwrap_or(u32::MAX);
        *max_tokens = new_max;
        Some(format!(
            "thinking_budget ({thinking_budget}) >= max_tokens; expanded max_tokens to {new_max}"
        ))
    }

    /// Update the thinking budget used when constructing subsequent LLM requests.
    /// An in-flight request already owns its `CreateMessageParams` and is unchanged.
    ///
    /// If the new budget is active and not strictly smaller than the current
    /// `max_tokens`, `max_tokens` is automatically expanded to `budget + 1` and a
    /// warning is emitted to the UI channel.
    ///
    /// Always emits [`AgentUpdate::ModelInfo`] so the TUI status bar resyncs after
    /// `/model` picks: `SetThinkingBudget` is queued behind an in-flight task, and
    /// that task's older `ModelInfo` would otherwise leave the bar stuck on the
    /// previous budget.
    pub fn set_thinking_budget(&mut self, budget: usize) {
        self.agent_settings.thinking_budget = budget;
        if let Some(msg) =
            Self::ensure_max_tokens_gt_thinking_budget(&mut self.agent_settings.max_tokens, budget)
        {
            self.emit_update(AgentUpdate::Info(msg));
        }
        self.emit_model_status();
    }

    /// Update this agent's session model (per-agent; never the global provider).
    ///
    /// Same queue semantics as [`set_thinking_budget`]: the TUI sends
    /// `UserCommand::SetModel` behind an in-flight task; the resulting
    /// `ModelInfo` resyncs the status bar.
    pub fn set_model(&mut self, model: String) {
        if model.trim().is_empty() {
            return;
        }
        self.agent_settings.model = model;
        self.emit_model_status();
    }

    /// Update this agent's session reasoning effort (per-agent; never the
    /// global provider). `None` clears the explicit effort (wire omits it).
    ///
    /// Same queue semantics as [`set_thinking_budget`].
    pub fn set_reasoning_effort(&mut self, effort: Option<OpenAiReasoningEffort>) {
        self.agent_settings.reasoning_effort = effort;
        self.emit_model_status();
    }

    /// Push current model / token / thinking settings to the TUI status bar.
    fn emit_model_status(&self) {
        let model_name = self.agent_settings.model.clone();
        let budget = self.thinking_budget();
        self.emit_update(AgentUpdate::ModelInfo(tact_protocol::ModelCallParams {
            model: model_name,
            max_tokens: self.max_tokens(),
            thinking_budget: (budget > 0).then_some(budget as u32),
            reasoning_effort: self
                .agent_settings
                .reasoning_effort
                .map(|effort| effort.as_str().to_string()),
            extra_body: None,
        }));
    }

    fn thinking_budget(&self) -> usize {
        self.agent_settings.thinking_budget
    }

    fn thinking_config(&self) -> Thinking {
        Thinking {
            budget_tokens: self.thinking_budget(),
            type_: ThinkingType::Enabled,
        }
    }

    /// Attaches a TUI update channel so the agent can stream events
    /// (token usage, thinking blocks, tool results) to the terminal.
    pub fn with_ui_channel(mut self, tx: tokio::sync::mpsc::UnboundedSender<AgentUpdate>) -> Self {
        self.runtime.ui_tx = Some(tx.clone());
        // Keep tool_context in sync so ToolProgress / tool helpers use the same
        // channel (critical for tagged subagent ui_tx).
        self.tool_context.ui_tx = Some(tx);
        self
    }

    /// Attach a session store with a fully initialized session id.
    ///
    /// Callers must create/resolve the id and persist the session row before
    /// this (startup path). Also wires DeepSeek `user_id` for KV cache isolation.
    pub fn with_session(mut self, session_id: String, store: DynSessionStore) -> Self {
        self.runtime.client.set_user_id(&session_id);
        self.tool_context.session_id = Some(session_id.clone());
        self.tool_context.session_store = Some(store.clone());
        self.runtime.session_store = Some(store);
        self.runtime.session_id = Some(session_id);
        self
    }

    /// Gracefully disconnect all MCP server child processes.
    pub async fn shutdown_mcp(&mut self) {
        self.mcp_router.disconnect_all().await;
    }

    pub fn emit_update(&self, update: AgentUpdate) {
        // Desktop notifications for key lifecycle events
        match &update {
            AgentUpdate::TaskComplete(text) => {
                let summary = text.chars().take(200).collect::<String>();
                let _ = crate::notifications::notify_task_complete(&summary);
            }
            AgentUpdate::StepFailed { idx, error, .. } => {
                let _ = crate::notifications::notify_step_failed(*idx, error);
            }
            _ => {}
        }

        if let Some(tx) = &self.runtime.ui_tx {
            let _ = tx.send(update);
        }
    }

    /// Load persisted history into an empty context.
    ///
    /// Session id and store must already be set via [`Self::with_session`];
    /// this does not allocate a new id.
    pub async fn ensure_session(&mut self) -> Result<String> {
        let Some(store) = self.runtime.session_store.as_ref() else {
            return Ok(self.runtime.session_id.clone().unwrap_or_default());
        };

        let session_id = self
            .runtime
            .session_id
            .clone()
            .context("session_id must be set via with_session before ensure_session")?;

        // Idempotent: startup normally created the row already.
        let root_dir = self.tool_context.work_dir.display().to_string();
        store.ensure_session_row(&session_id, &root_dir, "").await?;

        if self.runtime.context.is_empty() {
            let history = store.load_session(&session_id).await?;
            self.runtime.context = history;
        }

        if self.runtime.provider_state.is_none() {
            let state = store.load_provider_state(&session_id).await?;
            if let Some(state) = state {
                // Reject a persisted state bound to another provider, base
                // URL, or model before any LLM call is allowed.
                self.validate_provider_state_binding(&state)?;
                self.runtime.provider_state = Some(state);
            }
        }

        Ok(session_id)
    }

    /// Validate a loaded Responses provider state against the active client.
    ///
    /// The state records the provider name, base URL, and request model it was
    /// created for. Reusing it with a different provider/base URL/model would
    /// silently corrupt the conversation, so a mismatch is a hard error and is
    /// never silently reset or dropped.
    fn validate_provider_state_binding(&self, state: &ProviderConversationState) -> Result<()> {
        let ProviderConversationState::OpenAiResponses(inner) = state;
        let LlmProvider::OpenAiResponses(adapter) = &self.runtime.client else {
            anyhow::bail!(
                "provider state is stored for provider '{}', but the active client is not OpenAI Responses",
                inner.provider
            );
        };
        if inner.provider != "openai_responses" {
            anyhow::bail!(
                "provider state is bound to provider '{}', expected 'openai_responses'",
                inner.provider
            );
        }
        if inner.base_url != adapter.base_url() {
            anyhow::bail!(
                "provider state is bound to base URL '{}', expected '{}'",
                inner.base_url,
                adapter.base_url()
            );
        }
        let model = self.agent_settings.model.clone();
        if inner.model != model {
            anyhow::bail!(
                "provider state is bound to model '{}', expected '{}'",
                inner.model,
                model
            );
        }
        Ok(())
    }

    fn is_openai_responses(&self) -> bool {
        matches!(self.runtime.client, LlmProvider::OpenAiResponses(_))
    }

    async fn push_message(&mut self, message: Message) -> Result<()> {
        let blocks = message.content.clone();
        let role = message.role;
        self.runtime.context.push(message);
        if self.is_openai_responses() {
            // Persist messages and the (unchanged) provider state atomically
            // so a crash never leaves a state anchor ahead of the messages.
            let provider_state = self.runtime.provider_state.clone();
            if let Err(error) = self
                .replace_persisted_context_and_state(provider_state.as_ref())
                .await
            {
                self.runtime.context.pop();
                return Err(error);
            }
            Ok(())
        } else {
            self.persist_message(role, &blocks).await
        }
    }

    async fn persist_message(&mut self, role: Role, content: &MessageContent) -> Result<()> {
        let Some(store) = self.runtime.session_store.as_ref() else {
            return Ok(());
        };
        let Some(session_id) = self.runtime.session_id.as_ref() else {
            return Ok(());
        };
        let ordinal = self.runtime.context.len() as i64;
        let db_id = store
            .append_message(session_id, role, content, ordinal)
            .await?;
        if self.runtime.first_message_db_id == 0 {
            self.runtime.first_message_db_id = db_id;
        }
        self.runtime.last_message_db_id = db_id;
        Ok(())
    }

    async fn replace_persisted_context(&mut self) -> Result<()> {
        let Some(store) = self.runtime.session_store.as_ref() else {
            return Ok(());
        };
        let Some(session_id) = self.runtime.session_id.as_ref() else {
            return Ok(());
        };

        let (first_id, last_id) = store
            .replace_session_messages(session_id, &self.runtime.context)
            .await?;
        self.runtime.first_message_db_id = first_id;
        self.runtime.last_message_db_id = last_id;
        Ok(())
    }

    /// Atomically replace the persisted messages and provider state in one
    /// transaction. Used by Responses paths so the logical context and the
    /// protocol baseline can never diverge on disk.
    async fn replace_persisted_context_and_state(
        &mut self,
        provider_state: Option<&ProviderConversationState>,
    ) -> Result<()> {
        let Some(store) = self.runtime.session_store.as_ref() else {
            return Ok(());
        };
        let Some(session_id) = self.runtime.session_id.as_ref() else {
            return Ok(());
        };

        let (first_id, last_id) = store
            .replace_session_messages_and_provider_state(
                session_id,
                &self.runtime.context,
                provider_state,
            )
            .await?;
        self.runtime.first_message_db_id = first_id;
        self.runtime.last_message_db_id = last_id;
        Ok(())
    }

    /// Persist token usage and/or request body for an LLM call.
    /// Links to the message range that was sent ([first_message_db_id .. last_message_db_id]).
    async fn persist_llm_call(
        &self,
        call_type: &str,
        usage: Option<&TokenUsageInfo>,
        request_body: Option<&[u8]>,
    ) -> Result<()> {
        if usage.is_none() && request_body.is_none() {
            return Ok(());
        }
        let Some(store) = self.runtime.session_store.as_ref() else {
            return Ok(());
        };
        let Some(session_id) = self.runtime.session_id.as_ref() else {
            return Ok(());
        };
        store
            .record_token_usage(
                session_id,
                call_type,
                usage,
                self.runtime.first_message_db_id,
                self.runtime.last_message_db_id,
                request_body,
            )
            .await?;
        Ok(())
    }

    /// Persist the tool-schedule summary for the current turn, attaching it to
    /// the token-usage row of the LLM call that produced these tool calls
    /// (keyed by the assistant message id). Best-effort: failures are ignored.
    async fn persist_tool_schedule(&self, summary: &tool_schedule::ToolScheduleSummary) {
        let Some(store) = self.runtime.session_store.as_ref() else {
            return;
        };
        let Some(session_id) = self.runtime.session_id.as_ref() else {
            return;
        };
        if let Ok(json) = serde_json::to_string(summary) {
            let anchor = self.runtime.llm_call_last_message_id;
            if anchor > 0 {
                let _ = store.record_tool_schedule(session_id, anchor, &json).await;
            }
        }
    }

    fn next_step_idx(&mut self) -> usize {
        let idx = self.tool_use_counter;
        self.tool_use_counter += 1;
        idx
    }

    /// The main agent conversation loop.
    ///
    /// 1. Builds the system prompt and primes the context.
    /// 2. Loops: sends context to LLM → processes streaming response →
    ///    dispatches tool-use blocks (native or MCP) → applies permissions →
    ///    writes results back.  Continues until the LLM returns a stop reason
    ///    other than `ToolUse` or an unrecoverable error occurs.
    #[tracing::instrument(skip(self), name = "agent_loop")]
    pub async fn agent_loop(&mut self, user_turn_message: Option<Message>) -> Result<()> {
        self.runtime.recovery_state = RecoveryState::default();

        // Restore history if the startup path left context empty.
        self.ensure_session().await?;

        // Codex-style pre-turn: compact *old* history before appending this
        // turn's user message, reserving space for the incoming prompt so we
        // do not overflow immediately after push.
        let incoming_tokens = user_turn_message
            .as_ref()
            .map(estimate_message_tokens)
            .unwrap_or(0);
        if self.auto_compact_due(incoming_tokens) {
            self.emit_update(AgentUpdate::Info("[auto compact]".into()));
            self.compact_history(None).await?;
        }
        if let Some(message) = user_turn_message {
            self.push_message(message).await?;
        }

        // Build the system prompt once per task. Memory saved mid-task takes
        // effect on the next task; stable sections stay before DYNAMIC_BOUNDARY
        // so the prefix KV-cache holds across turns and tasks.
        let system_prompt = self.build_system_prompt()?;
        loop {
            if self
                .runtime
                .cancel_flag
                .load(std::sync::atomic::Ordering::Relaxed)
            {
                self.emit_update(AgentUpdate::Info("Cancelled by user".into()));
                return Ok(());
            }
            // Micro-compaction truncates old tool results in the logical
            // context. For Responses the provider state anchors the logical
            // prefix by hash, so mutating covered messages would invalidate
            // the baseline; the wire-level input is already incremental, so
            // there is nothing to shrink client-side.
            if !self.is_openai_responses() {
                micro_compact(
                    &mut self.runtime.context,
                    self.agent_settings.micro_compact_enabled,
                );
            }
            // Turn already in context — no incoming reservation.
            if self.auto_compact_due(0) {
                self.emit_update(AgentUpdate::Info("[auto compact]".into()));
                self.compact_history(None).await?;
            }

            // Defense-in-depth: if max_tokens <= thinking_budget (e.g. after a runtime
            // /model pick), auto-expand max_tokens and warn once.
            let thinking_budget = self.thinking_budget();
            if let Some(msg) = Self::ensure_max_tokens_gt_thinking_budget(
                &mut self.agent_settings.max_tokens,
                thinking_budget,
            ) {
                self.emit_update(AgentUpdate::Info(msg));
            }

            // Snapshot the complete conversation after micro/auto compaction.
            // Includes the current user turn plus history, or retained users +
            // summary when compact_history ran above.
            let conversation_messages = self.runtime.context.clone();
            let model_name = self.agent_settings.model.clone();
            let request = CreateMessageParams::new(RequiredMessageParams {
                model: model_name.clone(),
                messages: conversation_messages,
                max_tokens: self.max_tokens(),
            })
            .with_system(&system_prompt)
            .with_tools(self.all_tool_specs())
            .with_stream(true)
            .with_thinking(self.thinking_config())
            .with_reasoning_effort(self.agent_settings.reasoning_effort);

            self.emit_update(AgentUpdate::ModelInfo(tact_protocol::ModelCallParams {
                model: model_name,
                max_tokens: request.max_tokens,
                thinking_budget: request.thinking.as_ref().map(|t| t.budget_tokens as u32),
                reasoning_effort: self
                    .agent_settings
                    .reasoning_effort
                    .map(|effort| effort.as_str().to_string()),
                extra_body: request
                    .thinking
                    .as_ref()
                    .map(|t| serde_json::json!({"thinking": t}).to_string()),
            }));

            // ── Stats: before LLM call ──
            self.runtime.stats.prompt_count += 1;
            let prompt_chars = serde_json::to_string(&request)
                .map(|s| s.chars().count() as u64)
                .unwrap_or(0);
            self.runtime.stats.total_prompt_chars += prompt_chars;
            let llm_call_start = std::time::Instant::now();

            let (content, stop_reason, token_usage, request_body, state_update) = match self
                .stream_message(&request)
                .await
            {
                Ok(result) => {
                    self.runtime.recovery_state.transport_attempts = 0;
                    result
                }
                Err(error) => {
                    let error_text = error.to_string().to_lowercase();
                    if is_prompt_too_long_error(&error_text)
                        && self.runtime.recovery_state.compact_attempts < MAX_COMPACT_ATTEMPTS
                    {
                        self.runtime.recovery_state.compact_attempts += 1;
                        self.emit_update(AgentUpdate::Info(format!(
                            "[Recovery] compact ({}/{}): context too large",
                            self.runtime.recovery_state.compact_attempts, MAX_COMPACT_ATTEMPTS
                        )));
                        self.compact_history(None).await?;
                        continue;
                    }

                    if is_transient_transport_error(&error_text)
                        && self.runtime.recovery_state.transport_attempts < MAX_TRANSPORT_ATTEMPTS
                    {
                        let delay = backoff_delay(self.runtime.recovery_state.transport_attempts);
                        self.runtime.recovery_state.transport_attempts += 1;
                        let summary = error_summary(
                            &error
                                .chain()
                                .map(|cause| cause.to_string())
                                .collect::<Vec<_>>()
                                .join(": "),
                        );
                        self.emit_update(AgentUpdate::Info(format!(
                            "[Recovery] backoff ({}/{}): retrying in {:.1}s — {summary}",
                            self.runtime.recovery_state.transport_attempts,
                            MAX_TRANSPORT_ATTEMPTS,
                            delay.as_secs_f64()
                        )));
                        tokio::time::sleep(delay).await;
                        continue;
                    }

                    return Err(anyhow::anyhow!(error));
                }
            };

            // ── Stats: after LLM call ──
            self.runtime
                .stats
                .llm_call_durations
                .push(llm_call_start.elapsed());
            let response_chars = serde_json::to_string(&content)
                .map(|s| s.chars().count() as u64)
                .unwrap_or(0);
            self.runtime.stats.total_response_chars += response_chars;
            for block in &content {
                if let ContentBlock::Thinking { thinking, .. } = block {
                    self.runtime.stats.thinking_blocks += 1;
                    self.runtime.stats.total_thinking_chars += thinking.chars().count() as u64;
                }
            }

            if let Some(ref usage) = token_usage {
                self.runtime.stats.record_token_usage(usage);
                self.runtime.last_token_total = usage.total;
            }
            self.runtime.llm_call_last_message_id = self.runtime.last_message_db_id;
            let _ = self
                .persist_llm_call("stream", token_usage.as_ref(), request_body.as_deref())
                .await;

            // REVIEW: Persisting a truncated assistant message can leave an empty
            // OpenAI assistant message on the next turn (e.g. only a thinking block
            // that convert.rs drops, or an orphaned tool-call that gets stripped).
            // sanitize_assistant_messages in tact_llm::convert currently patches this,
            // but a cleaner fix might be to avoid adding a purely-empty assistant
            // message to the context here in the first place.
            self.runtime
                .context
                .push(Message::new_blocks(Role::Assistant, content.clone()));

            // Check whether the truncated response contains pending tool calls.
            // OpenAI requires every assistant message with tool_calls to be
            // immediately followed by tool-result messages for each id.
            let has_pending_tools = content
                .iter()
                .any(|b| matches!(b, ContentBlock::ToolUse { .. }));

            if self.is_openai_responses() {
                // Commit the assistant message together with the provider
                // state baseline in one transaction before any further LLM
                // call or tool execution. On failure the loop stops and the
                // tool is not executed; in-memory context is rolled back so
                // the old committed state stays intact.
                let candidate_state = match state_update {
                    ProviderStateUpdate::Replace(state) => Some(state),
                    ProviderStateUpdate::Unchanged => self.runtime.provider_state.clone(),
                };
                if let Err(error) = self
                    .replace_persisted_context_and_state(candidate_state.as_ref())
                    .await
                {
                    self.runtime.context.pop();
                    return Err(error);
                }
                self.runtime.provider_state = candidate_state;
            } else {
                self.persist_message(
                    Role::Assistant,
                    &MessageContent::Blocks {
                        content: content.clone(),
                    },
                )
                .await?;
            }

            if matches!(stop_reason, Some(StopReason::MaxTokens))
                && self.runtime.recovery_state.continuation_attempts < MAX_CONTINUATION_ATTEMPTS
            {
                // Execute any tool calls that arrived before the token limit
                // was hit, so the context remains valid for the API.
                if has_pending_tools {
                    let (tool_result, manual_compact) = self.execute_tool_call(&content).await?;
                    self.push_message(Message::new_blocks(Role::User, tool_result.clone()))
                        .await?;
                    if let Some(focus) = manual_compact {
                        self.emit_update(AgentUpdate::Info("[manual compact]".into()));
                        self.compact_history(Some(focus.as_str())).await?;
                    }
                }

                self.runtime.recovery_state.continuation_attempts += 1;
                self.emit_update(AgentUpdate::Info(format!(
                    "[Recovery] continue ({}/{}): output truncated",
                    self.runtime.recovery_state.continuation_attempts, MAX_CONTINUATION_ATTEMPTS
                )));
                let continuation_message =
                    continuation_message(self.runtime.recovery_state.continuation_attempts);
                self.push_message(Message::new_text(Role::User, continuation_message))
                    .await?;
                continue;
            }
            self.runtime.recovery_state.continuation_attempts = 0;

            // Stop-reason handling follows Anthropic guidance:
            // https://platform.claude.com/docs/en/build-with-claude/handling-stop-reasons
            // - end_turn / stop_sequence → finish
            // - max_tokens → continuation path above (or finish if attempts exhausted)
            // - tool_use → execute tools and loop
            // - refusal → surface clearly (HTTP 200, not a transport error); no auto
            //   model fallback yet — see refusals-and-fallback docs
            // - pause_turn → mapped to EndTurn in tact_llm (no Anthropic server tools)
            match &stop_reason {
                Some(StopReason::ToolUse) => {}
                Some(StopReason::Refusal) => {
                    let info_msg =
                        "Model refused this request (stop_reason=refusal). Try rephrasing, \
                         or switch to another model with different safety filters."
                            .to_string();
                    self.emit_update(AgentUpdate::Info(info_msg));
                    return Err(anyhow::anyhow!(
                        "model refused to process this request (stop_reason=refusal)"
                    ));
                }
                Some(StopReason::Unknown(raw)) => {
                    self.emit_update(AgentUpdate::Info(format!(
                        "Unrecognized stop_reason={raw:?}; treating as end of turn"
                    )));
                    return Ok(());
                }
                // PauseTurn: Tact does not use Anthropic server tools; finish like EndTurn.
                Some(
                    StopReason::EndTurn
                    | StopReason::StopSequence
                    | StopReason::MaxTokens
                    | StopReason::PauseTurn,
                )
                | None => {
                    return Ok(());
                }
            }

            if self
                .runtime
                .cancel_flag
                .load(std::sync::atomic::Ordering::Relaxed)
            {
                self.emit_update(AgentUpdate::Info("Cancelled by user".into()));
                return Ok(());
            }
            let (tool_result, manual_compact) = self.execute_tool_call(&content).await?;

            self.push_message(Message::new_blocks(Role::User, tool_result.clone()))
                .await?;

            if let Some(focus) = manual_compact {
                self.emit_update(AgentUpdate::Info("[manual compact]".into()));
                self.compact_history(Some(focus.as_str())).await?;
            }
        }
    }

    async fn stream_message(
        &mut self,
        request: &CreateMessageParams,
    ) -> Result<
        (
            Vec<ContentBlock>,
            Option<StopReason>,
            Option<TokenUsageInfo>,
            Option<tact_llm::LlmRequestBody>,
            ProviderStateUpdate,
        ),
        anyhow::Error,
    > {
        let ui_tx = self.runtime.ui_tx.clone();
        let response = self
            .runtime
            .client
            .stream_message(request, self.runtime.provider_state.as_ref(), ui_tx)
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        Ok((
            response.blocks,
            response.stop_reason,
            response.usage,
            response.request_body,
            response.state_update,
        ))
    }

    pub fn with_session_start(mut self, hook: impl SessionStartFn + 'static) -> Self {
        self.hooks.push(Hook::SessionStart(Box::new(hook)));
        self
    }

    pub fn with_post_tool(mut self, hook: impl PostToolUseFn + 'static) -> Self {
        // RTK filter is opt-in — defaults to off for privacy.
        if !config::settings().tools.rtk_filter {
            return self;
        }
        self.hooks.push(Hook::PostToolUse(Box::new(hook)));
        self
    }

    pub fn with_pre_tool(mut self, hook: impl PreToolUseFn + 'static) -> Self {
        self.hooks.push(Hook::PreToolUse(Box::new(hook)));
        self
    }

    pub async fn dispatch_session_start_hooks(&mut self) -> Result<()> {
        match invoke_hooks!(SessionStart, self)? {
            HookControl::Continue => Ok(()),
            HookControl::Block(reason) => {
                self.emit_update(AgentUpdate::Info(format!(
                    "[SessionStart hook blocked] {reason}"
                )));
                Ok(())
            }
        }
    }
    /// Returns hooks registered for the given [`HookTypes`] variant.
    pub fn hooks_by_type(&self, hook_type: HookTypes) -> Vec<&Hook> {
        self.hooks
            .iter()
            .filter(|hook| hook_type == (*hook).into())
            .collect()
    }

    /// Returns all tool specs for the agent.
    pub fn all_tool_specs(&self) -> Vec<ToolSpec> {
        self.cached_tool_specs
            .iter()
            .map(crate::tool::copy_tool_spec)
            .collect()
    }

    // TODO(compact): summarization input is a crude tail-truncation to 80k
    // chars of raw JSON; consider a smarter selection (e.g. drop tool-result
    // bodies first, keep user/assistant text).
    pub async fn compact_history(&mut self, focus: Option<&str>) -> Result<()> {
        if self.is_openai_responses() && self.provider_kind != ProviderKind::DeepSeek {
            self.compact_responses_native().await
        } else {
            self.compact_history_local(focus).await
        }
    }

    /// Native OpenAI Responses compaction via `POST /responses/compact`.
    ///
    /// Sends the current protocol baseline plus any logical messages not yet
    /// represented in it. A valid compact resource replaces the wire
    /// baseline; the logical context stays unchanged (the state baseline now
    /// covers it). Messages and provider state are persisted atomically, and
    /// only then are the runtime fields/counters committed. `focus` is
    /// ignored: the native endpoint has no summary-focus semantics. Errors
    /// leave the old committed state intact; transient transport errors are
    /// retried with bounded backoff, protocol errors are not.
    async fn compact_responses_native(&mut self) -> Result<()> {
        let model_name = self.agent_settings.model.clone();
        let request = CreateMessageParams::new(RequiredMessageParams {
            model: model_name,
            messages: self.runtime.context.clone(),
            max_tokens: self.max_tokens(),
        })
        .with_reasoning_effort(self.agent_settings.reasoning_effort);
        self.emit_update(AgentUpdate::Info("[native compact]".into()));

        let mut retry_attempt = 0;
        let response = loop {
            match self
                .runtime
                .client
                .compact(&request, self.runtime.provider_state.as_ref())
                .await
            {
                Ok(response) => break response,
                Err(error) => {
                    let error_text = error.to_string();
                    if retry_attempt >= MAX_COMPACT_SUMMARY_RETRY_ATTEMPTS
                        || !is_transient_transport_error(&error_text.to_lowercase())
                    {
                        return Err(anyhow::Error::from(error));
                    }
                    retry_attempt = retry_attempt.saturating_add(1);
                    let delay = backoff_delay(retry_attempt.saturating_sub(1));
                    let summary = error_summary(&error_text);
                    self.emit_update(AgentUpdate::Info(format!(
                        "[compact retry {retry_attempt}/{MAX_COMPACT_SUMMARY_RETRY_ATTEMPTS}] retrying in {:.1}s — {summary}",
                        delay.as_secs_f64()
                    )));
                    tokio::time::sleep(delay).await;
                }
            }
        };

        self.persist_llm_call(
            "responses_compact",
            response.usage.as_ref(),
            response.request_body.as_deref(),
        )
        .await
        .context("failed to persist Responses compact usage")?;

        let ProviderStateUpdate::Replace(ProviderConversationState::OpenAiResponses(
            candidate_state,
        )) = response.state_update
        else {
            anyhow::bail!("native Responses compaction returned no replacement provider state");
        };

        // The replacement baseline covers the entire logical context, so the
        // candidate logical context is the current one.
        let candidate_context = self.runtime.context.clone();
        self.replace_persisted_context_and_state(Some(
            &ProviderConversationState::OpenAiResponses(candidate_state.clone()),
        ))
        .await?;
        // Context and persistence now agree; commit runtime fields/counters.
        self.runtime.context = candidate_context;
        self.runtime.provider_state = Some(ProviderConversationState::OpenAiResponses(
            candidate_state.clone(),
        ));
        self.runtime.first_message_db_id = 0;
        self.runtime.last_message_db_id = 0;
        self.runtime.llm_call_last_message_id = 0;
        self.runtime.last_token_total = 0;
        self.runtime.compact_state.has_compacted = true;
        self.runtime.stats.compactions += 1;

        // Informational status only: item count and a bounded compaction id
        // prefix. Never expose the opaque encrypted content, and never echo
        // the full compaction id into user-facing diagnostics; the complete
        // id is retained inside the provider state and SQLite metadata.
        let item_count = candidate_state.input_items.len();
        let compaction_id = candidate_state.compaction_id.unwrap_or_default();
        self.emit_update(AgentUpdate::Info(format!(
            "[responses compacted: items={item_count}, id={}]",
            compact_id_display(&compaction_id)
        )));
        Ok(())
    }

    /// Previous single-summary compaction (entire history → one user message).
    /// Kept for reference / rollback; production call sites use [`Self::compact_history`].
    #[allow(dead_code)]
    pub async fn compact_history_legacy(&mut self, focus: Option<&str>) -> Result<()> {
        self.compact_history_local_with_mode(focus, CompactRebuildMode::LegacySingleSummary)
            .await
    }

    async fn compact_history_local(&mut self, focus: Option<&str>) -> Result<()> {
        self.compact_history_local_with_mode(focus, CompactRebuildMode::CodexStyle)
            .await
    }

    async fn compact_history_local_with_mode(
        &mut self,
        focus: Option<&str>,
        mode: CompactRebuildMode,
    ) -> Result<()> {
        let tact_path = crate::consts::TactPath::new(&self.tool_context.work_dir);
        let transcript_path = write_transcript(&tact_path, &self.runtime.context).await?;
        self.emit_update(AgentUpdate::Info(format!(
            "[transcript saved: {}]",
            transcript_path.display()
        )));

        let model_context_window = self.model_context_window();
        let summary_max_tokens = if model_context_window == 0 {
            COMPACT_SUMMARY_MAX_TOKENS
        } else {
            u32::try_from(
                model_context_window
                    .saturating_mul(COMPACT_SUMMARY_OUTPUT_PERCENT)
                    .div_ceil(100)
                    .min(COMPACT_SUMMARY_MAX_TOKENS as usize)
                    .max(1),
            )
            .context("summary output token budget does not fit u32")?
        };
        let summary_input_limit = if model_context_window == 0 {
            crate::compact::KEEP_USER_MESSAGE_TOKENS
        } else {
            let headroom = model_context_window
                .saturating_mul(COMPACT_SUMMARY_HEADROOM_PERCENT)
                .div_ceil(100);
            model_context_window
                .saturating_sub(summary_max_tokens as usize)
                .saturating_sub(headroom)
        };
        let mut prompt = COMPACT_SUMMARY_INSTRUCTIONS.to_string();
        if approx_text_tokens(&prompt) > summary_input_limit {
            anyhow::bail!(
                "model context window {model_context_window} is too small for the compaction summary request"
            );
        }
        if let Some(focus) = focus.filter(|value| !value.trim().is_empty()) {
            let addition = format!("\n\nFocus to preserve next: {focus}");
            let available = summary_input_limit.saturating_sub(approx_text_tokens(&prompt));
            if approx_text_tokens(&addition) <= available {
                prompt.push_str(&addition);
            } else {
                tracing::warn!(
                    focus_chars = focus.len(),
                    available_tokens = available,
                    needed_tokens = approx_text_tokens(&addition),
                    "focus too large for compact summary prompt, dropping"
                );
            }
        }
        let mut heading_added = false;
        for path in &self.runtime.compact_state.recent_files {
            let addition = if heading_added {
                format!("\n- {path}")
            } else {
                format!("\n\nRecent files to reopen if needed:\n- {path}")
            };
            if approx_text_tokens(&prompt).saturating_add(approx_text_tokens(&addition))
                > summary_input_limit
            {
                tracing::warn!(
                    prompt_tokens = approx_text_tokens(&prompt),
                    needed_tokens = approx_text_tokens(&addition),
                    "compact summary prompt too large, dropping recent files"
                );
                break;
            }
            prompt.push_str(&addition);
            heading_added = true;
        }
        let history_budget = summary_input_limit
            .saturating_sub(approx_text_tokens(&prompt))
            .saturating_sub(1)
            .min(crate::compact::KEEP_USER_MESSAGE_TOKENS);
        let recent_messages = recent_messages_for_summary(&self.runtime.context, history_budget)?;
        if recent_messages != "[]" {
            prompt.push_str("\n\n");
            prompt.push_str(&recent_messages);
        }
        debug_assert!(
            model_context_window == 0 || approx_text_tokens(&prompt) <= summary_input_limit
        );

        let model_name = self.agent_settings.model.clone();
        let initial_request = CreateMessageParams::new(RequiredMessageParams {
            model: model_name.clone(),
            messages: vec![Message::new_text(Role::User, prompt.clone())],
            max_tokens: summary_max_tokens,
        })
        .with_reasoning_effort(self.agent_settings.reasoning_effort);

        self.emit_update(AgentUpdate::ModelInfo(tact_protocol::ModelCallParams {
            model: model_name.clone(),
            max_tokens: initial_request.max_tokens,
            thinking_budget: initial_request
                .thinking
                .as_ref()
                .map(|t| t.budget_tokens as u32),
            reasoning_effort: self
                .agent_settings
                .reasoning_effort
                .map(|effort| effort.as_str().to_string()),
            extra_body: initial_request
                .thinking
                .as_ref()
                .map(|t| serde_json::json!({"thinking": t}).to_string()),
        }));
        // ── Stats: before compaction LLM call ──
        self.runtime.stats.prompt_count += 1;
        let compact_prompt_chars = serde_json::to_string(&initial_request)
            .map(|s| s.chars().count() as u64)
            .unwrap_or(0);
        self.runtime.stats.total_prompt_chars += compact_prompt_chars;
        let compact_start = std::time::Instant::now();

        // Summarization call with two independent recovery axes:
        // - transient transport errors → bounded backoff retry (`retry_attempt`);
        // - `MaxTokens` truncation → append the partial summary as an assistant
        //   message plus a continuation prompt and re-call, mirroring the main
        //   agent loop's output-limit recovery (`continuation_attempt`).
        // When continuation attempts are exhausted, the partial summary is
        // accepted as best-effort (the Codex-style rebuild keeps recent real
        // user messages anyway).
        let mut retry_attempt = 0;
        let mut continuation_attempt = 0u32;
        let mut messages = vec![Message::new_text(Role::User, prompt.clone())];
        let mut blocks_all: Vec<ContentBlock> = Vec::new();
        let (stop_reason, token_usage, request_body) = loop {
            let request = CreateMessageParams::new(RequiredMessageParams {
                model: model_name.clone(),
                messages: messages.clone(),
                max_tokens: summary_max_tokens,
            })
            .with_reasoning_effort(self.agent_settings.reasoning_effort);
            match self.runtime.client.create_message(&request, None).await {
                Ok(response) => {
                    let truncated = matches!(response.stop_reason, Some(StopReason::MaxTokens));
                    if truncated && continuation_attempt < MAX_CONTINUATION_ATTEMPTS {
                        continuation_attempt = continuation_attempt.saturating_add(1);
                        blocks_all.extend(response.blocks.clone());
                        messages.push(Message::new_blocks(Role::Assistant, response.blocks));
                        messages.push(Message::new_text(
                            Role::User,
                            continuation_message(continuation_attempt).to_string(),
                        ));
                        self.emit_update(AgentUpdate::Info(format!(
                            "[compact continue {continuation_attempt}/{MAX_CONTINUATION_ATTEMPTS}] summary truncated, continuing"
                        )));
                        continue;
                    }
                    blocks_all.extend(response.blocks);
                    break (response.stop_reason, response.usage, response.request_body);
                }
                Err(error) => {
                    let error_text = error.to_string();
                    if retry_attempt >= MAX_COMPACT_SUMMARY_RETRY_ATTEMPTS
                        || !is_transient_transport_error(&error_text.to_lowercase())
                    {
                        return Err(anyhow::Error::from(error));
                    }
                    retry_attempt = retry_attempt.saturating_add(1);
                    let delay = backoff_delay(retry_attempt.saturating_sub(1));
                    let summary = error_summary(&error_text);
                    self.emit_update(AgentUpdate::Info(format!(
                        "[compact retry {retry_attempt}/{MAX_COMPACT_SUMMARY_RETRY_ATTEMPTS}] retrying in {:.1}s — {summary}",
                        delay.as_secs_f64()
                    )));
                    tokio::time::sleep(delay).await;
                }
            }
        };
        let blocks = blocks_all;

        // ── Stats: after compaction LLM call ──
        self.runtime
            .stats
            .llm_call_durations
            .push(compact_start.elapsed());
        let compact_response_chars = serde_json::to_string(&blocks)
            .map(|s| s.chars().count() as u64)
            .unwrap_or(0);
        self.runtime.stats.total_response_chars += compact_response_chars;
        for block in &blocks {
            if let ContentBlock::Thinking { thinking, .. } = block {
                self.runtime.stats.thinking_blocks += 1;
                self.runtime.stats.total_thinking_chars += thinking.chars().count() as u64;
            }
        }
        if let Some(ref usage) = token_usage {
            self.runtime.stats.record_token_usage(usage);
            // Do NOT assign usage.total to last_token_total: that figure is for
            // the summarization request (large history prompt), not the size of
            // the replacement context below.
        }
        let _ = self
            .persist_llm_call("compact", token_usage.as_ref(), request_body.as_deref())
            .await;
        match stop_reason {
            // `MaxTokens` is accepted when continuation attempts are exhausted:
            // the partial summary is still usable (Codex-style rebuild keeps
            // recent real user messages; a one-shot summary that ran out of
            // output budget is a best-effort loss, not a fatal error).
            None | Some(StopReason::EndTurn) | Some(StopReason::MaxTokens) => {}
            Some(reason) => {
                anyhow::bail!("compaction summary ended with invalid stop reason: {reason:?}")
            }
        }
        let summary = blocks
            .iter()
            .filter_map(|block| match block {
                ContentBlock::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");
        let summary = summary.trim().to_string();
        if summary.is_empty() {
            anyhow::bail!("compaction summary response contained no text")
        }

        // Inject recently accessed file list into summary, helping the agent recover context after amnesia
        let mut full_summary = summary.clone();
        if !self.runtime.compact_state.recent_files.is_empty() {
            full_summary
                .push_str("\n\nRecently accessed files (re-read if you need their contents):\n");
            for path in &self.runtime.compact_state.recent_files {
                full_summary.push_str(&format!("- {path}\n"));
            }
        }

        let rebuilt_context = match mode {
            CompactRebuildMode::CodexStyle => {
                // Rebuild: keep recent real user messages verbatim from the
                // tail, then append a single summary message. Budget is
                // model_context_window minus system prompt, tool specs,
                // summary, max output, and headroom. Shrink the budget in a
                // loop until the rebuilt context fits, or bail if it can't.
                let retained = collect_user_messages(&self.runtime.context);
                let system_prompt_tokens = approx_text_tokens(&self.build_system_prompt()?);
                let tool_specs_tokens = approx_text_tokens(
                    &serde_json::to_string(&self.all_tool_specs())
                        .context("failed to serialize tool specs for compact budget")?,
                );
                let summary_only = compacted_context(full_summary.clone());
                let non_retained_input_tokens = system_prompt_tokens
                    .saturating_add(tool_specs_tokens)
                    .saturating_add(estimate_context_tokens(&summary_only));
                let mut retained_tokens = retained_user_message_token_budget(
                    self.model_context_window(),
                    self.max_tokens() as usize,
                    non_retained_input_tokens,
                );
                let mut rebuilt =
                    build_compacted_history(&retained, full_summary.clone(), retained_tokens);
                if model_context_window > 0 {
                    let headroom = compact_rebuild_headroom_tokens(model_context_window);
                    loop {
                        let total = system_prompt_tokens
                            .saturating_add(tool_specs_tokens)
                            .saturating_add(estimate_context_tokens(&rebuilt))
                            .saturating_add(self.max_tokens() as usize)
                            .saturating_add(headroom);
                        if total <= model_context_window {
                            break;
                        }
                        if retained_tokens == 0 {
                            anyhow::bail!(
                                "compacted request cannot fit model context window {model_context_window}"
                            );
                        }
                        tracing::warn!(
                            "rebuild over window, shrinking retained token budget: total={total} window={model_context_window} retained={retained_tokens}"
                        );
                        retained_tokens = retained_tokens
                            .saturating_sub(total.saturating_sub(model_context_window).max(1));
                        rebuilt = build_compacted_history(
                            &retained,
                            full_summary.clone(),
                            retained_tokens,
                        );
                    }
                }
                rebuilt
            }
            CompactRebuildMode::LegacySingleSummary => compacted_context(full_summary),
        };
        let previous_context = std::mem::replace(&mut self.runtime.context, rebuilt_context);
        if self.is_openai_responses() {
            // The rebuilt logical history invalidates the old Responses
            // baseline (e.g. DeepSeek + Responses, whose endpoint lacks
            // `/responses/compact` and compacts locally). Clear it in both the
            // runtime and persistence so the next request rebuilds from the
            // compacted context instead of failing the stale-hash check.
            self.runtime.provider_state = None;
            if let Err(error) = self.replace_persisted_context_and_state(None).await {
                self.runtime.context = previous_context;
                return Err(error);
            }
        } else if let Err(error) = self.replace_persisted_context().await {
            self.runtime.context = previous_context;
            return Err(error);
        }
        // Context and persistence now agree, so future messages start a new
        // message-id window and compaction state can be committed.
        self.runtime.first_message_db_id = 0;
        self.runtime.last_message_db_id = 0;
        self.runtime.llm_call_last_message_id = 0;
        self.runtime.compact_state.has_compacted = true;
        self.runtime.compact_state.last_summary = Some(summary);
        // Reset so the next should_auto_compact check reflects the new small
        // context (via token estimate / next main-loop TokenUsage), not the
        // pre-compact or summarizer-prompt totals.
        self.runtime.last_token_total = 0;
        self.runtime.stats.compactions += 1;
        Ok(())
    }

    fn remember_recent_file(&mut self, path: &str) {
        self.runtime
            .compact_state
            .recent_files
            .retain(|existing| existing != path);
        self.runtime
            .compact_state
            .recent_files
            .push(path.to_string());
        if self.runtime.compact_state.recent_files.len() > 5 {
            let overflow = self.runtime.compact_state.recent_files.len() - 5;
            self.runtime.compact_state.recent_files.drain(0..overflow);
        }
    }

    fn build_system_prompt(&mut self) -> Result<String> {
        if let AgentSystemPrompt::Static(system_prompt) = &self.system_prompt {
            return Ok(system_prompt.clone());
        }

        let workdir = &self.tool_context.work_dir;
        let mut prompt_builder = SystemPrompt::builder();
        if matches!(&self.runtime.client, LlmProvider::OpenAiResponses(_)) {
            prompt_builder.template(responses_prompt_template());
        }
        let prompt = prompt_builder
            .role(format!(
                "You are a coding agent operating in {}.",
                workdir.display()
            ))
            .guidelines([
                "Try to understand how to complete the task well before completing it.",
            ])
            .constraints([
                "Think step by step",
                "Think before you act; respond with your thoughts before calling tools",
                "Do not make up any assumptions, use tools to get the information you need",
                "Use the provided tools to interact with the system and accomplish the task",
                "If you are stuck, or otherwise cannot complete the task, respond with your thoughts and stop",
                "If the task is completed, or otherwise cannot continue, like requiring user feedback, stop.",
                "Always end your response with a visible text conclusion; never exit after thinking alone without a text block — even a single-sentence summary of your reasoning result is enough.",
                "When editing files, always re-read the file first if its content may have changed since you last read it",
                "If a tool result was truncated and you need the details, re-run the relevant tool (e.g., read_file)",
                "For small edits to existing files, prefer edit_file over write_file; use write_file only for new files or complete rewrites",
            ])
            .skills_available({
                let reg = crate::skill::lock_skills(&self.tool_context.skill_registry);
                if self.agent_settings.skill_body_auto_inject {
                    reg.describe_available_with_body()
                } else {
                    reg.describe_available()
                }
            })
            .memory(self.load_memory_prompt()?)
            .claude_md(cached_md_section(&mut self.runtime.cached_claude_md, || {
                assemble_claude_md_prompt(workdir, &self.agent_settings.instruction_sources)
            }))
            .additional(cached_md_section(&mut self.runtime.cached_agents_md, || {
                assemble_agents_md_prompt(workdir, &self.agent_settings.instruction_sources)
            }))
            .dynamic_context(load_dynamic_context(
                workdir,
                &mut self.runtime.cached_dir_snapshot,
                self.agent_settings.snapshot_max_items,
                &self.agent_settings.model,
            ))
            .memory_guidance(MEMORY_GUIDANCE.trim())
            .build()?;

        prompt
            .to_prompt()
            .render()
            .context("failed to render system prompt")
    }

    fn load_memory_prompt(&self) -> Result<String> {
        self.tool_context
            .memory_manager
            .lock()
            .map_err(|_| anyhow::anyhow!("memory manager lock poisoned"))
            .map(|manager| manager.load_memory_prompt())
    }
}

/// Build the dynamic-context block that appears after `=== DYNAMIC_BOUNDARY ===`.
///
/// The directory snapshot is expensive to compute and its output must be
/// byte-for-byte identical across requests so that DeepSeek's prefix KV-cache
/// can hit.  We compute it once per session and reuse the cached string.
fn load_dynamic_context(
    workdir: &Path,
    cached_snapshot: &mut Option<String>,
    snapshot_limit: usize,
    model: &str,
) -> String {
    let tree = match cached_snapshot {
        Some(cached) => cached.clone(),
        None => {
            let snap = snapshot_dir(workdir, snapshot_limit);
            *cached_snapshot = snap.clone();
            snap.unwrap_or_default()
        }
    };

    let mut lines = vec![
        format!("Current date: {}", Utc::now().date_naive()),
        format!("Working directory: {}", workdir.display()),
        format!("Model: {model}"),
        format!("Platform: {}", std::env::consts::OS),
    ];

    if !tree.is_empty() {
        lines.push(String::new());
        lines.push(tree);
    }

    lines.join("\n")
}

/// Bounded display for a compaction id in user-facing diagnostics: only a
/// short prefix is shown. The full id is retained inside the provider state
/// and SQLite metadata; it is never echoed into Info lines.
fn compact_id_display(id: &str) -> String {
    const MAX_ID_CHARS: usize = 12;
    id.chars().take(MAX_ID_CHARS).collect()
}

/// Directory-only workspace snapshot for the system prompt.
fn snapshot_dir(root: &Path, max_items: usize) -> Option<String> {
    const IGNORE_DIRS: &[&str] = &[
        ".git",
        ".hg",
        ".svn",
        "target",
        "build",
        "node_modules",
        "vendor",
        "dist",
        ".next",
        ".nuxt",
        ".turbo",
        ".cache",
        "coverage",
        ".venv",
        "venv",
        "__pycache__",
        ".gradle",
        "bin",
        "obj",
        "_build",
        "deps",
        ".idea",
        ".DS_Store",
    ];

    use std::{cmp::Ordering, collections::BTreeMap};

    // filter_entry prunes ignored dirs during the walk, not after.
    let mut items: Vec<std::path::PathBuf> = Vec::new();

    let should_keep = |entry: &walkdir::DirEntry| {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            return true;
        };
        (name.eq_ignore_ascii_case(".env.example")
            || name.eq_ignore_ascii_case(".gitignore")
            || !name.starts_with('.'))
            && !IGNORE_DIRS.contains(&name)
    };

    for entry in walkdir::WalkDir::new(root)
        .max_depth(4)
        .follow_links(false)
        .into_iter()
        .filter_entry(should_keep)
        .filter_map(|e| e.ok())
    {
        let rel = entry.path().strip_prefix(root).ok()?;
        if rel.as_os_str().is_empty() {
            continue;
        }
        if !entry.file_type().is_dir() {
            continue;
        }
        items.push(rel.to_path_buf());
    }

    if items.is_empty() {
        return None;
    }

    items.sort_by(|a, b| {
        let depth = |path: &Path| path.components().count();
        match depth(a).cmp(&depth(b)) {
            Ordering::Equal => a.cmp(b),
            other => other,
        }
    });
    let truncated = if items.len() > max_items {
        items.truncate(max_items);
        true
    } else {
        false
    };

    let mut dirs: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for rel in &items {
        let parent = rel
            .parent()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| ".".to_string());
        let name = rel.file_name().and_then(|n| n.to_str()).unwrap_or("?");
        dirs.entry(parent).or_default().push(format!("{name}/"));
    }

    let mut out = vec!["## Project structure".to_string(), String::new()];
    for (dir, mut children) in dirs {
        out.push(dir);
        children.sort();
        for child in children {
            out.push(format!("  {child}"));
        }
    }

    if truncated {
        out.push(format!("(truncated at {} items)", max_items));
    }

    Some(out.join("\n"))
}

/// Return a session-cached markdown section, computing it once on first use.
///
/// Empty string is still cached so missing files do not re-stat every turn.
fn cached_md_section(cached: &mut Option<String>, compute: impl FnOnce() -> String) -> String {
    if let Some(hit) = cached.as_ref() {
        return hit.clone();
    }
    let value = compute();
    *cached = Some(value.clone());
    value
}

fn assemble_claude_md_prompt(
    workdir: &Path,
    sources: &crate::config::InstructionSources,
) -> String {
    if !sources.claude_user && !sources.claude_project && !sources.claude_subdir {
        return String::new();
    }

    let mut file_sources = Vec::new();

    if sources.claude_user {
        let user_claude =
            crate::consts::TactPath::home_claude_dir().map(|home| home.join("CLAUDE.md"));
        if let Some(path) = user_claude
            && let Ok(content) = std::fs::read_to_string(&path)
        {
            file_sources.push((
                "user global (~/.claude/CLAUDE.md)".to_string(),
                content.trim().to_string(),
            ));
        }
    }

    if sources.claude_project {
        let project_claude = workdir.join("CLAUDE.md");
        if let Ok(content) = std::fs::read_to_string(&project_claude) {
            file_sources.push((
                "project root (CLAUDE.md)".to_string(),
                content.trim().to_string(),
            ));
        }
    }

    if sources.claude_subdir
        && let Ok(cwd) = std::env::current_dir()
        && cwd != workdir
    {
        let subdir_claude = cwd.join("CLAUDE.md");
        if let Ok(content) = std::fs::read_to_string(&subdir_claude) {
            file_sources.push((
                format!("subdir ({}/CLAUDE.md)", cwd.display()),
                content.trim().to_string(),
            ));
        }
    }

    if file_sources.is_empty() {
        return String::new();
    }

    let mut lines = vec!["## CLAUDE.md instructions".to_string(), String::new()];
    for (label, content) in file_sources {
        lines.push(format!("### From {}", label));
        lines.push(String::new());
        lines.push(content);
        lines.push(String::new());
    }
    lines.join("\n").trim().to_string()
}

/// Assemble project `AGENTS.md` for the system-prompt `additional` section.
///
/// Looks at the agent workdir and, when different, the process cwd — matching
/// the local CLAUDE.md discovery paths (without a user-global file).
fn assemble_agents_md_prompt(
    workdir: &Path,
    sources: &crate::config::InstructionSources,
) -> String {
    if !sources.agents_md {
        return String::new();
    }

    let mut file_sources = Vec::new();

    let project_agents = workdir.join("AGENTS.md");
    if let Ok(content) = std::fs::read_to_string(&project_agents) {
        file_sources.push((
            "project root (AGENTS.md)".to_string(),
            content.trim().to_string(),
        ));
    }

    if let Ok(cwd) = std::env::current_dir()
        && cwd != workdir
    {
        let subdir_agents = cwd.join("AGENTS.md");
        if let Ok(content) = std::fs::read_to_string(&subdir_agents) {
            file_sources.push((
                format!("subdir ({}/AGENTS.md)", cwd.display()),
                content.trim().to_string(),
            ));
        }
    }

    if file_sources.is_empty() {
        return String::new();
    }

    let mut lines = vec!["## AGENTS.md instructions".to_string(), String::new()];
    for (label, content) in file_sources {
        lines.push(format!("### From {}", label));
        lines.push(String::new());
        lines.push(content);
        lines.push(String::new());
    }
    lines.join("\n").trim().to_string()
}

#[cfg(test)]
mod tests {
    use std::sync::Once;

    use sqlx::Row;
    use tact_llm::{
        ContentBlock, LlmProvider, Message, MockClient, ProviderConversationState, ProviderKind,
        Role, StopReason,
    };
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;
    use crate::store::SessionStore;
    use crate::tool::test_support::test_context;

    static INIT_CONFIG: Once = Once::new();

    fn ensure_config() {
        INIT_CONFIG.call_once(|| {
            let config = crate::config::ResolvedConfig {
                llm: crate::config::LlmSettings {
                    provider: ProviderKind::OpenAi,
                    protocol: tact_llm::OpenAiProtocol::default(),
                    reasoning_effort: None,
                    api_key: String::new(),
                    base_url: String::new(),
                    model: "mock-model".to_string(),
                    models: Vec::new(),
                    model_profiles: Default::default(),
                    responses_compact_threshold: None,
                },
                agent: crate::config::AgentSettings {
                    model: "mock-model".to_string(),
                    reasoning_effort: None,
                    model_context_window: 500_000,
                    max_tokens: 8192,
                    thinking_budget: 0,
                    snapshot_max_items: 80,
                    notifications_enabled: false,
                    micro_compact_enabled: true,
                    skill_body_auto_inject: false,
                    skill_dirs: Vec::new(),
                    instruction_sources: crate::config::InstructionSources::default(),
                    subagent: None,
                },
                ui: crate::config::UiSettings {
                    theme: "retro".to_string(),
                    vision_image: crate::config::VisionImageSettings {
                        compress: crate::config::VisionImageSettings::DEFAULT_COMPRESS,
                        max_edge: crate::config::VisionImageSettings::DEFAULT_MAX_EDGE,
                        jpeg_quality: crate::config::VisionImageSettings::DEFAULT_JPEG_QUALITY,
                    },
                },
                tools: crate::config::ToolSettings {
                    bash_timeout_secs: crate::config::ToolSettings::DEFAULT_BASH_TIMEOUT_SECS,
                    bash_nice: crate::config::ToolSettings::DEFAULT_BASH_NICE,
                    rtk_filter: false,
                },
                voice: crate::config::VoiceSettings::disabled_defaults(),
                permission_mode: None,
                tokio_console: false,
                config_path: None,
            };
            crate::config::install(config);
        });
    }

    fn make_text_block(content: &str) -> ContentBlock {
        ContentBlock::Text {
            text: content.to_string(),
        }
    }

    #[test]
    fn agent_settings_snapshot_survives_global_config_override() {
        ensure_config();
        let context = test_context("agent_settings_snapshot");
        let router = crate::tool::toolset();
        let mcp = crate::mcp::MCPToolRouter::new();
        let perm = crate::permission::PermissionManager::try_new(
            crate::permission::PermissionMode::Default,
        )
        .unwrap();

        let tiny = crate::config::AgentSettings {
            model: "mock-model".to_string(),
            reasoning_effort: None,
            model_context_window: 500,
            max_tokens: 1024,
            thinking_budget: 0,
            snapshot_max_items: 10,
            notifications_enabled: false,
            micro_compact_enabled: true,
            skill_body_auto_inject: false,
            skill_dirs: Vec::new(),
            instruction_sources: crate::config::InstructionSources::default(),
            subagent: None,
        };
        let agent = Agent::new(
            LlmProvider::Mock(MockClient::new(vec![])),
            context,
            router,
            mcp,
            perm,
            AgentSystemPrompt::Static("You are a test agent.".to_string()),
        )
        .with_agent_settings(tiny.clone());

        #[cfg(feature = "test-support")]
        {
            let mut big = crate::config::settings();
            big.agent.model_context_window = 900_000;
            crate::config::install_or_override(big);
        }

        assert_eq!(agent.model_context_window(), 500);
        assert_eq!(agent.max_tokens(), 1024);
        assert_eq!(
            agent.agent_settings.model_context_window,
            tiny.model_context_window
        );
    }

    #[test]
    fn agent_new_initializes_with_correct_tool_specs() {
        let context = test_context("agent_new_test");
        let router = crate::tool::toolset();
        let mcp = crate::mcp::MCPToolRouter::new();
        let perm = crate::permission::PermissionManager::try_new(
            crate::permission::PermissionMode::Default,
        )
        .unwrap();

        let agent = Agent::new(
            LlmProvider::Mock(MockClient::new(vec![])),
            context,
            router,
            mcp,
            perm,
            AgentSystemPrompt::Static("You are a test agent.".to_string()),
        );

        let specs = agent.all_tool_specs();
        assert!(!specs.is_empty(), "tool specs should not be empty");
        let names: Vec<&str> = specs.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"bash"));
        assert!(names.contains(&"read_file"));
        assert!(names.contains(&"write_file"));
    }

    fn chat_completions_test_agent(context_name: &str) -> Agent {
        Agent::new(
            LlmProvider::Mock(MockClient::new(vec![])),
            test_context(context_name),
            crate::tool::toolset(),
            crate::mcp::MCPToolRouter::new(),
            crate::permission::PermissionManager::try_new(
                crate::permission::PermissionMode::Default,
            )
            .unwrap(),
            AgentSystemPrompt::Static("test".to_string()),
        )
    }

    fn responses_test_agent(context_name: &str, base_url: &str) -> Agent {
        Agent::new(
            LlmProvider::OpenAiResponses(tact_llm::openai::responses::OpenAiResponsesAdapter::new(
                "test-key", base_url, None,
            )),
            test_context(context_name),
            crate::tool::toolset(),
            crate::mcp::MCPToolRouter::new(),
            crate::permission::PermissionManager::try_new(
                crate::permission::PermissionMode::Default,
            )
            .unwrap(),
            AgentSystemPrompt::Static("test".to_string()),
        )
    }

    #[test]
    fn responses_tool_specs_exclude_compact() {
        let agent = responses_test_agent("responses_tool_specs", "https://api.openai.com/v1");
        assert!(
            !agent
                .all_tool_specs()
                .iter()
                .any(|spec| spec.name == "compact"),
            "Responses model-facing tool specs must not include the local compact tool"
        );
    }

    #[test]
    fn non_responses_tool_specs_keep_compact() {
        let agent = chat_completions_test_agent("non_responses_tool_specs_keep_compact");
        assert!(
            agent
                .all_tool_specs()
                .iter()
                .any(|spec| spec.name == "compact"),
            "non-Responses providers keep the compact tool"
        );
    }

    #[tokio::test]
    async fn responses_compact_history_dispatches_to_native_endpoint() {
        ensure_config();
        let server = MockServer::start().await;
        let fixture = serde_json::json!({
            "id": "cmp_sanitized_01",
            "object": "response.compaction",
            "created_at": 1754000000,
            "output": [
                {
                    "type": "function_call_output",
                    "call_id": "call_sanitized_1",
                    "output": "sanitized tool output retained by compaction",
                    "id": "fc_out_sanitized_1",
                    "status": "completed"
                },
                {
                    "type": "compaction",
                    "id": "cmp_sanitized_01",
                    "encrypted_content": "sanitized-encrypted-compaction-content-placeholder"
                }
            ],
            "usage": {
                "input_tokens": 1200,
                "input_tokens_details": { "cached_tokens": 0 },
                "output_tokens": 340,
                "output_tokens_details": { "reasoning_tokens": 0 },
                "total_tokens": 1540
            }
        });
        Mock::given(method("POST"))
            .and(path("/responses/compact"))
            .respond_with(ResponseTemplate::new(200).set_body_json(fixture))
            .expect(1)
            .mount(&server)
            .await;
        // No local summary `create_message()` may be attempted: an ordinary
        // `/responses` request must never arrive.
        Mock::given(method("POST"))
            .and(path("/responses"))
            .respond_with(ResponseTemplate::new(500))
            .expect(0)
            .mount(&server)
            .await;

        let mut agent = responses_test_agent("responses_compact_native", &server.uri());
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        agent = agent.with_ui_channel(tx);
        agent
            .runtime
            .context
            .push(Message::new_text(Role::User, "first turn"));
        agent
            .runtime
            .context
            .push(Message::new_text(Role::Assistant, "second turn"));

        agent
            .compact_history(None)
            .await
            .expect("native compaction should succeed");

        // Exactly one native compact request; no local summary `create_message`
        // request may hit a `/responses` endpoint.
        server.verify().await;

        // The runtime provider state carries the fixture compaction id and the
        // replacement baseline.
        let Some(ProviderConversationState::OpenAiResponses(state)) = &agent.runtime.provider_state
        else {
            panic!("runtime provider state must be set after native compaction");
        };
        assert_eq!(state.compaction_id.as_deref(), Some("cmp_sanitized_01"));
        assert!(state.is_compacted);
        assert_eq!(state.logical_message_count, 2);
        assert_eq!(
            state.input_items.len(),
            2,
            "retained function_call_output + compaction item"
        );
        // The opaque encrypted content is preserved in the protocol baseline
        // (it must be replayed to the endpoint) but never surfaces in TUI
        // diagnostics.
        let encrypted = state
            .input_items
            .iter()
            .find_map(|item| item.get("encrypted_content").and_then(|v| v.as_str()))
            .expect("compaction item must carry encrypted_content");
        assert!(!encrypted.is_empty());
        let mut updates = Vec::new();
        while let Ok(update) = rx.try_recv() {
            updates.push(update);
        }
        let info_messages: Vec<&str> = updates
            .iter()
            .filter_map(|u| match u {
                tact_protocol::AgentUpdate::Info(msg) => Some(msg.as_str()),
                _ => None,
            })
            .collect();
        assert!(
            info_messages
                .iter()
                .any(|msg| msg.contains("[responses compacted: items=2, id=cmp_sanitize]")),
            "success Info must display only a bounded compaction id prefix, got: {updates:?}"
        );
        assert!(
            info_messages
                .iter()
                .all(|msg| !msg.contains("cmp_sanitized_01")),
            "full compaction id must never surface in Info updates, got: {updates:?}"
        );
        assert!(
            info_messages.iter().all(|msg| !msg.contains(encrypted)),
            "encrypted compaction content must never surface in Info updates, got: {updates:?}"
        );
        assert_eq!(
            agent.runtime.context.len(),
            2,
            "native compaction keeps the logical context unchanged"
        );
    }

    #[tokio::test]
    async fn deepseek_responses_compact_falls_back_to_local_summary() {
        ensure_config();
        // The DeepSeek endpoint lacks `/responses/compact` (verified live
        // 2026-08-02: it returns an empty body), so DeepSeek + Responses must
        // compact through the local summary path and clear the stale baseline.

        let server = MockServer::start().await;
        // The local summary request is an ordinary (non-streaming) `/responses`
        // call; DeepSeek serves it fine.
        let summary_fixture = serde_json::json!({
            "id": "resp_summary",
            "object": "response",
            "created_at": 1,
            "completed_at": 2,
            "status": "completed",
            "model": "deepseek-v4-flash",
            "output": [{
                "type": "message",
                "id": "msg_summary",
                "role": "assistant",
                "status": "completed",
                "content": [{
                    "type": "output_text",
                    "annotations": [],
                    "logprobs": null,
                    "text": "compaction summary text"
                }]
            }],
            "usage": {
                "input_tokens": 10,
                "input_tokens_details": { "cached_tokens": 0 },
                "output_tokens": 2,
                "output_tokens_details": { "reasoning_tokens": 0 },
                "total_tokens": 12
            }
        });
        Mock::given(method("POST"))
            .and(path("/responses"))
            .respond_with(ResponseTemplate::new(200).set_body_json(summary_fixture))
            .mount(&server)
            .await;
        // `/responses/compact` must never be called for DeepSeek.
        Mock::given(method("POST"))
            .and(path("/responses/compact"))
            .respond_with(ResponseTemplate::new(200))
            .expect(0)
            .mount(&server)
            .await;

        let mut agent = responses_test_agent("deepseek_responses_local_compact", &server.uri())
            .with_provider_kind(tact_llm::ProviderKind::DeepSeek);
        agent
            .runtime
            .context
            .push(Message::new_text(Role::User, "first turn"));
        agent
            .runtime
            .context
            .push(Message::new_text(Role::Assistant, "second turn"));

        agent
            .compact_history(None)
            .await
            .expect("local summary compaction should succeed for DeepSeek Responses");

        server.verify().await;

        // The old Responses baseline covers the pre-compact history and must
        // not survive the rebuild.
        assert!(
            agent.runtime.provider_state.is_none(),
            "stale Responses baseline must be cleared after local compaction"
        );
        let context_text = serde_json::to_string(&agent.runtime.context).unwrap_or_default();
        assert!(
            context_text.contains("compaction summary text"),
            "rebuilt context must contain the summary, got: {context_text}"
        );
    }

    #[tokio::test]
    async fn local_compact_continues_truncated_summary() {
        ensure_config();
        use tact_protocol::AgentUpdate;

        let context = test_context("local_compact_continue");
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();

        // First summary call hits the output limit (MaxTokens); the partial
        // summary must be continued on the next call (EndTurn).
        let mock = MockClient::new(vec![
            (
                vec![make_text_block("summary part one")],
                Some(StopReason::MaxTokens),
            ),
            (
                vec![make_text_block("summary part two")],
                Some(StopReason::EndTurn),
            ),
        ]);

        let mut agent = Agent::new(
            LlmProvider::Mock(mock),
            context,
            crate::tool::toolset(),
            crate::mcp::MCPToolRouter::new(),
            crate::permission::PermissionManager::try_new(
                crate::permission::PermissionMode::Default,
            )
            .unwrap(),
            AgentSystemPrompt::Static("You are a test agent.".to_string()),
        )
        .with_ui_channel(tx);
        agent
            .runtime
            .context
            .push(Message::new_text(Role::User, "first turn"));
        agent
            .runtime
            .context
            .push(Message::new_text(Role::Assistant, "second turn"));

        agent
            .compact_history(None)
            .await
            .expect("compact with truncated summary must continue and succeed");

        // Both the truncated part and the continuation end up in the rebuilt
        // context (the continuation request carries the partial assistant
        // message, so both blocks are merged into the final summary).
        let context_text = serde_json::to_string(&agent.runtime.context).unwrap_or_default();
        assert!(
            context_text.contains("summary part one"),
            "truncated summary part missing: {context_text}"
        );
        assert!(
            context_text.contains("summary part two"),
            "continuation summary part missing: {context_text}"
        );

        let mut updates = Vec::new();
        while let Ok(update) = rx.try_recv() {
            updates.push(update);
        }
        assert!(
            updates.iter().any(|u| {
                matches!(u, AgentUpdate::Info(msg) if msg.contains("[compact continue 1/3]"))
            }),
            "expected a compact-continue Info update, got: {updates:?}"
        );
    }

    #[tokio::test]
    async fn local_compact_continues_through_multiple_truncations() {
        ensure_config();
        let context = test_context("local_compact_continue_multi");
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let mut tool_context = context;
        tool_context.ui_tx = Some(tx);

        // Two consecutive truncations, then a completed call.
        let mock = MockClient::new(vec![
            (
                vec![make_text_block("part one")],
                Some(StopReason::MaxTokens),
            ),
            (
                vec![make_text_block("part two")],
                Some(StopReason::MaxTokens),
            ),
            (
                vec![make_text_block("part three")],
                Some(StopReason::EndTurn),
            ),
        ]);

        let mut agent = Agent::new(
            LlmProvider::Mock(mock),
            tool_context,
            crate::tool::toolset(),
            crate::mcp::MCPToolRouter::new(),
            crate::permission::PermissionManager::try_new(
                crate::permission::PermissionMode::Default,
            )
            .unwrap(),
            AgentSystemPrompt::Static("You are a test agent.".to_string()),
        );
        agent
            .runtime
            .context
            .push(Message::new_text(Role::User, "first turn"));

        agent
            .compact_history(None)
            .await
            .expect("compact must survive repeated truncations");

        let context_text = serde_json::to_string(&agent.runtime.context).unwrap_or_default();
        assert!(
            context_text.contains("part one") && context_text.contains("part two"),
            "all truncated parts must be retained: {context_text}"
        );
        assert!(
            context_text.contains("part three"),
            "final continuation part missing: {context_text}"
        );
    }

    #[tokio::test]
    async fn local_compact_accepts_partial_summary_when_continuations_exhausted() {
        ensure_config();
        let context = test_context("local_compact_continue_exhausted");
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let mut tool_context = context;
        tool_context.ui_tx = Some(tx);

        // Every call is truncated: MAX_CONTINUATION_ATTEMPTS (3) continuations
        // run, then the partial summary is accepted as best-effort instead of
        // failing the whole compaction.
        let mock = MockClient::new(vec![
            (
                vec![make_text_block("partial one")],
                Some(StopReason::MaxTokens),
            ),
            (
                vec![make_text_block("partial two")],
                Some(StopReason::MaxTokens),
            ),
            (
                vec![make_text_block("partial three")],
                Some(StopReason::MaxTokens),
            ),
            (
                vec![make_text_block("partial four")],
                Some(StopReason::MaxTokens),
            ),
        ]);

        let mut agent = Agent::new(
            LlmProvider::Mock(mock),
            tool_context,
            crate::tool::toolset(),
            crate::mcp::MCPToolRouter::new(),
            crate::permission::PermissionManager::try_new(
                crate::permission::PermissionMode::Default,
            )
            .unwrap(),
            AgentSystemPrompt::Static("You are a test agent.".to_string()),
        );
        agent
            .runtime
            .context
            .push(Message::new_text(Role::User, "first turn"));

        agent
            .compact_history(None)
            .await
            .expect("exhausted continuations must not fail compaction");

        let context_text = serde_json::to_string(&agent.runtime.context).unwrap_or_default();
        assert!(
            context_text.contains("partial one") && context_text.contains("partial two"),
            "partial summary must still be used: {context_text}"
        );
    }

    #[tokio::test]
    async fn responses_compact_surfaces_usage_persistence_failure() {
        ensure_config();
        let server = MockServer::start().await;
        let fixture = serde_json::json!({
            "id": "cmp_sanitized_02",
            "object": "response.compaction",
            "created_at": 1754000001,
            "output": [
                {
                    "type": "function_call_output",
                    "call_id": "call_sanitized_2",
                    "output": "sanitized tool output retained by compaction",
                    "id": "fc_out_sanitized_2",
                    "status": "completed"
                },
                {
                    "type": "compaction",
                    "id": "cmp_sanitized_02",
                    "encrypted_content": "sanitized-encrypted-compaction-content-placeholder"
                }
            ],
            "usage": {
                "input_tokens": 1200,
                "input_tokens_details": { "cached_tokens": 0 },
                "output_tokens": 340,
                "output_tokens_details": { "reasoning_tokens": 0 },
                "total_tokens": 1540
            }
        });
        Mock::given(method("POST"))
            .and(path("/responses/compact"))
            .respond_with(ResponseTemplate::new(200).set_body_json(fixture))
            .expect(1)
            .mount(&server)
            .await;

        let dir = tempfile::tempdir().unwrap();
        let sqlite =
            crate::store::session_store::SqliteSessionStore::new(&dir.path().join("session.db"))
                .await
                .unwrap();
        sqlite
            .create_session("session-1", dir.path().to_str().unwrap(), "")
            .await
            .unwrap();

        // Seed the committed baseline that a successful compaction would
        // replace: two messages plus a compacted provider state.
        let old_messages = vec![
            Message::new_text(Role::User, "old user turn"),
            Message::new_text(Role::Assistant, "old assistant turn"),
        ];
        let old_state =
            ProviderConversationState::OpenAiResponses(tact_llm::ResponsesConversationState {
                version: 1,
                provider: "openai_responses".to_string(),
                base_url: server.uri(),
                model: "mock-model".to_string(),
                input_items: vec![serde_json::json!({"type": "message", "id": "old_msg_1"})],
                compaction_id: Some("cmp_old".to_string()),
                is_compacted: true,
                logical_message_count: 2,
                logical_context_hash: tact_llm::context_hash(&old_messages).unwrap(),
            });
        sqlite
            .replace_session_messages_and_provider_state(
                "session-1",
                &old_messages,
                Some(&old_state),
            )
            .await
            .unwrap();

        // The native compact endpoint succeeds, but persisting its usage
        // (`responses_compact` token-usage row) fails in the database.
        sqlite.inject_token_usage_insert_failure().await.unwrap();
        let store: crate::store::DynSessionStore = std::sync::Arc::new(sqlite);

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut agent = responses_test_agent("responses_compact_usage_failure", &server.uri());
        agent = agent
            .with_ui_channel(tx)
            .with_session("session-1".to_string(), store);
        agent.runtime.context = old_messages.clone();
        agent.runtime.provider_state = Some(old_state.clone());

        let error = agent
            .compact_history(None)
            .await
            .expect_err("usage persistence failure must surface from native compaction");
        assert!(
            format!("{error:#}").contains("failed to persist Responses compact usage"),
            "error must carry the Responses compact usage context, got: {error:#}"
        );

        // Atomic correctness: usage persistence precedes the commit, so a
        // usage failure must leave the old runtime state fully intact.
        assert_eq!(
            agent.runtime.provider_state.as_ref(),
            Some(&old_state),
            "runtime provider state must remain the old committed state"
        );
        assert_eq!(
            serde_json::to_value(&agent.runtime.context).unwrap(),
            serde_json::to_value(&old_messages).unwrap(),
            "runtime context must remain the old committed context"
        );
        assert_eq!(
            agent.runtime.stats.compactions, 0,
            "no compaction may be recorded when usage persistence failed"
        );
        assert!(
            !agent.runtime.compact_state.has_compacted,
            "runtime must not report compacted when usage persistence failed"
        );

        // No success Info may be emitted for a compaction whose usage row
        // never persisted.
        let mut updates = Vec::new();
        while let Ok(update) = rx.try_recv() {
            updates.push(update);
        }
        assert!(
            updates.iter().all(|u| !matches!(
                u,
                tact_protocol::AgentUpdate::Info(msg) if msg.contains("[responses compacted")
            )),
            "no compaction success Info may be emitted, got: {updates:?}"
        );

        // DB state intact: old messages and old provider state remain, and no
        // `responses_compact` usage row was recorded.
        let pool = sqlx::SqlitePool::connect(&format!(
            "sqlite:{}",
            dir.path().join("session.db").display()
        ))
        .await
        .unwrap();
        let message_count: i64 =
            sqlx::query("SELECT COUNT(*) as cnt FROM messages WHERE session_id = 'session-1'")
                .fetch_one(&pool)
                .await
                .unwrap()
                .try_get("cnt")
                .unwrap();
        assert_eq!(
            message_count, 2,
            "old committed messages must stay in the database"
        );
        let state_count: i64 = sqlx::query(
            "SELECT COUNT(*) as cnt FROM responses_states WHERE session_id = 'session-1'",
        )
        .fetch_one(&pool)
        .await
        .unwrap()
        .try_get("cnt")
        .unwrap();
        assert_eq!(
            state_count, 1,
            "old committed provider state must stay in the database"
        );
        let usage_count: i64 =
            sqlx::query("SELECT COUNT(*) as cnt FROM token_usages WHERE session_id = 'session-1'")
                .fetch_one(&pool)
                .await
                .unwrap()
                .try_get("cnt")
                .unwrap();
        assert_eq!(
            usage_count, 0,
            "no token usage row may be recorded for the failed compact call"
        );
    }

    #[tokio::test]
    async fn responses_compact_history_rejects_malformed_compact_resource() {
        ensure_config();
        let server = MockServer::start().await;
        // No compaction item in the output → protocol error, not retried.
        Mock::given(method("POST"))
            .and(path("/responses/compact"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": "cmp_bad",
                "object": "response.compaction",
                "output": []
            })))
            .expect(1)
            .mount(&server)
            .await;

        let mut agent = responses_test_agent("responses_compact_malformed", &server.uri());
        agent
            .runtime
            .context
            .push(Message::new_text(Role::User, "hello"));

        let error = agent
            .compact_history(None)
            .await
            .expect_err("malformed compact resource must fail");
        assert!(
            error.to_string().contains("compaction item"),
            "error should describe the malformed resource, got: {error}"
        );
        assert!(
            agent.runtime.provider_state.is_none(),
            "failed native compaction must leave the old committed state intact"
        );
        server.verify().await;
    }

    #[test]
    fn responses_auto_compact_requires_reported_usage() {
        ensure_config();
        // Explicit local snapshot (window 500_000, max_tokens 8192 → threshold
        // 400_000) so this test never depends on process-global config, which
        // parallel tests may override via `install_or_override`.
        let defaults = crate::config::AgentSettings {
            model: "mock-model".to_string(),
            reasoning_effort: None,
            model_context_window: 500_000,
            max_tokens: 8192,
            thinking_budget: 0,
            snapshot_max_items: 80,
            notifications_enabled: false,
            micro_compact_enabled: true,
            skill_body_auto_inject: false,
            skill_dirs: Vec::new(),
            instruction_sources: crate::config::InstructionSources::default(),
            subagent: None,
        };
        let agent = responses_test_agent("responses_auto_compact", "https://api.openai.com/v1")
            .with_agent_settings(defaults);
        assert!(!agent.auto_compact_due(0), "no usage yet → no auto compact");

        let mut agent = agent;
        // projected = usage + incoming + max_tokens; threshold = 400_000.
        agent.runtime.last_token_total = 390_000;
        assert!(
            !agent.auto_compact_due(0),
            "usage + max_tokens below threshold must not compact"
        );
        agent.runtime.last_token_total = 395_000;
        assert!(
            agent.auto_compact_due(10_000),
            "usage + incoming turn crossing the threshold must compact"
        );
        agent.runtime.last_token_total = 400_000;
        assert!(agent.auto_compact_due(0), "usage at threshold must compact");
    }

    #[test]
    fn non_responses_auto_compact_keeps_context_estimate_trigger() {
        ensure_config();
        let mut agent = chat_completions_test_agent("non_responses_auto_compact");
        let tiny = crate::config::AgentSettings {
            model: "mock-model".to_string(),
            reasoning_effort: None,
            model_context_window: 1_000,
            max_tokens: 100,
            thinking_budget: 0,
            snapshot_max_items: 10,
            notifications_enabled: false,
            micro_compact_enabled: true,
            skill_body_auto_inject: false,
            skill_dirs: Vec::new(),
            instruction_sources: crate::config::InstructionSources::default(),
            subagent: None,
        };
        agent.agent_settings = tiny;
        // Threshold = 80% of 1000 = 800 tokens; a ~4000-char message
        // estimates above that, so the estimate branch fires even with no
        // provider usage reported.
        agent
            .runtime
            .context
            .push(Message::new_text(Role::User, "x".repeat(4_000)));
        assert!(agent.auto_compact_due(0));
        agent.runtime.context.clear();
        assert!(!agent.auto_compact_due(0));
    }

    #[tokio::test]
    async fn responses_agent_loop_passes_and_commits_provider_state() {
        ensure_config();
        let server = MockServer::start().await;
        let completed = serde_json::json!({
            "type": "response.completed",
            "sequence_number": 1,
            "response": {
                "id": "resp_1",
                "object": "response",
                "created_at": 1,
                "completed_at": 2,
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
                        "text": "hello there"
                    }]
                }],
                "usage": {
                    "input_tokens": 10,
                    "input_tokens_details": { "cached_tokens": 0 },
                    "output_tokens": 2,
                    "output_tokens_details": { "reasoning_tokens": 0 },
                    "total_tokens": 12
                }
            }
        });
        let sse_body = format!("data: {completed}\n\n");
        Mock::given(method("POST"))
            .and(path("/responses"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .set_body_string(sse_body),
            )
            .expect(1)
            .mount(&server)
            .await;

        let mut agent = responses_test_agent("responses_agent_loop_state", &server.uri());
        agent
            .agent_loop(Some(Message::new_text(Role::User, "hi")))
            .await
            .expect("agent loop should complete");

        // The logical context got user + assistant messages.
        assert_eq!(agent.runtime.context.len(), 2);

        // The response committed a provider state covering the request
        // messages plus the pushed assistant message (the protocol baseline
        // already contains the terminal output), so the anchor is the
        // post-assistant logical context.
        let Some(ProviderConversationState::OpenAiResponses(state)) = &agent.runtime.provider_state
        else {
            panic!("agent loop must commit provider state after the LLM response");
        };
        assert_eq!(state.logical_message_count, 2);
        assert_eq!(
            state.logical_context_hash,
            tact_llm::context_hash(&agent.runtime.context).unwrap(),
            "committed anchor must cover the post-assistant logical context"
        );
        assert!(!state.is_compacted);
        assert_eq!(state.input_items.len(), 2, "user input + assistant output");
        assert!(
            state
                .input_items
                .iter()
                .any(|item| item.get("type").and_then(|v| v.as_str()) == Some("message")),
            "baseline must contain the converted messages"
        );

        // The request sent to the endpoint carried the user input converted to
        // wire items (state-aware conversion path was used).
        let requests = server.received_requests().await.expect("received requests");
        assert_eq!(requests.len(), 1);
        let body: serde_json::Value =
            serde_json::from_slice(&requests[0].body).expect("request body is JSON");
        let input = body["input"].as_array().expect("input array");
        assert_eq!(
            input.len(),
            1,
            "one user message converted for the first request"
        );
        assert_eq!(input[0]["role"], "user");
    }

    #[tokio::test]
    async fn responses_agent_loop_two_turns_never_duplicates_assistant_tool_items() {
        ensure_config();
        let server = MockServer::start().await;
        // Turn 1: reasoning + assistant message + read_file function call
        // (the agent executes the tool and continues). Turn 2: terminal
        // assistant message. Served sequentially by request index.
        let turn1 = serde_json::json!({
            "type": "response.completed",
            "sequence_number": 1,
            "response": {
                "id": "resp_1",
                "object": "response",
                "created_at": 1,
                "completed_at": 2,
                "status": "completed",
                "model": "gpt-5",
                "output": [
                    {
                        "type": "reasoning",
                        "id": "rs_1",
                        "summary": [{"type": "summary_text", "text": "plan"}],
                        "encrypted_content": "encrypted-plan-1",
                        "status": "completed"
                    },
                    {
                        "type": "message",
                        "id": "msg_1",
                        "role": "assistant",
                        "status": "completed",
                        "content": [{
                            "type": "output_text",
                            "annotations": [],
                            "logprobs": null,
                            "text": "reading the file"
                        }]
                    },
                    {
                        "type": "function_call",
                        "arguments": "{\"path\":\"a.txt\"}",
                        "call_id": "call_1",
                        "name": "read_file",
                        "id": "fc_1",
                        "status": "completed"
                    }
                ],
                "usage": {
                    "input_tokens": 10,
                    "input_tokens_details": { "cached_tokens": 0 },
                    "output_tokens": 5,
                    "output_tokens_details": { "reasoning_tokens": 2 },
                    "total_tokens": 15
                }
            }
        });
        let turn2 = serde_json::json!({
            "type": "response.completed",
            "sequence_number": 1,
            "response": {
                "id": "resp_2",
                "object": "response",
                "created_at": 3,
                "completed_at": 4,
                "status": "completed",
                "model": "gpt-5",
                "output": [{
                    "type": "message",
                    "id": "msg_2",
                    "role": "assistant",
                    "status": "completed",
                    "content": [{
                        "type": "output_text",
                        "annotations": [],
                        "logprobs": null,
                        "text": "done reading"
                    }]
                }],
                "usage": {
                    "input_tokens": 12,
                    "input_tokens_details": { "cached_tokens": 0 },
                    "output_tokens": 2,
                    "output_tokens_details": { "reasoning_tokens": 0 },
                    "total_tokens": 14
                }
            }
        });
        let call_count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let call_count_for_responder = call_count.clone();
        Mock::given(method("POST"))
            .and(path("/responses"))
            .respond_with(move |_request: &wiremock::Request| {
                let index =
                    call_count_for_responder.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let body = if index == 0 { &turn1 } else { &turn2 };
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .set_body_string(format!("data: {body}\n\n"))
            })
            .expect(2)
            .mount(&server)
            .await;

        let context = crate::tool::test_support::test_context("responses_two_turns");
        crate::tool::test_support::write_workspace_file(&context.work_dir, "a.txt", "aaa");
        let mut tool_context = context;
        tool_context.ui_tx = None;
        let mut agent = Agent::new(
            LlmProvider::OpenAiResponses(tact_llm::openai::responses::OpenAiResponsesAdapter::new(
                "test-key",
                server.uri(),
                None,
            )),
            tool_context,
            crate::tool::toolset(),
            crate::mcp::MCPToolRouter::new(),
            crate::permission::PermissionManager::try_new(crate::permission::PermissionMode::Auto)
                .unwrap(),
            AgentSystemPrompt::Static("test".to_string()),
        );

        agent
            .agent_loop(Some(Message::new_text(Role::User, "read a.txt")))
            .await
            .expect("agent loop should finish both turns");
        server.verify().await;

        // Logical context: user, assistant (tool call), user (tool result),
        // assistant (final).
        assert_eq!(agent.runtime.context.len(), 4);

        // The committed anchor covers the full post-assistant logical context.
        let Some(ProviderConversationState::OpenAiResponses(state)) = &agent.runtime.provider_state
        else {
            panic!("agent loop must commit provider state after the LLM response");
        };
        assert_eq!(
            state.logical_message_count,
            agent.runtime.context.len(),
            "committed anchor must match the post-assistant logical context"
        );
        assert_eq!(
            state.logical_context_hash,
            tact_llm::context_hash(&agent.runtime.context).unwrap()
        );
        assert_eq!(
            state.input_items.len(),
            6,
            "user + reasoning + message + function_call + function_call_output + message"
        );

        // Second request body: the baseline is replayed verbatim and only the
        // new user/tool suffix is converted — no duplicated assistant,
        // reasoning, or function-call items/ids.
        let requests = server.received_requests().await.expect("received requests");
        assert_eq!(requests.len(), 2, "exactly two /responses requests");
        let second: serde_json::Value =
            serde_json::from_slice(&requests[1].body).expect("second request body is JSON");
        let input = second["input"].as_array().expect("second request input");
        assert_eq!(
            input.len(),
            5,
            "baseline (4) + one converted tool-result suffix item, got: {input:?}"
        );
        let reasoning_items = input
            .iter()
            .filter(|item| item["type"] == "reasoning")
            .collect::<Vec<_>>();
        assert_eq!(
            reasoning_items.len(),
            1,
            "reasoning item must not be duplicated, got: {input:?}"
        );
        assert_eq!(reasoning_items[0]["id"], "rs_1");
        let assistant_messages = input
            .iter()
            .filter(|item| item["type"] == "message" && item["role"] == "assistant")
            .collect::<Vec<_>>();
        assert_eq!(
            assistant_messages.len(),
            1,
            "assistant message must not be duplicated, got: {input:?}"
        );
        assert_eq!(assistant_messages[0]["id"], "msg_1");
        let function_calls = input
            .iter()
            .filter(|item| item["type"] == "function_call")
            .collect::<Vec<_>>();
        assert_eq!(
            function_calls.len(),
            1,
            "function_call item must not be duplicated, got: {input:?}"
        );
        assert_eq!(function_calls[0]["call_id"], "call_1");
        assert_eq!(function_calls[0]["id"], "fc_1");
        // The only newly converted item is the tool result of the executed
        // read_file call.
        let outputs = input
            .iter()
            .filter(|item| item["type"] == "function_call_output")
            .collect::<Vec<_>>();
        assert_eq!(
            outputs.len(),
            1,
            "exactly one tool-result suffix item, got: {input:?}"
        );
        assert_eq!(outputs[0]["call_id"], "call_1");
        assert_eq!(outputs[0]["output"], "aaa");
        // Baseline items are replayed verbatim (same ids, same order).
        assert_eq!(input[0]["type"], "message");
        assert_eq!(input[0]["role"], "user");
        assert_eq!(input[1]["type"], "reasoning");
        assert_eq!(input[2]["id"], "msg_1");
        assert_eq!(input[3]["id"], "fc_1");
        assert_eq!(input[4]["type"], "function_call_output");
    }

    #[tokio::test]
    async fn responses_agent_loop_automatic_compaction_stays_single_stream_call() {
        ensure_config();
        let server = MockServer::start().await;
        // A streamed terminal response that includes a provider-side
        // automatic-compaction item (the opaque encrypted content must never
        // surface in diagnostics).
        let completed = serde_json::json!({
            "type": "response.completed",
            "sequence_number": 1,
            "response": {
                "id": "resp_auto_1",
                "object": "response",
                "created_at": 1,
                "completed_at": 2,
                "status": "completed",
                "model": "gpt-5",
                "output": [
                    {
                        "type": "reasoning",
                        "id": "rs_1",
                        "summary": [{"type": "summary_text", "text": "reasoning"}],
                        "encrypted_content": "opaque-reasoning-encrypted-content",
                        "status": "completed"
                    },
                    {
                        "type": "message",
                        "id": "msg_1",
                        "role": "assistant",
                        "status": "completed",
                        "content": [{
                            "type": "output_text",
                            "annotations": [],
                            "logprobs": null,
                            "text": "automatic compaction handled inline"
                        }]
                    },
                    {
                        "type": "compaction",
                        "id": "cmp_auto_01",
                        "encrypted_content": "opaque-encrypted-compaction-content"
                    }
                ],
                "usage": {
                    "input_tokens": 160012,
                    "input_tokens_details": { "cached_tokens": 120000 },
                    "output_tokens": 120,
                    "output_tokens_details": { "reasoning_tokens": 0 },
                    "total_tokens": 160132
                }
            }
        });
        let sse_body = format!("data: {completed}\n\n");
        Mock::given(method("POST"))
            .and(path("/responses"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .set_body_string(sse_body),
            )
            .expect(1)
            .mount(&server)
            .await;
        // Automatic compaction must NOT trigger a second HTTP call to the
        // explicit compact endpoint.
        Mock::given(method("POST"))
            .and(path("/responses/compact"))
            .respond_with(ResponseTemplate::new(500))
            .expect(0)
            .mount(&server)
            .await;

        let dir = tempfile::tempdir().unwrap();
        let store = crate::store::open_sqlite_session_store(&dir.path().join("session.db"))
            .await
            .unwrap();
        store
            .create_session("session-1", dir.path().to_str().unwrap(), "")
            .await
            .unwrap();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut agent = responses_test_agent("responses_auto_compact_stream", &server.uri());
        agent = agent
            .with_ui_channel(tx)
            .with_session("session-1".to_string(), store);
        agent
            .agent_loop(Some(Message::new_text(Role::User, "hi")))
            .await
            .expect("agent loop should complete with provider-side compaction");

        // Exactly one streamed /responses request; no explicit compact call.
        server.verify().await;

        // The compaction item committed to the provider state without a
        // separate HTTP call.
        let Some(ProviderConversationState::OpenAiResponses(state)) = &agent.runtime.provider_state
        else {
            panic!("agent loop must commit provider state after the LLM response");
        };
        assert!(
            state.is_compacted,
            "provider state must reflect auto compaction"
        );
        assert_eq!(state.compaction_id.as_deref(), Some("cmp_auto_01"));

        // Usage accounting stays on the ordinary stream call: exactly one
        // `stream` row and no `responses_compact` row for a call that never
        // happened.
        let pool = sqlx::SqlitePool::connect(&format!(
            "sqlite:{}",
            dir.path().join("session.db").display()
        ))
        .await
        .unwrap();
        let rows = sqlx::query("SELECT call_type FROM token_usages WHERE session_id = 'session-1'")
            .fetch_all(&pool)
            .await
            .unwrap();
        let call_types: Vec<String> = rows
            .iter()
            .map(|row| row.try_get("call_type").unwrap())
            .collect();
        assert_eq!(
            call_types,
            vec!["stream"],
            "automatic compaction must stay associated with the stream call, \
             got: {call_types:?}"
        );

        // Encrypted compaction content and the reasoning encrypted envelope
        // never surface in Info diagnostics: the compaction payload is
        // strictly provider state/request body, and the reasoning envelope
        // lives only in the internal Thinking.signature.
        let mut updates = Vec::new();
        while let Ok(update) = rx.try_recv() {
            updates.push(update);
        }
        for secret in [
            "opaque-encrypted-compaction-content",
            "opaque-reasoning-encrypted-content",
        ] {
            assert!(
                updates
                    .iter()
                    .filter_map(|u| match u {
                        tact_protocol::AgentUpdate::Info(msg) => Some(msg.as_str()),
                        _ => None,
                    })
                    .all(|msg| !msg.contains(secret)),
                "encrypted payload {secret:?} must never surface in Info updates: {updates:?}"
            );
        }
    }

    #[tokio::test]
    async fn agent_loop_completes_with_end_turn_response() {
        ensure_config();
        let context = test_context("agent_loop_end_turn");
        let router = crate::tool::toolset();
        let mcp = crate::mcp::MCPToolRouter::new();
        let perm = crate::permission::PermissionManager::try_new(
            crate::permission::PermissionMode::Default,
        )
        .unwrap();

        let mock = MockClient::new(vec![(
            vec![make_text_block("Hello, I am a coding agent.")],
            Some(StopReason::EndTurn),
        )]);

        let mut agent = Agent::new(
            LlmProvider::Mock(mock),
            context,
            router,
            mcp,
            perm,
            AgentSystemPrompt::Static("You are a test agent.".to_string()),
        );

        let result = agent
            .agent_loop(Some(Message::new_text(Role::User, "Say hello")))
            .await;

        assert!(
            result.is_ok(),
            "agent_loop should complete: {:?}",
            result.err()
        );
        assert!(
            agent.runtime.context.len() >= 2,
            "context should have at least user + assistant messages"
        );
    }

    #[tokio::test]
    async fn agent_loop_surfaces_refusal_as_error() {
        ensure_config();
        use tact_protocol::AgentUpdate;

        let context = test_context("agent_loop_refusal");
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut tool_context = context;
        tool_context.ui_tx = Some(tx.clone());

        let mock = MockClient::new(vec![(
            vec![make_text_block("I cannot help with that.")],
            Some(StopReason::Refusal),
        )]);

        let mut agent = Agent::new(
            LlmProvider::Mock(mock),
            tool_context,
            crate::tool::toolset(),
            crate::mcp::MCPToolRouter::new(),
            crate::permission::PermissionManager::try_new(
                crate::permission::PermissionMode::Default,
            )
            .unwrap(),
            AgentSystemPrompt::Static("You are a test agent.".to_string()),
        )
        .with_ui_channel(tx);

        let result = agent
            .agent_loop(Some(Message::new_text(Role::User, "unsafe request")))
            .await;

        let err = result.expect_err("refusal should return Err");
        assert!(
            err.to_string().contains("refusal"),
            "error should mention refusal, got: {err}"
        );

        let mut updates = Vec::new();
        while let Ok(u) = rx.try_recv() {
            updates.push(u);
        }
        assert!(
            updates
                .iter()
                .any(|u| matches!(u, AgentUpdate::Info(msg) if msg.contains("refused"))),
            "expected Info about refusal, got: {updates:?}"
        );
    }

    #[test]
    fn next_step_idx_increments() {
        let context = test_context("next_step_idx");
        let router = crate::tool::toolset();
        let mcp = crate::mcp::MCPToolRouter::new();
        let perm = crate::permission::PermissionManager::try_new(
            crate::permission::PermissionMode::Default,
        )
        .unwrap();

        let mut agent = Agent::new(
            LlmProvider::Mock(MockClient::new(vec![])),
            context,
            router,
            mcp,
            perm,
            AgentSystemPrompt::Static("test".to_string()),
        );

        assert_eq!(agent.next_step_idx(), 0);
        assert_eq!(agent.next_step_idx(), 1);
        assert_eq!(agent.next_step_idx(), 2);
    }

    #[test]
    fn agent_new_preserves_work_dir_in_tool_context() {
        let context = test_context("agent_work_dir");
        let router = crate::tool::toolset();
        let mcp = crate::mcp::MCPToolRouter::new();
        let perm = crate::permission::PermissionManager::try_new(
            crate::permission::PermissionMode::Default,
        )
        .unwrap();

        let expected = context.work_dir.clone();

        let agent = Agent::new(
            LlmProvider::Mock(MockClient::new(vec![])),
            context,
            router,
            mcp,
            perm,
            AgentSystemPrompt::Static("test".to_string()),
        );

        assert_eq!(agent.tool_context.work_dir, expected);
    }

    #[test]
    fn with_ui_channel_syncs_tool_context_ui_tx() {
        ensure_config();
        use crate::tool::test_support::test_context;

        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let context = test_context("ui_tx_sync");
        let agent = Agent::new(
            LlmProvider::Mock(MockClient::new(vec![])),
            context,
            crate::tool::toolset(),
            crate::mcp::MCPToolRouter::new(),
            crate::permission::PermissionManager::try_new(
                crate::permission::PermissionMode::Default,
            )
            .unwrap(),
            AgentSystemPrompt::Static("test".to_string()),
        )
        .with_ui_channel(tx);

        assert!(agent.runtime.ui_tx.is_some());
        assert!(agent.tool_context.ui_tx.is_some());
    }

    #[tokio::test]
    async fn agent_loop_runs_parallel_read_tools() {
        ensure_config();
        use tact_protocol::AgentUpdate;

        use crate::tool::test_support::{test_context, write_workspace_file};

        let context = test_context("agent_parallel_reads");
        let work_dir = context.work_dir.clone();
        write_workspace_file(&work_dir, "a.txt", "aaa");
        write_workspace_file(&work_dir, "b.txt", "bbb");

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut tool_context = context;
        tool_context.ui_tx = Some(tx.clone());

        let mock = MockClient::new(vec![
            (
                vec![
                    ContentBlock::ToolUse {
                        id: "r1".to_string(),
                        name: "read_file".to_string(),
                        input: serde_json::json!({ "path": "a.txt" }),
                    },
                    ContentBlock::ToolUse {
                        id: "r2".to_string(),
                        name: "read_file".to_string(),
                        input: serde_json::json!({ "path": "b.txt" }),
                    },
                ],
                Some(StopReason::ToolUse),
            ),
            (
                vec![make_text_block("reads done")],
                Some(StopReason::EndTurn),
            ),
        ]);

        let mut agent = Agent::new(
            LlmProvider::Mock(mock),
            tool_context,
            crate::tool::toolset(),
            crate::mcp::MCPToolRouter::new(),
            crate::permission::PermissionManager::try_new(crate::permission::PermissionMode::Auto)
                .unwrap(),
            AgentSystemPrompt::Static("test".to_string()),
        )
        .with_ui_channel(tx);

        agent
            .agent_loop(Some(Message::new_text(Role::User, "read both")))
            .await
            .expect("agent_loop");

        let mut updates = Vec::new();
        while let Ok(u) = rx.try_recv() {
            updates.push(u);
        }

        let finished: Vec<_> = updates
            .iter()
            .filter_map(|u| match u {
                AgentUpdate::StepFinished {
                    tool_id, result, ..
                } if result.tool == "read_file" => Some(tool_id.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(finished.len(), 2);
        assert!(finished.contains(&"r1"));
        assert!(finished.contains(&"r2"));
    }

    #[tokio::test]
    async fn agent_loop_plan_mode_denies_write() {
        ensure_config();
        use tact_protocol::AgentUpdate;

        use crate::tool::test_support::test_context;

        let context = test_context("agent_plan_deny");
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut tool_context = context;
        tool_context.ui_tx = Some(tx.clone());

        let mock = MockClient::new(vec![
            (
                vec![ContentBlock::ToolUse {
                    id: "w1".to_string(),
                    name: "write_file".to_string(),
                    input: serde_json::json!({ "path": "x.txt", "content": "data" }),
                }],
                Some(StopReason::ToolUse),
            ),
            (
                vec![make_text_block("continued")],
                Some(StopReason::EndTurn),
            ),
        ]);

        let mut agent = Agent::new(
            LlmProvider::Mock(mock),
            tool_context,
            crate::tool::toolset(),
            crate::mcp::MCPToolRouter::new(),
            crate::permission::PermissionManager::try_new(crate::permission::PermissionMode::Plan)
                .unwrap(),
            AgentSystemPrompt::Static("test".to_string()),
        )
        .with_ui_channel(tx);

        agent
            .agent_loop(Some(Message::new_text(Role::User, "write")))
            .await
            .expect("agent_loop");

        let mut updates = Vec::new();
        while let Ok(u) = rx.try_recv() {
            updates.push(u);
        }

        assert!(
            updates.iter().any(|u| {
                matches!(
                    u,
                    AgentUpdate::StepFailed { tool_id, error, .. }
                        if tool_id == "w1" && error.contains("Plan mode")
                )
            }),
            "Plan mode should StepFailed on write, got: {updates:?}"
        );
    }

    #[tokio::test]
    async fn agent_loop_emits_token_usage_from_mock() {
        ensure_config();
        use tact_protocol::{AgentUpdate, TokenUsageInfo};

        use crate::tool::test_support::test_context;

        let context = test_context("agent_token_usage");
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut tool_context = context;
        tool_context.ui_tx = Some(tx.clone());

        let usage = TokenUsageInfo {
            prompt: 50,
            completion: 10,
            total: 60,
            prompt_cache_hit_tokens: 0,
            prompt_cache_miss_tokens: 50,
            reasoning_tokens: 0,
        };

        let mock = MockClient::with_usage(vec![(
            vec![make_text_block("ok")],
            Some(StopReason::EndTurn),
            usage.clone(),
        )]);

        let mut agent = Agent::new(
            LlmProvider::Mock(mock),
            tool_context,
            crate::tool::toolset(),
            crate::mcp::MCPToolRouter::new(),
            crate::permission::PermissionManager::try_new(crate::permission::PermissionMode::Auto)
                .unwrap(),
            AgentSystemPrompt::Static("test".to_string()),
        )
        .with_ui_channel(tx);

        agent
            .agent_loop(Some(Message::new_text(Role::User, "hi")))
            .await
            .expect("agent_loop");

        let mut updates = Vec::new();
        while let Ok(u) = rx.try_recv() {
            updates.push(u);
        }

        assert!(
            updates.iter().any(|u| {
                matches!(
                    u,
                    AgentUpdate::TokenUsage(u) if u.total == usage.total
                )
            }),
            "expected TokenUsage from mock, got: {updates:?}"
        );
    }

    #[tokio::test]
    async fn agent_loop_serializes_read_before_write_on_same_file() {
        ensure_config();
        use tact_protocol::AgentUpdate;

        use crate::tool::test_support::{test_context, write_workspace_file};

        let context = test_context("agent_read_write_serial");
        let work_dir = context.work_dir.clone();
        write_workspace_file(&work_dir, "shared.txt", "seed");

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut tool_context = context;
        tool_context.ui_tx = Some(tx.clone());

        let mock = MockClient::new(vec![
            (
                vec![
                    ContentBlock::ToolUse {
                        id: "r1".to_string(),
                        name: "read_file".to_string(),
                        input: serde_json::json!({ "path": "shared.txt" }),
                    },
                    ContentBlock::ToolUse {
                        id: "w1".to_string(),
                        name: "write_file".to_string(),
                        input: serde_json::json!({ "path": "shared.txt", "content": "next" }),
                    },
                ],
                Some(StopReason::ToolUse),
            ),
            (vec![make_text_block("done")], Some(StopReason::EndTurn)),
        ]);

        let mut agent = Agent::new(
            LlmProvider::Mock(mock),
            tool_context,
            crate::tool::toolset(),
            crate::mcp::MCPToolRouter::new(),
            crate::permission::PermissionManager::try_new(crate::permission::PermissionMode::Auto)
                .unwrap(),
            AgentSystemPrompt::Static("test".to_string()),
        )
        .with_ui_channel(tx);

        agent
            .agent_loop(Some(Message::new_text(Role::User, "rw")))
            .await
            .expect("agent_loop");

        let mut updates = Vec::new();
        while let Ok(u) = rx.try_recv() {
            updates.push(u);
        }

        let read_done = updates.iter().position(
            |u| matches!(u, AgentUpdate::StepFinished { tool_id, .. } if tool_id == "r1"),
        );
        let write_done = updates.iter().position(
            |u| matches!(u, AgentUpdate::StepFinished { tool_id, .. } if tool_id == "w1"),
        );
        assert!(
            read_done.is_some() && write_done.is_some() && read_done < write_done,
            "read must finish before write on same file, got: {updates:?}"
        );
        assert_eq!(
            std::fs::read_to_string(work_dir.join("shared.txt")).unwrap(),
            "next"
        );
    }

    #[test]
    fn assemble_agents_md_prompt_reads_workdir_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("AGENTS.md"),
            "# Crate map\n\n- `crates/tact` — runtime\n",
        )
        .unwrap();

        let rendered =
            assemble_agents_md_prompt(dir.path(), &crate::config::InstructionSources::default());
        assert!(rendered.starts_with("## AGENTS.md instructions"));
        assert!(rendered.contains("### From project root (AGENTS.md)"));
        assert!(rendered.contains("Crate map"));
        assert!(rendered.contains("crates/tact"));
    }

    #[test]
    fn assemble_agents_md_prompt_empty_when_missing() {
        let dir = tempfile::tempdir().unwrap();
        assert!(
            assemble_agents_md_prompt(dir.path(), &crate::config::InstructionSources::default())
                .is_empty()
        );
    }

    #[test]
    fn assemble_agents_md_prompt_skipped_when_disabled() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("AGENTS.md"), "# Rules\n").unwrap();
        let sources =
            crate::config::InstructionSources::from_config(Some(vec!["claude_md_project".into()]))
                .unwrap();
        assert!(assemble_agents_md_prompt(dir.path(), &sources).is_empty());
    }

    #[test]
    fn assemble_claude_md_prompt_skipped_when_disabled() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("CLAUDE.md"), "# Claude rules\n").unwrap();
        assert!(
            assemble_claude_md_prompt(dir.path(), &crate::config::InstructionSources::default())
                .is_empty()
        );
    }

    #[test]
    fn assemble_claude_md_prompt_reads_project_when_enabled() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("CLAUDE.md"), "# Claude rules\n").unwrap();
        let sources =
            crate::config::InstructionSources::from_config(Some(vec!["claude_md_project".into()]))
                .unwrap();
        let rendered = assemble_claude_md_prompt(dir.path(), &sources);
        assert!(rendered.starts_with("## CLAUDE.md instructions"));
        assert!(rendered.contains("### From project root (CLAUDE.md)"));
        assert!(rendered.contains("Claude rules"));
    }

    #[test]
    fn cached_md_section_computes_once() {
        let mut cache = None;
        let mut calls = 0usize;
        let first = cached_md_section(&mut cache, || {
            calls += 1;
            "hello".to_string()
        });
        let second = cached_md_section(&mut cache, || {
            calls += 1;
            "should-not-run".to_string()
        });
        assert_eq!(first, "hello");
        assert_eq!(second, "hello");
        assert_eq!(calls, 1);
        assert_eq!(cache.as_deref(), Some("hello"));
    }

    #[test]
    fn cached_md_section_caches_empty_string() {
        let mut cache = None;
        let mut calls = 0usize;
        let _ = cached_md_section(&mut cache, || {
            calls += 1;
            String::new()
        });
        let _ = cached_md_section(&mut cache, || {
            calls += 1;
            "later".to_string()
        });
        assert_eq!(calls, 1);
        assert_eq!(cache.as_deref(), Some(""));
    }

    // ── ensure_max_tokens_gt_thinking_budget ──

    #[test]
    fn zero_budget_is_noop() {
        let mut mt = 8_000u32;
        assert!(Agent::ensure_max_tokens_gt_thinking_budget(&mut mt, 0).is_none());
        assert_eq!(mt, 8_000);
    }

    #[test]
    fn max_tokens_already_larger_is_noop() {
        let mut mt = 16_000u32;
        assert!(Agent::ensure_max_tokens_gt_thinking_budget(&mut mt, 8_000).is_none());
        assert_eq!(mt, 16_000);
    }

    #[test]
    fn expands_when_budget_exceeds_max_tokens() {
        let mut mt = 8_000u32;
        let msg = Agent::ensure_max_tokens_gt_thinking_budget(&mut mt, 32_000).unwrap();
        assert_eq!(mt, 32_001);
        assert!(msg.contains("32000"));
        assert!(msg.contains("32001"));
    }

    #[test]
    fn set_thinking_budget_auto_expands_max_tokens() {
        ensure_config();
        let context = test_context("set_thinking_auto_test");
        let router = crate::tool::toolset();
        let mcp = crate::mcp::MCPToolRouter::new();
        let perm = crate::permission::PermissionManager::try_new(
            crate::permission::PermissionMode::Default,
        )
        .unwrap();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();

        let mut agent = Agent::new(
            LlmProvider::Mock(MockClient::new(vec![])),
            context,
            router,
            mcp,
            perm,
            AgentSystemPrompt::Static("test".to_string()),
        )
        .with_ui_channel(tx);

        assert_eq!(agent.thinking_budget(), 0);
        agent.set_thinking_budget(64_000);
        assert!(agent.max_tokens() > 64_000);
        assert_eq!(agent.thinking_budget(), 64_000);

        let mut saw_model_info = false;
        while let Ok(update) = rx.try_recv() {
            match update {
                AgentUpdate::ModelInfo(params) => {
                    assert_eq!(params.thinking_budget, Some(64_000));
                    assert!(params.max_tokens > 64_000);
                    saw_model_info = true;
                }
                AgentUpdate::Info(_) => {}
                other => panic!("unexpected update: {other:?}"),
            }
        }
        assert!(saw_model_info, "set_thinking_budget must emit ModelInfo");
    }

    #[tokio::test]
    async fn ensure_session_restores_matching_provider_state() {
        ensure_config();
        let dir = tempfile::tempdir().unwrap();
        let store = crate::store::open_sqlite_session_store(&dir.path().join("session.db"))
            .await
            .unwrap();
        store
            .create_session("session-1", dir.path().to_str().unwrap(), "")
            .await
            .unwrap();
        let state =
            ProviderConversationState::OpenAiResponses(tact_llm::ResponsesConversationState {
                version: 1,
                provider: "openai_responses".to_string(),
                base_url: "https://api.openai.com/v1".to_string(),
                model: "mock-model".to_string(),
                input_items: vec![serde_json::json!({"type": "message"})],
                compaction_id: Some("cmp_1".to_string()),
                is_compacted: true,
                logical_message_count: 1,
                logical_context_hash: "abc".to_string(),
            });
        store
            .replace_session_messages_and_provider_state("session-1", &[], Some(&state))
            .await
            .unwrap();

        let mut agent = responses_test_agent("ensure_session_restore", "https://api.openai.com/v1");
        agent = agent.with_session("session-1".to_string(), store);
        agent
            .ensure_session()
            .await
            .expect("matching state must load");

        let Some(ProviderConversationState::OpenAiResponses(loaded)) =
            &agent.runtime.provider_state
        else {
            panic!("provider state must be loaded from the store");
        };
        assert_eq!(loaded.compaction_id.as_deref(), Some("cmp_1"));
        assert!(loaded.is_compacted);
    }

    #[tokio::test]
    async fn ensure_session_rejects_provider_state_bound_to_other_base_url() {
        ensure_config();
        let dir = tempfile::tempdir().unwrap();
        let store = crate::store::open_sqlite_session_store(&dir.path().join("session.db"))
            .await
            .unwrap();
        store
            .create_session("session-1", dir.path().to_str().unwrap(), "")
            .await
            .unwrap();
        let state =
            ProviderConversationState::OpenAiResponses(tact_llm::ResponsesConversationState {
                version: 1,
                provider: "openai_responses".to_string(),
                base_url: "https://other.example.com/v1".to_string(),
                model: "mock-model".to_string(),
                input_items: Vec::new(),
                compaction_id: None,
                is_compacted: false,
                logical_message_count: 0,
                logical_context_hash: "abc".to_string(),
            });
        store
            .replace_session_messages_and_provider_state("session-1", &[], Some(&state))
            .await
            .unwrap();

        let mut agent = responses_test_agent("ensure_session_binding", "https://api.openai.com/v1");
        agent = agent.with_session("session-1".to_string(), store);
        let error = agent
            .ensure_session()
            .await
            .expect_err("base URL mismatch must be rejected before any LLM call");
        assert!(
            error.to_string().contains("base URL"),
            "error must describe the binding mismatch, got: {error}"
        );
        assert!(
            agent.runtime.provider_state.is_none(),
            "mismatched state must not be adopted"
        );
    }
}
