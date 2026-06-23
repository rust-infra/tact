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
pub mod notifications;
pub mod hook;
pub mod llm;
pub mod lsp;
pub mod mcp;
pub mod memory;
pub mod permission;
pub mod prompt;
pub mod recovery;
pub mod session_store;
pub mod skill;
pub mod stats;
pub mod store;
pub mod task;
pub mod team;
pub mod tool;
pub mod worktree;
pub use anthropic_ai_sdk::types::message::Tool as ToolSpec;

use crate::llm::{LlmClient, LlmProvider};
use anthropic_ai_sdk::types::message::{
    ContentBlock, CreateMessageParams, Message, MessageContent,
    RequiredMessageParams, Role, StopReason, Thinking, ThinkingType,
};
use anyhow::{Context, Result};
use chrono::Utc;
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
use crate::session_store::DynSessionStore;
use crate::stats::SessionStats;
use crate::tool::{ToolContext, ToolRouter};
use tact_core::{AgentUpdate, StepResult, StepStatus, TokenUsageInfo};

/// Soft context limit in characters. When the serialized context exceeds
/// this threshold the agent will attempt micro-compaction.
///
/// Defaults to 500_000 (~125K tokens), raised to 900_000 for Kimi K2.x which
/// has a 256K-token context window. Override with `TACT_CONTEXT_LIMIT_CHARS`.
fn context_limit() -> usize {
    std::env::var("TACT_CONTEXT_LIMIT_CHARS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(|| {
            if crate::llm::is_kimi_k2x() {
                900_000
            } else {
                500_000
            }
        })
}

/// Maximum tokens to generate per LLM call.
/// Defaults to 8000, raised to 32000 for Kimi K2.x thinking models because
/// they emit both reasoning_content and content. Override with `TACT_MAX_TOKENS`.
fn max_tokens() -> u32 {
    std::env::var("TACT_MAX_TOKENS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(|| {
            if crate::llm::is_kimi_k2x() {
                32_000
            } else {
                8_000
            }
        })
}

/// Budget tokens for extended thinking.
/// Defaults to 32000. Override with `TACT_THINKING_BUDGET`.
fn thinking_budget() -> usize {
    std::env::var("TACT_THINKING_BUDGET")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(32000)
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
    llm::get_provider().model.as_str()
}

