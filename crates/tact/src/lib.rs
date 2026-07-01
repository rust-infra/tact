//! Tact — the agent runtime crate.
//!
//! This crate implements the core agent loop: it manages conversation context,
//! dispatches tool calls (native and MCP), enforces permission policies,
//! handles context compaction, and integrates with the TUI frontend for
//! streaming output and user interaction.
//!
//! # Key concepts
//!
//! - [`Agent`] owns the message history, tool router, MCP router, and hooks.
//! - [`AgentRuntime`] carries the Anthropic client, context window state, and
//!   recovery/permission state.
//! - [`agent_loop`] is the main conversation loop: it sends messages to the LLM,
//!   processes tool-use blocks, applies permissions, and writes results back.
//! - Module [`tool`] defines the [`Tool`] trait, the [`ToolRouter`], and
//!   registers all built-in tools.
//! - Module [`hook`] provides pre/post tool-use and session-start hooks.
//! - Module [`compact`] handles context compaction and transcript persistence.
//! - Module [`permission`] classifies tool risk and enforces approval policies.
//! - Module [`notifications`] sends macOS desktop notifications for task lifecycle events.

pub mod background;
pub mod compact;
pub mod config;
pub mod consts;
pub mod cron;
pub mod hook;
pub mod lsp;
pub mod mcp;
pub mod memory;
pub mod notifications;
pub mod permission;
pub mod prompt;
pub mod recovery;
pub mod skill;
pub mod stats;
pub mod store;
pub mod task;
pub mod team;
pub mod tool;
mod tool_schedule;
pub mod worktree;
pub use anthropic_ai_sdk::types::message::Tool as ToolSpec;

use anthropic_ai_sdk::types::message::{
    ContentBlock, CreateMessageParams, Message, MessageContent, RequiredMessageParams, Role,
    StopReason, Thinking, ThinkingType,
};
use anyhow::{Context, Result};
use chrono::Utc;
use futures_util::{StreamExt, stream::FuturesUnordered};
use std::path::Path;

use crate::compact::{
    CompactState, compacted_context, estimate_context_size, micro_compact, persist_large_output,
    write_transcript,
};
use crate::hook::{
    Hook, HookControl, HookTypes, PostToolUseFn, PreToolUseFn, SessionStartFn, ToolResult, ToolUse,
};
use crate::mcp::MCPToolRouter;
use crate::memory::MEMORY_GUIDANCE;
use crate::permission::{PermissionBehavior, PermissionManager};
use crate::prompt::SystemPrompt;
use crate::recovery::{
    CONTINUATION_MESSAGE, MAX_RECOVERY_ATTEMPTS, RecoveryState, backoff_delay,
    is_prompt_too_long_error, is_transient_transport_error,
};
use crate::stats::SessionStats;
use crate::store::DynSessionStore;
use crate::tool::{ToolContext, ToolRouter};
use tact_llm::{LlmClient, LlmProvider};
use tact_protocol::{AgentUpdate, StepResult, StepStatus, TokenUsageInfo};

/// Soft context limit in characters. When the serialized context exceeds
/// this threshold the agent will attempt micro-compaction.
fn context_limit() -> usize {
    crate::config::settings().agent.context_limit_chars
}

/// Maximum tokens to generate per LLM call.
fn max_tokens() -> u32 {
    crate::config::settings().agent.max_tokens
}

/// Budget tokens for extended thinking.
fn thinking_budget() -> usize {
    crate::config::settings().agent.thinking_budget
}

/// Returns the thinking configuration to use.
fn thinking_config() -> Thinking {
    Thinking {
        budget_tokens: thinking_budget(),
        type_: ThinkingType::Enabled,
    }
}

/// Returns the model name from the active provider's environment variable.
/// Parsed once on first call and cached for the lifetime of the process.
pub fn get_model() -> &'static str {
    tact_llm::get_provider().model.as_str()
}

