//! Agent runtime: conversation loop, tool dispatch, and session state.

mod tool_dispatch;
mod tool_schedule;

use anthropic_ai_sdk::types::message::{
    ContentBlock, CreateMessageParams, Message, MessageContent, RequiredMessageParams, Role,
    StopReason, Thinking, ThinkingType,
};
use anyhow::{Context, Result};
use chrono::Utc;
use std::path::Path;

use crate::ToolSpec;
use crate::compact::{
    CompactState, compacted_context, estimate_context_size, micro_compact, write_transcript,
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
        tool_context: ToolContext,
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
        Self {
            runtime: AgentRuntime {
                client,
                context: Vec::new(),
                compact_state: CompactState::default(),
                recovery_state: RecoveryState::default(),
                permission_manager,
                stats: SessionStats::default(),
                ui_tx: None,
                cancel_flag: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
                session_store: None,
                session_id: None,
                first_message_db_id: 0,
                last_message_db_id: 0,
                llm_call_last_message_id: 0,
                cached_dir_snapshot: None,
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

    fn context_limit(&self) -> usize {
        self.agent_settings.context_limit_chars
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

    pub fn with_session(mut self, session_id: Option<String>, store: DynSessionStore) -> Self {
        self.runtime.session_store = Some(store);
        self.runtime.session_id = session_id;
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
            AgentUpdate::StepFailed(idx, _, msg) => {
                let _ = crate::notifications::notify_step_failed(*idx, msg);
            }
            _ => {}
        }

        if let Some(tx) = &self.runtime.ui_tx {
            let _ = tx.send(update);
        }
    }

    pub async fn ensure_session(&mut self) -> Result<String> {
        let Some(store) = self.runtime.session_store.as_ref() else {
            return Ok(self.runtime.session_id.clone().unwrap_or_default());
        };

        let session_id = match self.runtime.session_id.as_ref() {
            Some(id) if !id.is_empty() => id.clone(),
            _ => {
                let id = uuid::Uuid::new_v4().to_string();
                self.runtime.session_id = Some(id.clone());
                id
            }
        };

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
    pub async fn agent_loop(&mut self, initial_user_message: Option<Message>) -> Result<()> {
        self.runtime.recovery_state = RecoveryState::default();

        // Ensure a session exists and optionally restore history.
        let session_id = self.ensure_session().await?;
        // Wire the session_id as the DeepSeek `user_id` for KV cache isolation.
        self.runtime.client.set_user_id(&session_id);

        // If history is empty, add the initial user message so it is persisted.
        if self.runtime.context.is_empty() {
            if let Some(msg) = initial_user_message {
                self.push_message(msg).await?;
            }
        } else if let Some(msg) = initial_user_message {
            self.push_message(msg).await?;
        }

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
            if estimate_context_size(&self.runtime.context) > self.context_limit() {
                self.emit_update(AgentUpdate::Info("[auto compact]".into()));
                self.compact_history(None).await?;
            }

            // Re-render the system prompt each turn so memory/dynamic_context stay fresh.
            // Stable sections are placed before DYNAMIC_BOUNDARY to keep prefix cache-friendly.
            let system = self.build_system_prompt()?;

            let model_name = crate::get_model().to_string();
            let request = CreateMessageParams::new(RequiredMessageParams {
                model: model_name.clone(),
                messages: self.runtime.context.clone(),
                max_tokens: self.max_tokens(),
            })
            .with_system(&system)
            .with_tools(self.all_tool_specs())
            .with_stream(true)
            .with_thinking(self.thinking_config());

            self.emit_update(AgentUpdate::ModelInfo(tact_protocol::ModelCallParams {
                model: model_name,
                max_tokens: request.max_tokens,
                thinking_budget: request.thinking.as_ref().map(|t| t.budget_tokens as u32),
                reasoning_effort: request.thinking.as_ref().map(|_| "high".to_string()),
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

            if let Some(stop_reason) = stop_reason
                && !matches!(stop_reason, StopReason::ToolUse)
            {
                return Ok(());
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

    pub async fn compact_history(&mut self, focus: Option<&str>) -> Result<()> {
        let tact_path = crate::consts::TactPath::new(&self.tool_context.work_dir);
        let transcript_path = write_transcript(&tact_path, &self.runtime.context)?;
        self.emit_update(AgentUpdate::Info(format!(
            "[transcript saved: {}]",
            transcript_path.display()
        )));

        // Prefer recent messages (rather than earliest), since recent context matters most for continuing work
        let truncated = if self.runtime.context.is_empty() {
            String::new()
        } else {
            let mut recent_messages: Vec<&Message> = Vec::new();
            let mut char_count = 0;
            for msg in self.runtime.context.iter().rev() {
                let msg_json = serde_json::to_string(msg).unwrap_or_default();
                let msg_chars = msg_json.chars().count();
                // Keep at least one message, even if it's long
                if char_count + msg_chars > 80_000 && !recent_messages.is_empty() {
                    break;
                }
                char_count += msg_chars;
                recent_messages.push(msg);
            }
            recent_messages.reverse();
            serde_json::to_string(&recent_messages)
                .context("failed to serialize recent messages")?
        };
        let mut prompt = format!(
            "Summarize this coding-agent conversation so work can continue.\n\
Preserve:\n\
1. The current goal and what has been accomplished\n\
2. Important findings, decisions, and architectural insights\n\
3. Files read or changed (with key code structures like types, signatures, APIs if relevant)\n\
4. Remaining work and next steps\n\
5. User constraints and preferences\n\
6. Any errors encountered and their causes\n\
Be compact but concrete. Preserve exact file paths, function names, and type signatures when they are important for continuing the work.\n\n\
{truncated}"
        );
        if let Some(focus) = focus.filter(|value| !value.trim().is_empty()) {
            prompt.push_str(&format!("\n\nFocus to preserve next: {focus}"));
        }
        if !self.runtime.compact_state.recent_files.is_empty() {
            let recent = self
                .runtime
                .compact_state
                .recent_files
                .iter()
                .map(|path| format!("- {path}"))
                .collect::<Vec<_>>()
                .join("\n");
            prompt.push_str(&format!("\n\nRecent files to reopen if needed:\n{recent}"));
        }

        let model_name = crate::get_model().to_string();
        let request = CreateMessageParams::new(RequiredMessageParams {
            model: model_name.clone(),
            messages: vec![Message::new_text(Role::User, prompt)],
            max_tokens: 2000,
        });

        self.emit_update(AgentUpdate::ModelInfo(tact_protocol::ModelCallParams {
            model: model_name,
            max_tokens: request.max_tokens,
            thinking_budget: request.thinking.as_ref().map(|t| t.budget_tokens as u32),
            reasoning_effort: request.thinking.as_ref().map(|t| match t.type_ {
                ThinkingType::Enabled => "high".to_string(),
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

        let (blocks, _stop_reason, token_usage, request_body) = self
            .runtime
            .client
            .create_message(&request)
            .await
            .map_err(|e| anyhow::anyhow!(e.to_string()))?;

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
        }
        let _ = self
            .persist_llm_call("compact", token_usage.as_ref(), request_body.as_deref())
            .await;
        // After compaction the context is replaced with a summary, so
        // future messages start a new message-id window.
        self.runtime.first_message_db_id = 0;
        self.runtime.last_message_db_id = 0;
        self.runtime.llm_call_last_message_id = 0;
        let summary = blocks
            .iter()
            .filter_map(|block| match block {
                ContentBlock::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");

        self.runtime.compact_state.has_compacted = true;
        self.runtime.compact_state.last_summary = Some(summary.clone());

        // Inject recently accessed file list into summary, helping the agent recover context after amnesia
        let mut full_summary = summary;
        if !self.runtime.compact_state.recent_files.is_empty() {
            full_summary
                .push_str("\n\nRecently accessed files (re-read if you need their contents):\n");
            for path in &self.runtime.compact_state.recent_files {
                full_summary.push_str(&format!("- {path}\n"));
            }
        }
        self.runtime.context = compacted_context(full_summary);
        self.replace_persisted_context().await?;
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
                "For multi-line changes, prefer apply_patch; for single-line exact replacements, use edit_file",
                "If a tool result was compacted and you need the details, re-run the relevant tool (e.g., read_file)",
                "For small edits to existing files, prefer edit_file over write_file; use write_file only for new files or complete rewrites",
            ])
            .skills_available(self.tool_context.skill_registry.describe_available())
            .memory(self.load_memory_prompt()?)
            .claude_md(load_claude_md_prompt(workdir))
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

fn load_claude_md_prompt(workdir: &Path) -> String {
    let mut sources = Vec::new();

    let user_claude = crate::consts::TactPath::home_claude_dir().map(|home| home.join("CLAUDE.md"));
    if let Some(path) = user_claude
        && let Ok(content) = std::fs::read_to_string(&path)
    {
        sources.push((
            "user global (~/.claude/CLAUDE.md)".to_string(),
            content.trim().to_string(),
        ));
    }

    let project_claude = workdir.join("CLAUDE.md");
    if let Ok(content) = std::fs::read_to_string(&project_claude) {
        sources.push((
            "project root (CLAUDE.md)".to_string(),
            content.trim().to_string(),
        ));
    }

    if let Ok(cwd) = std::env::current_dir()
        && cwd != workdir
    {
        let subdir_claude = cwd.join("CLAUDE.md");
        if let Ok(content) = std::fs::read_to_string(&subdir_claude) {
            sources.push((
                format!("subdir ({}/CLAUDE.md)", cwd.display()),
                content.trim().to_string(),
            ));
        }
    }

    if sources.is_empty() {
        return String::new();
    }

    let mut lines = vec!["# CLAUDE.md instructions".to_string(), String::new()];
    for (label, content) in sources {
        lines.push(format!("## From {}", label));
        lines.push(String::new());
        lines.push(content);
        lines.push(String::new());
    }
    lines.join("\n").trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use anthropic_ai_sdk::types::message::{ContentBlock, Message, Role, StopReason};
    use std::sync::Once;
    use tact_llm::{LlmProvider, MockClient};

    use crate::tool::test_support::test_context;

    static INIT_CONFIG: Once = Once::new();

    fn ensure_config() {
        INIT_CONFIG.call_once(|| {
            let config = crate::config::ResolvedConfig {
                llm: crate::config::LlmSettings {
                    provider: "mock".to_string(),
                    api_key: String::new(),
                    base_url: String::new(),
                    model: "mock-model".to_string(),
                },
                agent: crate::config::AgentSettings {
                    context_limit_chars: 500_000,
                    max_tokens: 8192,
                    thinking_budget: 0,
                    snapshot_max_items: 80,
                    notifications_enabled: false,
                    micro_compact_enabled: true,
                },
                ui: crate::config::UiSettings {
                    theme: "retro".to_string(),
                },
                tools: crate::config::ToolSettings {
                    brave_search_api_key: None,
                },
                permission_mode: None,
                tokio_console: false,
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
            context_limit_chars: 500,
            max_tokens: 1024,
            thinking_budget: 0,
            snapshot_max_items: 10,
            notifications_enabled: false,
            micro_compact_enabled: true,
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
            big.agent.context_limit_chars = 900_000;
            crate::config::install_or_override(big);
        }

        assert_eq!(agent.context_limit(), 500);
        assert_eq!(agent.max_tokens(), 1024);
        assert_eq!(
            agent.agent_settings.context_limit_chars,
            tiny.context_limit_chars
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
                AgentUpdate::StepFinished(_, id, r) if r.tool == "read_file" => Some(id.as_str()),
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
                    AgentUpdate::StepFailed(_, id, msg)
                        if id == "w1" && msg.contains("Plan mode")
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
                    AgentUpdate::TokenUsage { total, .. } if *total == usage.total
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

        let read_done = updates
            .iter()
            .position(|u| matches!(u, AgentUpdate::StepFinished(_, id, _) if id == "r1"));
        let write_done = updates
            .iter()
            .position(|u| matches!(u, AgentUpdate::StepFinished(_, id, _) if id == "w1"));
        assert!(
            read_done.is_some() && write_done.is_some() && read_done < write_done,
            "read must finish before write on same file, got: {updates:?}"
        );
        assert_eq!(
            std::fs::read_to_string(work_dir.join("shared.txt")).unwrap(),
            "next"
        );
    }
}