/// Constructs the active LLM client from environment variables.
pub fn get_llm_client() -> anyhow::Result<LlmProvider> {
    llm::get_llm_client()
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
            AgentUpdate::StepFailed(idx, msg) => {
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

        store.create_session(&session_id, None).await?;

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

    async fn persist_message(
        &mut self,
        role: Role,
        content: &MessageContent,
    ) -> Result<()> {
        let Some(store) = self.runtime.session_store.as_ref() else {
            return Ok(());
        };
        let Some(session_id) = self.runtime.session_id.as_ref() else {
            return Ok(());
        };
        let ordinal = self.runtime.context.len() as i64;
        let db_id = store.append_message(session_id, role, content, ordinal).await?;
        if self.runtime.first_message_db_id == 0 {
            self.runtime.first_message_db_id = db_id;
        }
        self.runtime.last_message_db_id = db_id;
        Ok(())
    }

    /// Persist token usage (cache hit/miss, reasoning) to sqlite.
    /// Links to the message range that was sent ([first_message_db_id .. last_message_db_id]).
    async fn persist_token_usage(&self, call_type: &str, usage: &TokenUsageInfo) -> Result<()> {
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
            )
            .await?;
        Ok(())
    }

    /// Set or update the session title (displayed in --list-sessions).
    pub async fn set_session_title(&mut self, title: Option<&str>) -> Result<()> {
        let Some(session_id) = self.runtime.session_id.as_ref() else {
            return Ok(());
        };
        if let Some(store) = self.runtime.session_store.as_ref() {
            store.update_session_title(session_id, title).await?;
        }
        Ok(())
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
    pub async fn agent_loop(&mut self,
        initial_user_message: Option<Message>,
    ) -> Result<()> {
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

        let system = self.build_system_prompt()?;
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

            self.emit_update(AgentUpdate::ModelInfo(tact_core::ModelCallParams {
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

            let (content, stop_reason, token_usage) = match self.stream_message(&request).await {
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
                let _ = self.persist_token_usage("stream", usage).await;
            }

            self.runtime
                .context
                .push(Message::new_blocks(Role::Assistant, content.clone()));

            // Check whether the truncated response contains pending tool calls.
            // OpenAI requires every assistant message with tool_calls to be
            // immediately followed by tool-result messages for each id.
            let has_pending_tools = content
                .iter()
                .any(|b| matches!(b, ContentBlock::ToolUse { .. }));

            self.persist_message(Role::Assistant, &MessageContent::Blocks { content: content.clone() }).await?;

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
                    self.persist_message(Role::User, &MessageContent::Blocks { content: tool_result }).await?;
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
            self.persist_message(Role::User, &MessageContent::Blocks { content: tool_result }).await?;

            if let Some(focus) = manual_compact {
                self.emit_update(AgentUpdate::Info("[manual compact]".into()));
                self.compact_history(Some(focus.as_str())).await?;
            }
        }
    }

    async fn stream_message(
        &mut self,
        request: &CreateMessageParams,
    ) -> Result<(Vec<ContentBlock>, Option<StopReason>, Option<TokenUsageInfo>), anyhow::Error> {
        let ui_tx = self.runtime.ui_tx.clone();
        self.runtime
            .client
            .stream_message(request, ui_tx)
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))
    }

    pub async fn execute_tool_call(
        &mut self,
        content: &[ContentBlock],
    ) -> Result<(Vec<ContentBlock>, Option<String>)> {
        let mut result = Vec::new();
        let mut manual_compact = None;
        for block in content {
            if let ContentBlock::ToolUse { id, name, input } = block {
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
                let description = format!(
                    "{}: {}",
                    name,
                    input.to_string().chars().take(100).collect::<String>()
                );
                self.emit_update(AgentUpdate::StepAdded(tact_core::PlanStep {
                    description: description.clone(),
                    tool: name.clone(),
                    args: std::collections::HashMap::new(),
                    need_approval: false,
                    output: None,
                }));
                self.emit_update(AgentUpdate::StepStarted(step_idx));

                let mut tool_use = ToolUse {
                    id: id.clone(),
                    name: name.clone(),
                    input: input.clone(),
                };
                let output = match invoke_hooks!(PreToolUse, self, &mut tool_use) {
                    Ok(HookControl::Continue) => {
                        let decision = self
                            .runtime
                            .permission_manager
                            .check(&tool_use.name, &tool_use.input);
                        match decision.behavior {
                            PermissionBehavior::Allow => {}
                            PermissionBehavior::Deny => {
                                let msg = format!("Permission denied: {}", decision.reason);
                                self.emit_update(AgentUpdate::StepFailed(step_idx, msg.clone()));
                                return Ok((
                                    vec![ContentBlock::ToolResult {
                                        tool_use_id: id.clone(),
                                        content: msg,
                                    }],
                                    manual_compact,
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
                                    let prompt =
                                        format!("Allow {}: {}", tool_use.name, input_preview);
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
                                    Some("allow_once") => {}
                                    Some("always_allow") => {
                                        self.runtime.permission_manager.allow_tool(&tool_use.name);
                                    }
                                    _ => {
                                        let msg = format!(
                                            "Permission denied by user for {}",
                                            tool_use.name
                                        );
                                        self.emit_update(AgentUpdate::StepFailed(
                                            step_idx,
                                            msg.clone(),
                                        ));
                                        return Ok((
                                            vec![ContentBlock::ToolResult {
                                                tool_use_id: id.clone(),
                                                content: msg,
                                            }],
                                            manual_compact,
                                        ));
                                    }
                                }
                            }
                        }
                        let start = std::time::Instant::now();
                        let mut result = ToolResult {
                            tool_use_id: tool_use.id.clone(),
                            content: self
                                .execute(&tool_use.id, &tool_use.name, &tool_use.input)
                                .await,
                        };
                        let exec_output =
                            match invoke_hooks!(PostToolUse, self, &tool_use, &mut result) {
                                Ok(HookControl::Continue) => result.content,
                                Ok(HookControl::Block(reason)) => {
                                    format!("Tool blocked by PostToolUse hook: {reason}")
                                }
                                Err(error) => format!("PostToolUse hook failed: {error}"),
                            };
                        let summary = exec_output.chars().take(200).collect::<String>();
                        let arg_summary = match tool_use.name.as_str() {
                            "read_file" | "write_file" => tool_use
                                .input
                                .get("path")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string(),
                            "run_command" => tool_use
                                .input
                                .get("command")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string(),
                            _ => {
                                let input_str = tool_use.input.to_string();
                                let char_count = input_str.chars().count();
                                if char_count > 40 {
                                    format!("{}...", input_str.chars().take(37).collect::<String>())
                                } else {
                                    input_str
                                }
                            }
                        };
                        let detail = match tool_use.name.as_str() {
                            "read_file" | "run_command" => Some(exec_output.clone()),
                            "write_file" => tool_use
                                .input
                                .get("content")
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string()),
                            _ => None,
                        };
                        let duration_ms = start.elapsed().as_millis() as u64;
                        self.runtime.stats.tool_durations_ms.push(duration_ms);
                        let step_result = StepResult {
                            tool: tool_use.name.clone(),
                            arg_summary,
                            status: StepStatus::Success,
                            message: summary,
                            detail,
                            duration_ms: Some(duration_ms),
                        };
                        self.emit_update(AgentUpdate::StepFinished(step_idx, step_result));
                        exec_output
                    }
                    Ok(HookControl::Block(reason)) => {
                        let msg = format!("Tool blocked by PreToolUse hook: {reason}");
                        self.emit_update(AgentUpdate::StepFailed(step_idx, msg.clone()));
                        msg
                    }
                    Err(error) => {
                        let msg = format!("PreToolUse hook failed: {error}");
                        self.emit_update(AgentUpdate::StepFailed(step_idx, msg.clone()));
                        msg
                    }
                };
                result.push(ContentBlock::ToolResult {
                    tool_use_id: id.clone(),
                    content: output,
                });
                if tool_use.name == "read_file"
                    && let Some(path) = tool_use.input.get("path").and_then(|value| value.as_str())
                {
                    self.remember_recent_file(path);
                }
                if tool_use.name == "compact" {
                    manual_compact = tool_use
                        .input
                        .get("focus")
                        .and_then(|value| value.as_str())
                        .map(ToOwned::to_owned)
                        .or_else(|| Some(String::new()));
                }
            }
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

    async fn execute(
        &mut self,
        tool_use_id: &str,
        name: &str,
        input: &serde_json::Value,
    ) -> String {
        if MCPToolRouter::is_mcp_tool(name) {
            return match self.mcp_router.call(name, input.clone()).await {
                Ok(output) => {
                    self.emit_update(AgentUpdate::Info(format!(
                        "MCP tool:{}\n arg:{}\n output:\n{}\n",
                        name,
                        input,
                        output.chars().take(200).collect::<String>()
                    )));
                    output
                }
                Err(e) => {
                    self.emit_update(AgentUpdate::Info(format!(
                        "Error invoking MCP tool {}: {}",
                        name, e
                    )));
                    format!("Error invoking MCP tool {}: {}", name, e)
                }
            };
        }

        match self
            .tools
            .call(&self.tool_context, name, input.clone())
            .await
        {
            Ok(output) => {
                let output = if name == "bash" {
                    let tact_path = crate::consts::TactPath::new(&self.tool_context.work_dir);
                    persist_large_output(&tact_path, tool_use_id, &output).unwrap_or_else(|e| format!("Error persisting large output: {}", e))
                } else {
                    output
                };
                let input = input.to_string().chars().take(30).collect::<String>();
                self.emit_update(AgentUpdate::Info(format!(
                    "Executing {}({})\n",
                    name, input
                )));
                output
            }
            Err(e) => {
                self.emit_update(AgentUpdate::Info(format!(
                    "Error invoking tool {}: {}",
                    name, e
                )));
                format!("Error invoking tool {}: {}", name, e)
            }
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

        self.emit_update(AgentUpdate::ModelInfo(tact_core::ModelCallParams {
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

        let (blocks, _stop_reason, token_usage) = self
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
            let _ = self.persist_token_usage("compact", usage).await;
            // After compaction the context is replaced with a summary, so
            // future messages start a new message-id window.
            self.runtime.first_message_db_id = 0;
            self.runtime.last_message_db_id = 0;
        }
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

    fn build_system_prompt(&self) -> Result<String> {
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
            .dynamic_context(load_dynamic_context(workdir))
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

fn load_dynamic_context(workdir: &Path) -> String {
    let mut lines = vec![
        "# Dynamic context".to_string(),
        format!("Current date: {}", Utc::now().date_naive()),
        format!("Working directory: {}", workdir.display()),
        format!("Model: {}", get_model()),
        format!("Platform: {}", std::env::consts::OS),
    ];

    // Snapshot project directory structure (best-effort; skip if it fails).
    let snapshot_limit = std::env::var("TACT_SNAPSHOT_MAX_ITEMS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(200);
    if let Some(tree) = snapshot_dir(workdir, snapshot_limit) {
        lines.push(String::new());
        lines.push(tree);
    }

    lines.join("\n")
}

/// Generate a lightweight "tree"-style snapshot of the given directory.
///
/// Ignores common large/binary directories and respects `.gitignore`-like
/// paths (`target`, `node_modules`, `.git`, etc.).  Returns `None` when the
/// directory cannot be read.
fn snapshot_dir(root: &Path, max_items: usize) -> Option<String> {
    const IGNORE_DIRS: &[&str] = &[
        ".git",
        "target",
        "node_modules",
        ".venv",
        "venv",
        "__pycache__",
        ".mypy_cache",
        ".pytest_cache",
        ".ruff_cache",
    ];

    use std::collections::BTreeMap;

    // Collect paths first (owned), then group by parent.
    let mut items: Vec<(std::path::PathBuf, bool)> = Vec::new();

    for entry in walkdir::WalkDir::new(root)
        .max_depth(4)
        .follow_links(false)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        if items.len() >= max_items {
            break;
        }
        let path = entry.path();
        if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
            let is_dir = entry.file_type().is_dir();
            if IGNORE_DIRS.contains(&name)
                || (name.starts_with('.')
                    && !name.eq_ignore_ascii_case(".gitignore")
                    && !name.eq_ignore_ascii_case(".env.example"))
            {
                if is_dir {
                    continue;
                }
            }
        }
        let rel = path.strip_prefix(root).ok()?;
        items.push((rel.to_path_buf(), entry.file_type().is_dir()));
    }

    if items.is_empty() {
        return None;
    }

    let mut dirs: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (rel, is_dir) in &items {
        let parent = rel
            .parent()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| ".".to_string());
        let name = rel.file_name().and_then(|n| n.to_str()).unwrap_or("?");
        let display = if *is_dir {
            format!("{}/", name)
        } else {
            name.to_string()
        };
        dirs.entry(parent).or_default().push(display);
    }

    let mut out = vec!["## Project structure".to_string(), String::new()];
    for (dir, mut files) in dirs {
        out.push(dir);
        files.sort();
        for file in files {
            out.push(format!("  {}", file));
        }
    }

    if items.len() >= max_items {
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