/// Constructs the active LLM client from the installed configuration.
pub fn get_llm_client() -> anyhow::Result<LlmProvider> {
    tact_llm::get_llm_client()
}

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
                cached_dir_snapshot: None,
            },
            tool_context,
            tools,
            mcp_router,
            hooks: Vec::new(),
            system_prompt,
            tool_use_counter: 0,
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

        store.create_session(&session_id).await?;

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
            let _ = store
                .record_tool_schedule(session_id, self.runtime.last_message_db_id, &json)
                .await;
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
            micro_compact(&mut self.runtime.context);
            if estimate_context_size(&self.runtime.context) > context_limit() {
                self.emit_update(AgentUpdate::Info("[auto compact]".into()));
                self.compact_history(None).await?;
            }

            // Re-render the system prompt each turn so memory/dynamic_context stay fresh.
            // Stable sections are placed before DYNAMIC_BOUNDARY to keep prefix cache-friendly.
            let system = self.build_system_prompt()?;

            let model_name = get_model().to_string();
            let request = CreateMessageParams::new(RequiredMessageParams {
                model: model_name.clone(),
                messages: self.runtime.context.clone(),
                max_tokens: max_tokens(),
            })
            .with_system(&system)
            .with_tools(self.all_tool_specs())
            .with_stream(true)
            .with_thinking(thinking_config());

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
            let _ = self
                .persist_llm_call("stream", token_usage.as_ref(), request_body.as_deref())
                .await;

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

    /// Dispatch the tool calls in one assistant turn.
    ///
    /// Runs in three stages so that independent tools overlap while conflicting
    /// ones stay ordered:
    /// 1. **Pre-flight** (sequential): stats, step events, PreToolUse hooks, and
    ///    permission checks — the latter may prompt the user, so order matters.
    /// 2. **Execution** (parallel by wave): tools touching disjoint resources
    ///    run concurrently; a read/write or write/write on the same file (and
    ///    any unscoped "barrier" tool such as `bash`/MCP) is serialised. See
    ///    [`tool_schedule`].
    /// 3. **Post-processing** (sequential): PostToolUse hooks, step-finished
    ///    events, and bookkeeping, replayed in the model's original tool order.
    pub async fn execute_tool_call(
        &mut self,
        content: &[ContentBlock],
    ) -> Result<(Vec<ContentBlock>, Option<String>)> {
        // ── Phase 1: sequential pre-flight ──────────────────────────────────
        let mut prepared: Vec<PreparedTool> = Vec::new();
        for block in content {
            let ContentBlock::ToolUse { id, name, input } = block else {
                continue;
            };
            *self
                .runtime
                .stats
                .tool_counts
                .entry(name.clone())
                .or_insert(0) += 1;
            if self
                .runtime
                .cancel_flag
                .load(std::sync::atomic::Ordering::Relaxed)
            {
                self.emit_update(AgentUpdate::Info("Cancelled by user".into()));
                return Ok((vec![], None));
            }

            let step_idx = self.next_step_idx();
            let arg_summary = tool_arg_summary(name, input);
            let step_description = if arg_summary.is_empty() {
                name.clone()
            } else {
                format!("{name} ({arg_summary})")
            };
            self.emit_update(AgentUpdate::StepAdded(tact_protocol::PlanStep {
                description: step_description,
                tool: name.clone(),
                tool_id: id.clone(),
                args: tool_args_map(input),
                need_approval: false,
                output: None,
            }));
            self.emit_update(AgentUpdate::StepStarted(
                step_idx,
                id.clone(),
                name.clone(),
                arg_summary,
            ));

            let mut tool_use = ToolUse {
                id: id.clone(),
                name: name.clone(),
                input: input.clone(),
            };
            let mut permission_label: Option<String> = None;
            let state = match invoke_hooks!(PreToolUse, self, &mut tool_use) {
                Ok(HookControl::Continue) => {
                    let decision = self
                        .runtime
                        .permission_manager
                        .check(&tool_use.name, &tool_use.input);
                    match decision.behavior {
                        PermissionBehavior::Allow => PreparedState::Run,
                        PermissionBehavior::Deny => {
                            let msg = format!("Permission denied: {}", decision.reason);
                            self.emit_update(AgentUpdate::StepFailed(
                                step_idx,
                                id.clone(),
                                msg.clone(),
                            ));
                            return Ok((
                                vec![ContentBlock::ToolResult {
                                    tool_use_id: id.clone(),
                                    content: msg,
                                }],
                                None,
                            ));
                        }
                        PermissionBehavior::Ask => {
                            let choice = if let Some(tx) = &self.runtime.ui_tx {
                                let (respond_tx, respond_rx) = tokio::sync::oneshot::channel();
                                let input_preview = tool_use
                                    .input
                                    .to_string()
                                    .chars()
                                    .take(80)
                                    .collect::<String>();
                                let prompt = format!("Allow {}: {}", tool_use.name, input_preview);
                                let options = vec![
                                    "Allow once".to_string(),
                                    "Deny".to_string(),
                                    "Always allow this tool".to_string(),
                                ];
                                let _ = tx.send(AgentUpdate::RequestSelect {
                                    prompt,
                                    options,
                                    respond: respond_tx,
                                });
                                match respond_rx.await {
                                    Ok(Some(0)) => Some("allow_once"),
                                    Ok(Some(2)) => Some("always_allow"),
                                    _ => Some("deny"),
                                }
                            } else {
                                let choice = self
                                    .runtime
                                    .permission_manager
                                    .ask_user(&tool_use.name, &tool_use.input)?;
                                if choice {
                                    Some("allow_once")
                                } else {
                                    Some("deny")
                                }
                            };
                            match choice {
                                Some("allow_once") => {
                                    permission_label = Some("Allow once".to_string());
                                    PreparedState::Run
                                }
                                Some("always_allow") => {
                                    permission_label = Some("Always allow this tool".to_string());
                                    self.runtime.permission_manager.allow_tool(&tool_use.name);
                                    PreparedState::Run
                                }
                                _ => {
                                    let msg =
                                        format!("Permission denied by user for {}", tool_use.name);
                                    self.emit_update(AgentUpdate::StepFailed(
                                        step_idx,
                                        id.clone(),
                                        msg.clone(),
                                    ));
                                    return Ok((
                                        vec![ContentBlock::ToolResult {
                                            tool_use_id: id.clone(),
                                            content: msg,
                                        }],
                                        None,
                                    ));
                                }
                            }
                        }
                    }
                }
                Ok(HookControl::Block(reason)) => {
                    let msg = format!("Tool blocked by PreToolUse hook: {reason}");
                    self.emit_update(AgentUpdate::StepFailed(step_idx, id.clone(), msg.clone()));
                    PreparedState::Resolved(msg)
                }
                Err(error) => {
                    let msg = format!("PreToolUse hook failed: {error}");
                    self.emit_update(AgentUpdate::StepFailed(step_idx, id.clone(), msg.clone()));
                    PreparedState::Resolved(msg)
                }
            };

            prepared.push(PreparedTool {
                id: tool_use.id,
                name: tool_use.name,
                input: tool_use.input,
                step_idx,
                permission_label,
                state,
            });
        }

        // ── Phase 2: execute cleared tools in conflict-free waves ───────────
        let run_indices: Vec<usize> = prepared
            .iter()
            .enumerate()
            .filter(|(_, p)| matches!(p.state, PreparedState::Run))
            .map(|(i, _)| i)
            .collect();
        let resources: Vec<tool_schedule::ToolResources> = run_indices
            .iter()
            .map(|&i| {
                tool_schedule::tool_resources(
                    &prepared[i].name,
                    &prepared[i].input,
                    &self.tool_context.work_dir,
                )
            })
            .collect();

        // Record how this turn's tools were scheduled, linked to the same LLM
        // call as the token usage, so the parallelism can be audited later.
        if !run_indices.is_empty() {
            let names: Vec<String> = run_indices
                .iter()
                .map(|&i| prepared[i].name.clone())
                .collect();
            self.persist_tool_schedule(&tool_schedule::summarize(&names, &resources))
                .await;
        }

        // Final tool outputs keyed by index into `prepared`. We still collect
        // them for deterministic tool_result ordering, but StepFinished is now
        // emitted immediately when each tool completes (instead of after a
        // whole wave joins), so parallel progress is visible in the UI.
        let mut outputs: Vec<Option<String>> = (0..prepared.len()).map(|_| None).collect();
        let mut manual_compact = None;

        for wave in tool_schedule::waves_grouped(&resources) {
            if self
                .runtime
                .cancel_flag
                .load(std::sync::atomic::Ordering::Relaxed)
            {
                self.emit_update(AgentUpdate::Info("Cancelled by user".into()));
                return Ok((vec![], None));
            }

            // A barrier wave always holds a single tool. MCP tools need the
            // stateful router (`&mut self`), so run those sequentially; every
            // other wave runs concurrently over shared borrows.
            if wave.len() == 1 && MCPToolRouter::is_mcp_tool(&prepared[run_indices[wave[0]]].name) {
                let pi = run_indices[wave[0]];
                let start = std::time::Instant::now();
                let exec = self
                    .execute_mcp(&prepared[pi].name, &prepared[pi].input)
                    .await;
                let duration_us = start.elapsed().as_micros() as u64;
                let prep_id = prepared[pi].id.clone();
                let prep_name = prepared[pi].name.clone();
                let prep_input = prepared[pi].input.clone();
                let prep_step_idx = prepared[pi].step_idx;
                let prep_permission_label = prepared[pi].permission_label.clone();

                let tool_use = ToolUse {
                    id: prep_id.clone(),
                    name: prep_name.clone(),
                    input: prep_input.clone(),
                };
                let mut tool_result = ToolResult {
                    tool_use_id: prep_id.clone(),
                    content: exec.content,
                };
                let (exec_output, final_status) =
                    match invoke_hooks!(PostToolUse, self, &tool_use, &mut tool_result) {
                        Ok(HookControl::Continue) => (tool_result.content, exec.status),
                        Ok(HookControl::Block(reason)) => (
                            format!("Tool blocked by PostToolUse hook: {reason}"),
                            StepStatus::Failed,
                        ),
                        Err(error) => (
                            format!("PostToolUse hook failed: {error}"),
                            StepStatus::Failed,
                        ),
                    };
                self.runtime
                    .stats
                    .tool_durations_ms
                    .push(duration_us / 1000);
                let summary = exec_output.chars().take(200).collect::<String>();
                let arg_summary = tool_arg_summary(&prep_name, &prep_input);
                let arg_full = tool_arg_full(&prep_name, &prep_input);
                let detail = tool_detail_content(&prep_name, &prep_input, &exec_output);
                self.emit_update(AgentUpdate::StepFinished(
                    prep_step_idx,
                    prep_id,
                    StepResult {
                        tool: prep_name.clone(),
                        arg_summary,
                        arg_full: Some(arg_full),
                        status: final_status,
                        message: summary,
                        detail,
                        duration_us: Some(duration_us),
                        permission_label: prep_permission_label,
                    },
                ));
                if prep_name == "read_file"
                    && let Some(path) = prep_input.get("path").and_then(|value| value.as_str())
                {
                    self.remember_recent_file(path);
                }
                if prep_name == "compact" {
                    manual_compact = prep_input
                        .get("focus")
                        .and_then(|value| value.as_str())
                        .map(ToOwned::to_owned)
                        .or_else(|| Some(String::new()));
                }
                outputs[pi] = Some(exec_output);
                continue;
            }

            let mut futures = FuturesUnordered::new();
            for &pos in &wave {
                let pi = run_indices[pos];
                let tools = &self.tools;
                let ctx = &self.tool_context;
                let prep = &prepared[pi];
                futures.push(async move {
                    let start = std::time::Instant::now();
                    let exec = run_native_tool(tools, ctx, &prep.id, &prep.name, &prep.input).await;
                    (
                        pi,
                        exec.content,
                        exec.status,
                        start.elapsed().as_micros() as u64,
                    )
                });
            }
            let mut pending_durations_us: Vec<u64> = Vec::new();
            let mut pending_recent_files: Vec<String> = Vec::new();
            while let Some((pi, content, exec_status, duration_us)) = futures.next().await {
                let prep_id = prepared[pi].id.clone();
                let prep_name = prepared[pi].name.clone();
                let prep_input = prepared[pi].input.clone();
                let prep_step_idx = prepared[pi].step_idx;
                let prep_permission_label = prepared[pi].permission_label.clone();

                let tool_use = ToolUse {
                    id: prep_id.clone(),
                    name: prep_name.clone(),
                    input: prep_input.clone(),
                };
                let mut tool_result = ToolResult {
                    tool_use_id: prep_id.clone(),
                    content,
                };
                let (exec_output, final_status) =
                    match invoke_hooks!(PostToolUse, self, &tool_use, &mut tool_result) {
                        Ok(HookControl::Continue) => (tool_result.content, exec_status),
                        Ok(HookControl::Block(reason)) => (
                            format!("Tool blocked by PostToolUse hook: {reason}"),
                            StepStatus::Failed,
                        ),
                        Err(error) => (
                            format!("PostToolUse hook failed: {error}"),
                            StepStatus::Failed,
                        ),
                    };
                pending_durations_us.push(duration_us);
                let summary = exec_output.chars().take(200).collect::<String>();
                let arg_summary = tool_arg_summary(&prep_name, &prep_input);
                let arg_full = tool_arg_full(&prep_name, &prep_input);
                let detail = tool_detail_content(&prep_name, &prep_input, &exec_output);
                self.emit_update(AgentUpdate::StepFinished(
                    prep_step_idx,
                    prep_id,
                    StepResult {
                        tool: prep_name.clone(),
                        arg_summary,
                        arg_full: Some(arg_full),
                        status: final_status,
                        message: summary,
                        detail,
                        duration_us: Some(duration_us),
                        permission_label: prep_permission_label,
                    },
                ));
                if prep_name == "read_file"
                    && let Some(path) = prep_input.get("path").and_then(|value| value.as_str())
                {
                    pending_recent_files.push(path.to_string());
                }
                if prep_name == "compact" {
                    manual_compact = prep_input
                        .get("focus")
                        .and_then(|value| value.as_str())
                        .map(ToOwned::to_owned)
                        .or_else(|| Some(String::new()));
                }
                outputs[pi] = Some(exec_output);
            }
            drop(futures);
            for duration_us in pending_durations_us {
                self.runtime
                    .stats
                    .tool_durations_ms
                    .push(duration_us / 1000);
            }
            for path in pending_recent_files {
                self.remember_recent_file(&path);
            }
        }

        // ── Phase 3: build tool_result blocks in deterministic order ─────────
        let mut result = Vec::new();
        for (idx, prep) in prepared.into_iter().enumerate() {
            let output = match prep.state {
                PreparedState::Resolved(msg) => msg,
                PreparedState::Run => outputs[idx]
                    .take()
                    .expect("cleared tool must have produced output"),
            };
            result.push(ContentBlock::ToolResult {
                tool_use_id: prep.id,
                content: output,
            });
        }
        Ok((result, manual_compact))
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
        self.tools
            .tool_specs()
            .into_iter()
            .chain(self.mcp_router.all_tools())
            .collect()
    }

    /// Run a single MCP tool. The MCP router is stateful (`&mut self`), so MCP
    /// tools are always scheduled alone in their own wave and invoked here.
    async fn execute_mcp(&mut self, name: &str, input: &serde_json::Value) -> ExecResult {
        match self.mcp_router.call(name, input.clone()).await {
            Ok(output) => ExecResult {
                content: output,
                status: StepStatus::Success,
            },
            Err(e) => ExecResult {
                content: format!("Error invoking MCP tool {}: {}", name, e),
                status: StepStatus::Failed,
            },
        }
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

        let model_name = get_model().to_string();
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
            .dynamic_context(load_dynamic_context(workdir, &mut self.runtime.cached_dir_snapshot))
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

/// A tool call after phase-1 pre-flight in [`Agent::execute_tool_call`].
///
/// Carries everything phases 2 and 3 need so the actual tool work can be
/// scheduled and run independently of the `&mut self` framework around it.
struct PreparedTool {
    id: String,
    name: String,
    input: serde_json::Value,
    step_idx: usize,
    permission_label: Option<String>,
    state: PreparedState,
}

enum PreparedState {
    /// Cleared to execute in phase 2.
    Run,
    /// Pre-flight already produced the final output (blocked by a PreToolUse
    /// hook); skip execution and surface this text as the tool result.
    Resolved(String),
}

/// Run a single native (non-MCP) tool, borrowing only the shared router and
/// context so calls in the same wave can run concurrently.
async fn run_native_tool(
    tools: &ToolRouter,
    ctx: &ToolContext,
    tool_use_id: &str,
    name: &str,
    input: &serde_json::Value,
) -> ExecResult {
    match tools.call(ctx, name, input.clone()).await {
        Ok(output) => {
            let content = if name == "bash" {
                let tact_path = crate::consts::TactPath::new(&ctx.work_dir);
                persist_large_output(&tact_path, tool_use_id, &output)
                    .unwrap_or_else(|e| format!("Error persisting large output: {}", e))
            } else {
                output
            };
            ExecResult {
                content,
                status: StepStatus::Success,
            }
        }
        Err(e) => ExecResult {
            content: format!("Error invoking tool {}: {}", name, e),
            status: StepStatus::Failed,
        },
    }
}

struct ExecResult {
    content: String,
    status: StepStatus,
}

const MAX_TOOL_ARG_SUMMARY_CHARS: usize = 120;

fn truncate_tool_arg_summary(s: &str) -> String {
    if s.chars().count() <= MAX_TOOL_ARG_SUMMARY_CHARS {
        return s.to_string();
    }
    format!(
        "{}...",
        s.chars()
            .take(MAX_TOOL_ARG_SUMMARY_CHARS.saturating_sub(3))
            .collect::<String>()
    )
}

fn tool_arg_summary(name: &str, input: &serde_json::Value) -> String {
    let raw = tool_arg_full(name, input);
    truncate_tool_arg_summary(&raw)
}

fn tool_arg_full(name: &str, input: &serde_json::Value) -> String {
    match name {
        "read_file" | "write_file" => input
            .get("path")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        "run_command" | "bash" | "shell" => input
            .get("command")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        _ => input.to_string(),
    }
}

fn tool_args_map(input: &serde_json::Value) -> std::collections::HashMap<String, String> {
    input
        .as_object()
        .map(|obj| {
            obj.iter()
                .map(|(k, v)| {
                    let val = v.as_str().map_or_else(|| v.to_string(), |s| s.to_string());
                    (k.clone(), val)
                })
                .collect()
        })
        .unwrap_or_default()
}

fn tool_detail_content(name: &str, input: &serde_json::Value, exec_output: &str) -> Option<String> {
    match name {
        "read_file" | "run_command" | "bash" | "shell" => Some(exec_output.to_string()),
        "write_file" => input
            .get("content")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::tool_arg_summary;

    #[test]
    fn long_bash_summary_is_truncated() {
        let command = "x".repeat(200);
        let input = serde_json::json!({ "command": command });
        let summary = tool_arg_summary("bash", &input);
        assert_eq!(summary.chars().count(), 120);
        assert!(summary.ends_with("..."));
    }

    #[test]
    fn short_bash_summary_is_preserved() {
        let input = serde_json::json!({ "command": "git status --short" });
        let summary = tool_arg_summary("bash", &input);
        assert_eq!(summary, "git status --short");
    }
}

pub type LoopState = Agent;

/// Extracts plain text from a [`MessageContent`] block.
///
/// For `Text` content returns the string directly; for `Blocks` content
/// joins all text blocks with newlines.
pub fn extract_text(content: &MessageContent) -> String {
    match content {
        MessageContent::Text { content } => content.clone(),
        MessageContent::Blocks { content } => content
            .iter()
            .filter_map(|block| {
                if let ContentBlock::Text { text } = block {
                    Some(text.as_str())
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .join("\n"),
    }
}

/// Build the dynamic-context block that appears after `=== DYNAMIC_BOUNDARY ===`.
///
/// The directory snapshot is expensive to compute and its output must be
/// byte-for-byte identical across requests so that DeepSeek's prefix KV-cache
/// can hit.  We compute it once per session and reuse the cached string.
fn load_dynamic_context(workdir: &Path, cached_snapshot: &mut Option<String>) -> String {
    let snapshot_limit = crate::config::settings().agent.snapshot_max_items;

    let tree = match cached_snapshot {
        Some(cached) => cached.clone(),
        None => {
            let snap = snapshot_dir(workdir, snapshot_limit);
            *cached_snapshot = snap.clone();
            snap.unwrap_or_default()
        }
    };

    let mut lines = vec![
        // "# Dynamic context".to_string(),
        format!("Current date: {}", Utc::now().date_naive()),
        format!("Working directory: {}", workdir.display()),
        format!("Model: {}", get_model()),
        format!("Platform: {}", std::env::consts::OS),
    ];

    if !tree.is_empty() {
        lines.push(String::new());
        lines.push(tree);
    }

    lines.join("\n")
}

/// Generate a lightweight directory-only snapshot of the given workspace.
///
/// Ignores common large/binary directories (`target`, `node_modules`, `.git`,
/// etc.).  Collects **directories only** first, sorts by path, *then*
/// truncates to `max_items` so the output is deterministic regardless of
/// filesystem readdir order.  Returns `None` when the directory cannot be read.
fn snapshot_dir(root: &Path, max_items: usize) -> Option<String> {
    const IGNORE_DIRS: &[&str] = &[
        // ---- VCS ----
        ".git",
        ".hg",
        ".svn",
        // ---- Rust ----
        "target",
        // ---- C / C++ / general build outputs ----
        "build",
        "cmake-build-debug",
        "cmake-build-release",
        "obj",
        "out",
        // ---- Node.js / TypeScript / frontend ----
        "node_modules",
        ".next",
        ".nuxt",
        ".output",
        ".turbo",
        ".cache",
        ".parcel-cache",
        "coverage",
        ".nyc_output",
        // ---- Python ----
        ".venv",
        "venv",
        ".tox",
        "__pycache__",
        ".mypy_cache",
        ".pytest_cache",
        ".ruff_cache",
        ".eggs",
        // ---- Go ----
        "vendor",
        // ---- Java / Kotlin / Scala ----
        ".gradle",
        ".bloop",
        ".metals",
        ".bsp",
        // ---- .NET / C# ----
        "bin",
        "obj",
        "packages",
        // ---- Ruby ----
        ".bundle",
        // ---- Elixir ----
        "_build",
        "deps",
        ".elixir_ls",
        // ---- Haskell ----
        ".stack-work",
        "dist-newstyle",
        // ---- Dart / Flutter ----
        ".dart_tool",
        // ---- Swift ----
        ".build",
        // ---- IDE / editors ----
        ".idea",
        ".fleet",
        ".devcontainer",
        // ---- cross-ecosystem build artifacts ----
        "dist",
        // ---- macOS ----
        ".DS_Store",
    ];

    use std::cmp::Ordering;
    use std::collections::BTreeMap;

    // Phase 1 — collect visible directories only.
    // IMPORTANT: ignored directories must be pruned at traversal time,
    // otherwise `continue` only skips the directory node itself while still
    // walking its children.
    let mut items: Vec<std::path::PathBuf> = Vec::new();

    let should_keep = |entry: &walkdir::DirEntry| {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            return true;
        };
        !(IGNORE_DIRS.contains(&name)
            || (name.starts_with('.')
                && !name.eq_ignore_ascii_case(".gitignore")
                && !name.eq_ignore_ascii_case(".env.example")))
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
            // Skip workspace root itself.
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

    // Phase 2 — deterministic sort: shallow paths first, then lexical order.
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

    // Phase 3 — group by parent directory.
    let mut dirs: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for rel in &items {
        let parent = rel
            .parent()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| ".".to_string());
        let name = rel.file_name().and_then(|n| n.to_str()).unwrap_or("?");
        dirs.entry(parent)
            .or_default()
            .push(format!("{name}/"));
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
