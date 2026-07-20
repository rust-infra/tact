//! Agent runtime: conversation loop, tool dispatch, and session state.

mod tool_dispatch;
mod tool_schedule;

use anyhow::{Context, Result};
use chrono::Utc;
use std::path::Path;
use tact_llm::{
    ContentBlock, CreateMessageParams, Message, MessageContent, RequiredMessageParams, Role,
    StopReason, Thinking, ThinkingType,
};

use crate::ToolSpec;
use crate::compact::{
    CompactState, approx_text_tokens, build_compacted_history, collect_user_messages,
    compact_rebuild_headroom_tokens, compacted_context, estimate_context_tokens,
    estimate_message_tokens, micro_compact, recent_messages_for_summary,
    retained_user_message_token_budget, should_auto_compact, write_transcript,
};
use crate::config::AgentSettings;
use crate::hook::{Hook, HookTypes, PostToolUseFn, PreToolUseFn, SessionStartFn};
use crate::mcp::MCPToolRouter;
use crate::memory::MEMORY_GUIDANCE;
use crate::permission::PermissionManager;
use crate::prompt::SystemPrompt;
use crate::recovery::{
    CONTINUATION_MESSAGE, MAX_RECOVERY_ATTEMPTS, RecoveryState, backoff_delay,
    is_prompt_too_long_error, is_transient_transport_error,
};
use crate::stats::SessionStats;
use crate::store::DynSessionStore;
use crate::tool::{ToolContext, ToolRouter};
use tact_llm::{LlmClient, LlmProvider};
use tact_protocol::{AgentUpdate, TokenUsageInfo};

enum CompactRebuildMode {
    /// Retain recent real user messages + handoff summary (Codex-style).
    CodexStyle,
    /// Replace the entire context with a single summary user message.
    LegacySingleSummary,
}

const COMPACT_SUMMARY_MAX_TOKENS: u32 = 2_000;
const COMPACT_SUMMARY_OUTPUT_PERCENT: usize = 20;
const COMPACT_SUMMARY_HEADROOM_PERCENT: usize = 10;
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
        let cached_tool_specs = tools
            .tool_specs()
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
            },
            tool_context,
            tools,
            mcp_router,
            hooks: Vec::new(),
            system_prompt,
            tool_use_counter: 0,
            agent_settings: crate::config::settings().agent.clone(),
            cached_tool_specs,
        }
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
        self.runtime.ui_tx = Some(tx);
        self
    }

    /// Attach a session store with a fully initialized session id.
    ///
    /// Callers must create/resolve the id and persist the session row before
    /// this (startup path). Also wires DeepSeek `user_id` for KV cache isolation.
    pub fn with_session(mut self, session_id: String, store: DynSessionStore) -> Self {
        self.runtime.client.set_user_id(&session_id);
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
        store.ensure_session_row(&session_id, &root_dir).await?;

        if self.runtime.context.is_empty() {
            let history = store.load_session(&session_id).await?;
            self.runtime.context = history;
        }

        Ok(session_id)
    }

    async fn push_message(&mut self, message: Message) -> Result<()> {
        let blocks = message.content.clone();
        self.runtime.context.push(message.clone());
        self.persist_message(message.role, &blocks).await
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
        if should_auto_compact(
            self.runtime.last_token_total,
            self.model_context_window(),
            estimate_context_tokens(&self.runtime.context),
            incoming_tokens,
        ) {
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
            micro_compact(
                &mut self.runtime.context,
                self.agent_settings.micro_compact_enabled,
            );
            // Turn already in context — no incoming reservation.
            if should_auto_compact(
                self.runtime.last_token_total,
                self.model_context_window(),
                estimate_context_tokens(&self.runtime.context),
                0,
            ) {
                self.emit_update(AgentUpdate::Info("[auto compact]".into()));
                self.compact_history(None).await?;
            }

            // Snapshot the complete conversation after micro/auto compaction.
            // Includes the current user turn plus history, or retained users +
            // summary when compact_history ran above.
            let conversation_messages = self.runtime.context.clone();
            let model_name = crate::get_model();
            let request = CreateMessageParams::new(RequiredMessageParams {
                model: model_name.clone(),
                messages: conversation_messages,
                max_tokens: self.max_tokens(),
            })
            .with_system(&system_prompt)
            .with_tools(self.all_tool_specs())
            .with_stream(true)
            .with_thinking(self.thinking_config());

            self.emit_update(AgentUpdate::ModelInfo(tact_protocol::ModelCallParams {
                model: model_name,
                max_tokens: request.max_tokens,
                thinking_budget: request.thinking.as_ref().map(|t| t.budget_tokens as u32),
                reasoning_effort: request.thinking.as_ref().and_then(|t| {
                    tact_llm::current_reasoning_effort_from_budget(t.budget_tokens)
                        .map(str::to_string)
                }),
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

            let (content, stop_reason, token_usage, request_body) = match self
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
                        && self.runtime.recovery_state.compact_attempts < MAX_RECOVERY_ATTEMPTS
                    {
                        self.runtime.recovery_state.compact_attempts += 1;
                        self.emit_update(AgentUpdate::Info(format!(
                            "[Recovery] compact ({}/{}): context too large",
                            self.runtime.recovery_state.compact_attempts, MAX_RECOVERY_ATTEMPTS
                        )));
                        self.compact_history(None).await?;
                        continue;
                    }

                    if is_transient_transport_error(&error_text)
                        && self.runtime.recovery_state.transport_attempts < MAX_RECOVERY_ATTEMPTS
                    {
                        let delay = backoff_delay(self.runtime.recovery_state.transport_attempts);
                        self.runtime.recovery_state.transport_attempts += 1;
                        self.emit_update(AgentUpdate::Info(format!(
                            "[Recovery] backoff ({}/{}): retrying in {:.1}s",
                            self.runtime.recovery_state.transport_attempts,
                            MAX_RECOVERY_ATTEMPTS,
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

            self.persist_message(
                Role::Assistant,
                &MessageContent::Blocks {
                    content: content.clone(),
                },
            )
            .await?;

            if matches!(stop_reason, Some(StopReason::MaxTokens))
                && self.runtime.recovery_state.continuation_attempts < MAX_RECOVERY_ATTEMPTS
            {
                // Execute any tool calls that arrived before the token limit
                // was hit, so the context remains valid for the API.
                if has_pending_tools {
                    let (tool_result, manual_compact) = self.execute_tool_call(&content).await?;
                    self.runtime
                        .context
                        .push(Message::new_blocks(Role::User, tool_result.clone()));
                    self.persist_message(
                        Role::User,
                        &MessageContent::Blocks {
                            content: tool_result,
                        },
                    )
                    .await?;
                    if let Some(focus) = manual_compact {
                        self.emit_update(AgentUpdate::Info("[manual compact]".into()));
                        self.compact_history(Some(focus.as_str())).await?;
                    }
                }

                self.runtime.recovery_state.continuation_attempts += 1;
                self.emit_update(AgentUpdate::Info(format!(
                    "[Recovery] continue ({}/{}): output truncated",
                    self.runtime.recovery_state.continuation_attempts, MAX_RECOVERY_ATTEMPTS
                )));
                self.runtime
                    .context
                    .push(Message::new_text(Role::User, CONTINUATION_MESSAGE));
                self.persist_message(
                    Role::User,
                    &MessageContent::Text {
                        content: CONTINUATION_MESSAGE.to_string(),
                    },
                )
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

            self.runtime
                .context
                .push(Message::new_blocks(Role::User, tool_result.clone()));
            self.persist_message(
                Role::User,
                &MessageContent::Blocks {
                    content: tool_result,
                },
            )
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
        ),
        anyhow::Error,
    > {
        let ui_tx = self.runtime.ui_tx.clone();
        self.runtime
            .client
            .stream_message(request, ui_tx)
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))
    }

    pub fn session_start(&mut self, hook: impl SessionStartFn + 'static) {
        self.hooks.push(Hook::SessionStart(Box::new(hook)));
    }

    pub fn post_tool(&mut self, hook: impl PostToolUseFn + 'static) {
        self.hooks.push(Hook::PostToolUse(Box::new(hook)));
    }

    pub fn pre_tool(&mut self, hook: impl PreToolUseFn + 'static) {
        self.hooks.push(Hook::PreToolUse(Box::new(hook)));
    }

    /// Returns hooks registered for the given [`HookTypes`] variant.
    pub fn hooks_by_type(&self, hook_type: HookTypes) -> Vec<&Hook> {
        self.hooks
            .iter()
            .filter(|hook| hook_type == (*hook).into())
            .collect()
    }

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
        self.compact_history_with_mode(focus, CompactRebuildMode::CodexStyle)
            .await
    }

    /// Previous single-summary compaction (entire history → one user message).
    /// Kept for reference / rollback; production call sites use [`Self::compact_history`].
    #[allow(dead_code)]
    pub async fn compact_history_legacy(&mut self, focus: Option<&str>) -> Result<()> {
        self.compact_history_with_mode(focus, CompactRebuildMode::LegacySingleSummary)
            .await
    }

    async fn compact_history_with_mode(
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
            if approx_text_tokens(&prompt).saturating_add(approx_text_tokens(&addition))
                <= summary_input_limit
            {
                prompt.push_str(&addition);
            }
        }
        if !self.runtime.compact_state.recent_files.is_empty() {
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
                    break;
                }
                prompt.push_str(&addition);
                heading_added = true;
            }
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

        let model_name = crate::get_model();
        let request = CreateMessageParams::new(RequiredMessageParams {
            model: model_name.clone(),
            messages: vec![Message::new_text(Role::User, prompt)],
            max_tokens: summary_max_tokens,
        });

        self.emit_update(AgentUpdate::ModelInfo(tact_protocol::ModelCallParams {
            model: model_name,
            max_tokens: request.max_tokens,
            thinking_budget: request.thinking.as_ref().map(|t| t.budget_tokens as u32),
            reasoning_effort: request.thinking.as_ref().and_then(|t| {
                tact_llm::current_reasoning_effort_from_budget(t.budget_tokens).map(str::to_string)
            }),
            extra_body: request
                .thinking
                .as_ref()
                .map(|t| serde_json::json!({"thinking": t}).to_string()),
        }));
        // ── Stats: before compaction LLM call ──
        self.runtime.stats.prompt_count += 1;
        let compact_prompt_chars = serde_json::to_string(&request)
            .map(|s| s.chars().count() as u64)
            .unwrap_or(0);
        self.runtime.stats.total_prompt_chars += compact_prompt_chars;
        let compact_start = std::time::Instant::now();

        let mut retry_attempt = 0;
        let (blocks, stop_reason, token_usage, request_body) = loop {
            match self.runtime.client.create_message(&request).await {
                Ok(response) => break response,
                Err(error) => {
                    let error_text = error.to_string();
                    if retry_attempt >= MAX_RECOVERY_ATTEMPTS
                        || !is_transient_transport_error(&error_text.to_lowercase())
                    {
                        return Err(anyhow::Error::from(error));
                    }
                    retry_attempt = retry_attempt.saturating_add(1);
                    let delay = backoff_delay(retry_attempt.saturating_sub(1));
                    self.emit_update(AgentUpdate::Info(format!(
                        "[compact retry {retry_attempt}/{MAX_RECOVERY_ATTEMPTS}] retrying in {:.1}s",
                        delay.as_secs_f64()
                    )));
                    tokio::time::sleep(delay).await;
                }
            }
        };

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
            None | Some(StopReason::EndTurn) => {}
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
        if let Err(error) = self.replace_persisted_context().await {
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
        let prompt = SystemPrompt::builder()
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
                "When editing files, always re-read the file first if its content may have changed since you last read it",
                "For multi-line changes, prefer apply_patch; for exact string replacements, use edit_file (replace_all=true to change every occurrence in the file)",
                "If a tool result was compacted and you need the details, re-run the relevant tool (e.g., read_file)",
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
        format!("Model: {}", crate::get_model()),
        format!("Platform: {}", std::env::consts::OS),
    ];

    if !tree.is_empty() {
        lines.push(String::new());
        lines.push(tree);
    }

    lines.join("\n")
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

    use std::cmp::Ordering;
    use std::collections::BTreeMap;

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
    use super::*;
    use std::sync::Once;
    use tact_llm::{ContentBlock, Message, Role, StopReason};
    use tact_llm::{LlmProvider, MockClient, ProviderKind};

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
                },
                agent: crate::config::AgentSettings {
                    model_context_window: 500_000,
                    max_tokens: 8192,
                    thinking_budget: 0,
                    snapshot_max_items: 80,
                    notifications_enabled: false,
                    micro_compact_enabled: true,
                    skill_body_auto_inject: false,
                    instruction_sources: crate::config::InstructionSources::default(),
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
                    brave_search_api_key: None,
                    bash_timeout_secs: crate::config::ToolSettings::DEFAULT_BASH_TIMEOUT_SECS,
                },
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
            model_context_window: 500,
            max_tokens: 1024,
            thinking_budget: 0,
            snapshot_max_items: 10,
            notifications_enabled: false,
            micro_compact_enabled: true,
            skill_body_auto_inject: false,
            instruction_sources: crate::config::InstructionSources::default(),
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

    #[tokio::test]
    async fn agent_loop_runs_parallel_read_tools() {
        ensure_config();
        use crate::tool::test_support::{test_context, write_workspace_file};
        use tact_protocol::AgentUpdate;

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
        use crate::tool::test_support::test_context;
        use tact_protocol::AgentUpdate;

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
        use crate::tool::test_support::test_context;
        use tact_protocol::{AgentUpdate, TokenUsageInfo};

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
        use crate::tool::test_support::{test_context, write_workspace_file};
        use tact_protocol::AgentUpdate;

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
}
