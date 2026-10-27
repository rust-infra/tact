//! Agent runtime: conversation loop, tool dispatch, and session state.

mod outcome;
pub(crate) mod tool_schedule;

pub use outcome::{AgentLoopOutcome, AgentLoopStop};

use std::{
    collections::VecDeque,
    panic::AssertUnwindSafe,
    path::Path,
    sync::{Arc, Mutex, RwLock},
};

use anyhow::{Context, Result};
use chrono::Utc;
use futures_util::FutureExt;
use tact_agent_api::{AgentState, MessageOrigin, ModelCapabilities, RunInput, RunStop, SessionRef};
use tact_agent_core::KernelError;
use tact_llm::{
    LlmProvider, Message, MessageKind, OpenAiReasoningEffort, ProviderConversationState,
    ProviderKind, Role,
};
use tact_protocol::{AgentUpdate, UpdateSink};

use crate::{
    ToolSpec,
    compact::{CompactState, CompactTrigger, approx_text_tokens},
    config::{self, AgentSettings},
    hook::{
        Hook, HookContextChunk, HookControl, HookTypes, InterruptFn, NotificationFn,
        PermissionRequestFn, PostCompactFn, PostToolUseFailureFn, PostToolUseFn, PreCompactFn,
        PreToolUseFn, SessionEndFn, SessionStartContext, SessionStartFn, SessionStartSource,
        StopFn, TaskCompletedFn, UserPromptSubmitFn,
    },
    invoke_hooks,
    mcp::{MCPToolRouter, McpCatalogHandle, McpLoadReport},
    permission::PermissionManager,
    recovery::RecoveryState,
    runtime::{
        activation_view::{ModelIdentity, SessionIdentity, TactActivationView},
        cancel_bridge::CancelBridge,
        compaction_adapter::TactCompactor,
        composition::{TactKernelComposition, TactKernelConfig},
        facade_state::{
            FacadeActivationState, FacadeReceiptTracker, FacadeReceipts, FacadeSidecar,
        },
        permission_authority::PermissionAuthority,
        prompt_adapter::{PromptCaches, SystemPromptAssembler},
        services::TactSharedServices,
    },
    stats::SessionStats,
    store::DynSessionStore,
    subagent::SubagentResult,
    tool::{GoalYieldReason, ToolContext, ToolRouter},
};

#[cfg(test)]
use crate::{
    compact::{estimate_context_tokens, should_auto_compact},
    memory::MEMORY_GUIDANCE,
    prompt::{SystemPrompt, responses_prompt_template},
};

pub(crate) const COMPACT_SUMMARY_MAX_TOKENS: u32 = 2_000;
pub(crate) const COMPACT_SUMMARY_OUTPUT_PERCENT: usize = 20;
pub(crate) const COMPACT_SUMMARY_HEADROOM_PERCENT: usize = 10;
#[cfg(test)]
const RESPONSES_AUTO_COMPACT_THRESHOLD_PERCENT: usize = 80;
pub(crate) const COMPACT_SUMMARY_INSTRUCTIONS: &str = "Summarize this coding-agent conversation so another agent can continue the work.\n\
The conversation appears below as a JSON array of messages; tool results and attachments may be elided or truncated. Ground the summary only in that content and do not invent details.\n\
Preserve:\n\
1. The current goal and what has been accomplished\n\
2. Important findings, decisions, and architectural insights\n\
3. Files read or changed, with key code structures (types, signatures, APIs)\n\
4. Remaining work and next steps\n\
5. User constraints and preferences\n\
6. Any errors encountered and their causes\n\
Output a compact, structured handoff. Keep exact file paths, function names, type signatures, commands, and error text when they matter for continuing; never paraphrase identifiers.";

/// The effort a provider reasons at **by server default** when the request
/// omits `reasoning_effort`.
///
/// DeepSeek and Kimi K3 default thinking ON + effort high server-side and count
/// reasoning tokens inside the same `max_tokens` envelope as the text, so a
/// summary request that omits effort still needs the high bucket as headroom.
/// Every other provider defaults to no thinking at the level this client
/// models, so it needs none.
pub(crate) fn compact_summary_server_default_effort(
    provider_kind: &ProviderKind,
) -> Option<OpenAiReasoningEffort> {
    match provider_kind {
        ProviderKind::DeepSeek | ProviderKind::Kimi => Some(OpenAiReasoningEffort::High),
        _ => None,
    }
}

/// Ceiling for the summarizer's wire `max_tokens` on a given window.
///
/// The envelope may never eat the room the fixed instructions need, so a small
/// window still yields a constructible summary request instead of a hard
/// "window too small" bail.
pub(crate) fn compact_summary_envelope_ceiling(model_context_window: usize) -> u32 {
    if model_context_window == 0 {
        return u32::MAX;
    }
    let headroom = model_context_window
        .saturating_mul(COMPACT_SUMMARY_HEADROOM_PERCENT)
        .div_ceil(100);
    u32::try_from(
        model_context_window
            .saturating_sub(headroom)
            .saturating_sub(approx_text_tokens(COMPACT_SUMMARY_INSTRUCTIONS)),
    )
    .unwrap_or(u32::MAX)
}

/// The effort a provider really runs at when it is handed `effort`.
///
/// Only DeepSeek documents a compatibility fold
/// (`api-docs.deepseek.com/guides/thinking_mode`): `minimal`→`low`,
/// `medium`/`xhigh`→`high`, `max`→`max`, and `none` disables thinking. Tact
/// forwards the configured value verbatim — the provider accepts it — so every
/// *derived* budget has to be sized from what the provider will actually do:
/// otherwise `reasoning_effort = "medium"` on DeepSeek reserves the medium
/// bucket while the model reasons at high, and the envelope starves. Kimi's
/// table (`low`/`high`/`max`) needs no fold; no other kind documents one.
pub(crate) fn effort_after_provider_fold(
    provider_kind: &ProviderKind,
    effort: OpenAiReasoningEffort,
) -> OpenAiReasoningEffort {
    match provider_kind {
        ProviderKind::DeepSeek => match effort {
            OpenAiReasoningEffort::Minimal => OpenAiReasoningEffort::Low,
            OpenAiReasoningEffort::Medium | OpenAiReasoningEffort::Xhigh => {
                OpenAiReasoningEffort::High
            }
            other => other,
        },
        _ => effort,
    }
}

/// Initial reasoning reserve, in tokens, for an explicit effort tier.
///
/// Compaction uses a fixed token bucket per effort rather than a percentage of
/// the summary text budget: the text budget is capped at 2,000 tokens, so a
/// percentage would scale the reasoning headroom down with it even though an
/// effort tier names an absolute thinking allowance, not a fraction of the
/// output.
pub(crate) fn compact_effort_reserve_tokens(effort: OpenAiReasoningEffort) -> u32 {
    match effort {
        OpenAiReasoningEffort::None => 0,
        OpenAiReasoningEffort::Minimal | OpenAiReasoningEffort::Low => 2_000,
        OpenAiReasoningEffort::Medium => 4_000,
        OpenAiReasoningEffort::High => 8_000,
        OpenAiReasoningEffort::Xhigh | OpenAiReasoningEffort::Max => 16_000,
    }
}

/// Reasoning effort for a compaction-summary ladder stage.
///
/// Stage 0 inherits the session's live effort (the summary request never
/// enables Claude-style thinking). From stage 1 on, thinking is minimized where
/// the provider allows it: DeepSeek and Kimi K3 forward `low`, while an OpenAI
/// reasoning model — one with a configured effort — sends `none`. Providers
/// whose thinking is already off (Anthropic, unknown) keep omitting the field.
///
/// DeepSeek *can* be switched off outright (`reasoning.effort = "none"` on the
/// Responses format, `thinking.type = "disabled"` on chat), but the summarizer
/// deliberately stops at `low`: the identifier-level detail in a handoff is
/// produced inside that thinking, and the continuation stage only has to finish
/// a draft (measured: 649 reasoning + ~465 text tokens of a 2,000-token
/// envelope).
pub(crate) fn compact_summary_effort(
    provider_kind: &ProviderKind,
    session_effort: Option<OpenAiReasoningEffort>,
    stage: u32,
) -> Option<OpenAiReasoningEffort> {
    if stage == 0 {
        return session_effort;
    }
    match provider_kind {
        ProviderKind::DeepSeek | ProviderKind::Kimi => Some(OpenAiReasoningEffort::Low),
        ProviderKind::OpenAi => session_effort.map(|_| OpenAiReasoningEffort::None),
        ProviderKind::Anthropic | ProviderKind::Custom(_) => None,
    }
}

/// Next reasoning reserve for an adaptive compaction attempt.
///
/// The three signals are used as floor / target / cap rather than a plain
/// minimum — a minimum can shrink the envelope or leave the text starved:
/// - floor: the largest of the previous reserve, the effort-implied reserve and
///   a minimum headroom (a quarter of the text budget), so the reserve never
///   shrinks after a truncation and never collapses to zero;
/// - target: the reasoning the model actually spent plus 25% headroom; when no
///   reasoning tokens are reported the summary text itself overran, so the
///   current room is doubled instead;
/// - cap: at most double the current room (the caller additionally caps by the
///   context window).
pub(crate) fn next_compaction_reserve(
    effort_implied: u32,
    text_budget: u32,
    prev_reserve: u32,
    observed_think: usize,
) -> u32 {
    let floor = prev_reserve.max(effort_implied).max(text_budget / 4);
    let cap = floor.saturating_mul(2);
    let observed = u32::try_from(observed_think).unwrap_or(u32::MAX);
    let target = if observed == 0 {
        floor.saturating_mul(2)
    } else {
        observed.saturating_add(observed / 4)
    };
    target.clamp(floor, cap)
}

struct FacadeStateGuard<'a> {
    agent: &'a mut Agent,
    activation: Option<FacadeActivationState>,
    receipts: Arc<FacadeReceiptTracker>,
}

impl<'a> FacadeStateGuard<'a> {
    fn take(agent: &'a mut Agent, receipts: Arc<FacadeReceiptTracker>) -> Self {
        let activation = agent.take_facade_state();
        Self {
            agent,
            activation: Some(activation),
            receipts,
        }
    }

    fn state_mut(&mut self) -> &mut AgentState {
        &mut self
            .activation
            .as_mut()
            .expect("facade activation state is present until restore")
            .state
    }

    fn finish_activation(
        &mut self,
        session_start_pending: bool,
        session_start_source: SessionStartSource,
        pending_session_context: Vec<HookContextChunk>,
        compact_state: crate::compact::CompactState,
        prompt_caches: PromptCaches,
    ) {
        let activation = self
            .activation
            .as_mut()
            .expect("facade activation state is present until restore");
        activation.sidecar.session_start_hooks_pending = session_start_pending;
        activation.sidecar.session_start_source = session_start_source;
        activation.sidecar.pending_session_context = pending_session_context;
        activation.sidecar.compact_state = compact_state;
        activation.sidecar.cached_dir_snapshot = prompt_caches.directory_snapshot;
        activation.sidecar.cached_agents_md = prompt_caches.agents_md;
        activation.sidecar.apply_receipts(self.receipts.snapshot());
    }

    fn restore(&mut self) {
        if let Some(mut activation) = self.activation.take() {
            activation.sidecar.apply_receipts(self.receipts.snapshot());
            self.agent.restore_facade_state(activation);
        }
    }
}

impl Drop for FacadeStateGuard<'_> {
    fn drop(&mut self) {
        self.restore();
    }
}

/// Prompt text appended (to the system prompt, for this request only) on the
/// last permitted model call of a Goal run.
///
/// The hard turn cap is checked *before* the previous call, so this is the
/// model's last chance to persist what it learned. The reminder is advisory —
/// the cap still decides the stop — but without it the activation routinely
/// ended mid-thought with nothing written for the next run.
pub(crate) fn goal_final_turn_reminder(turn: u32) -> String {
    format!(
        "<goal-budget-reminder>\n\
         This is the final model call of this Goal run (turn {turn}).\n\
         Before you finish, call goal_checkpoint with a concise summary of what \
         you completed, what is still open, and the next concrete step, so the \
         next run can continue without re-discovering it. Do not start work you \
         cannot describe in that checkpoint.\n\
         </goal-budget-reminder>"
    )
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
    pub permission_manager: PermissionAuthority,
    /// The sensitive-path guard, derived from the permission settings at
    /// construction and inherited by subagents through the same snapshot that
    /// carries the permission context. Evaluated **before** modes, hooks and
    /// rules, so nothing can reach a credential file.
    pub security: crate::security::sensitive::Scanner,
    pub stats: Arc<RwLock<SessionStats>>,
    /// Where this agent's updates go: the answering frontend, if any, plus
    /// any read-only observers.
    pub events: UpdateSink,
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
    /// Cached `AGENTS.md` assembly (once per session) for a stable prompt prefix.
    pub cached_agents_md: Option<String>,
    /// Total tokens from the most recent LLM usage report (`0` = none yet).
    pub last_token_total: u32,
    /// Provider-specific conversation state (currently only the OpenAI
    /// Responses protocol baseline). Loaded in [`Agent::ensure_session`],
    /// committed after every LLM response, and passed into every LLM call.
    pub provider_state: Option<ProviderConversationState>,
    /// Goal-yield reason returned by the most recent tool dispatch, if this
    /// activation must return control to the Runner for user input.
    pub goal_yield_reason: Option<GoalYieldReason>,
    /// Results from background subagents that finished while the parent was
    /// mid-turn. Drained before the next LLM call and re-injected as a
    /// synthetic `<subagent-finished>` user message. `Arc<Mutex<VecDeque>>`
    /// so detached child tasks can push into it via the stamped
    /// `ToolContext.subagent_results` handle.
    pub pending_subagent_results: Arc<Mutex<VecDeque<SubagentResult>>>,
    /// Context collected by `SessionStart` hooks (plugin `additionalContext`),
    /// drained once by `Agent::inject_pending_session_context` before the
    /// first turn and recorded as synthetic `<hook-context>` user messages.
    ///
    /// Held here rather than pushed at dispatch time because
    /// [`Agent::ensure_session`] loads history only into an *empty* context —
    /// injecting during `dispatch_session_start_hooks` (which runs before the
    /// session is ensured) would suppress the history restore entirely.
    /// Context collected by [`Self::dispatch_session_start_hooks`], drained
    /// once by `Agent::inject_pending_session_context` before the first turn.
    pub pending_session_context: Vec<HookContextChunk>,
    /// Whether the `Interrupt` hooks have run for this turn.
    ///
    /// Cancellation is observed at several points in the loop; the hooks are
    /// observational, so running them once per turn is what a plugin expects.
    pub interrupt_hooks_fired: bool,
    /// Whether the `SessionStart` hooks still have to run.
    ///
    /// Startup no longer calls them: a plugin hook can take seconds (the
    /// reference `basic-memory` one takes ~8s warm) and would otherwise delay
    /// the first frame. [`Agent::agent_loop`] dispatches them on the first turn
    /// instead, which is still early enough — the context it collects is
    /// recorded before that turn's user message.
    pub session_start_hooks_pending: bool,
    /// What the `SessionStart` hooks are told they are running for; see
    /// [`SessionStartSource`]. Set to `Resume` when history is restored and to
    /// `Compact` after a compaction re-queues the hooks.
    pub session_start_source: SessionStartSource,
    /// Context collected by `PreToolUse` / `PostToolUse` hooks during a turn.
    ///
    /// `Arc<Mutex<…>>` because those hooks receive only `&Agent`; the agent loop
    /// drains it before building the next request, so the model reads the
    /// context alongside the tool call it annotates. Same shape as
    /// [`Self::pending_subagent_results`].
    pub pending_hook_context: Arc<Mutex<VecDeque<HookContextChunk>>>,
    /// Hands out the ids that tie a hook's progress line to its completion.
    ///
    /// Atomic because the `SessionStart` hooks run concurrently and each only
    /// ever sees `&Agent`; a plain counter would need a lock to be shared, and
    /// the ids have to be unique across those parallel runs or one hook's
    /// "finished" would rewrite another's line.
    pub hook_status_seq: Arc<std::sync::atomic::AtomicU64>,
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
    pub mcp_router: McpCatalogHandle,
    pub hooks: Vec<Hook>,
    pub system_prompt: AgentSystemPrompt,
    pub tool_use_counter: usize,
    /// Optional cap on nested agent-loop turns (set by `spawn_subagent`'s
    /// `max_turns` input to prevent runaway subagents). `None` = unbounded.
    pub max_turns: Option<u32>,
    /// Number of LLM turns completed in the current agent loop.
    pub turns_taken: u32,
    /// Set when the loop stopped because it hit [`Self::max_turns`].
    ///
    /// Recorded where it happens instead of being re-derived as
    /// `turns_taken > max_turns` by whoever reads the run afterwards: the cap
    /// check then lives in exactly one place, and the summary that has to
    /// explain *why* a child stopped cannot disagree with the loop about
    /// whether it did.
    pub max_turns_reached: bool,
    shared_services: TactSharedServices,
    #[cfg(any(test, feature = "test-support"))]
    kernel_activations: u32,
    /// Snapshot of agent settings at construction; avoids parallel tests racing on global config.
    agent_settings: AgentSettings,
    /// Compaction-routing provider kind, when set explicitly via
    /// [`Self::with_provider_kind`]. `None` means "inherit from the live
    /// provider" — see [`Self::provider_kind`].
    provider_kind: Option<ProviderKind>,
    cached_tool_specs: Vec<ToolSpec>,
}

impl Agent {
    /// Whether a background subagent result is waiting to be re-injected.
    ///
    /// The driver consults this before submitting a subagent-finished wake-up
    /// turn: such a turn only exists to drain the queue into the parent's
    /// context, so an empty queue means a previous turn already delivered the
    /// summary and the extra turn would have nothing to review.
    pub fn has_pending_subagent_results(&self) -> bool {
        self.runtime
            .pending_subagent_results
            .lock()
            .map(|queue| !queue.is_empty())
            // A poisoned lock leaves the queue contents unknown; fall back to
            // the previous always-wake behaviour rather than silently
            // swallowing a completion.
            .unwrap_or(true)
    }

    /// Number of bounded kernel activations started by this agent.
    #[cfg(any(test, feature = "test-support"))]
    pub fn kernel_activations_for_test(&self) -> u32 {
        self.kernel_activations
    }

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
        //
        // Routing kind is left unset: it is resolved lazily from the live
        // provider so a subagent (which never calls `with_provider_kind`)
        // inherits its parent's routing instead of silently defaulting to
        // OpenAI and calling a `/responses/compact` endpoint that DeepSeek
        // does not implement.
        let provider_kind = None;
        let cancel_flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        tool_context.cancel_flag = cancel_flag.clone();
        // Derived here rather than passed in, so every construction site — the
        // TUI, headless, and a subagent built from a permission snapshot —
        // inherits the same guard without a second parameter to thread.
        let security = permission_manager.scanner();
        let permission_manager = PermissionAuthority::new(permission_manager);
        let stats = Arc::new(RwLock::new(SessionStats::default()));
        let events = UpdateSink::default();
        let pending_subagent_results = Arc::new(Mutex::new(VecDeque::new()));
        let pending_hook_context = Arc::new(Mutex::new(VecDeque::new()));
        let hook_status_seq = Arc::new(std::sync::atomic::AtomicU64::new(1));
        let mcp_router = McpCatalogHandle::new(mcp_router);
        let shared_services = TactSharedServices::new(
            events.clone(),
            stats.clone(),
            permission_manager.clone(),
            pending_subagent_results.clone(),
            pending_hook_context.clone(),
            hook_status_seq.clone(),
            mcp_router.clone(),
        );
        let mut agent = Self {
            runtime: AgentRuntime {
                interrupt_hooks_fired: false,
                client,
                context: Vec::new(),
                compact_state: CompactState::default(),
                recovery_state: RecoveryState::default(),
                permission_manager,
                security,
                stats,
                events,
                cancel_flag,
                session_store: None,
                session_id: None,
                first_message_db_id: 0,
                last_message_db_id: 0,
                llm_call_last_message_id: 0,
                cached_dir_snapshot: None,
                cached_agents_md: None,
                last_token_total: 0,
                provider_state: None,
                goal_yield_reason: None,
                pending_subagent_results,
                pending_session_context: Vec::new(),
                session_start_hooks_pending: true,
                session_start_source: SessionStartSource::Startup,
                pending_hook_context,
                hook_status_seq,
            },
            tool_context,
            tools,
            mcp_router,
            hooks: Vec::new(),
            system_prompt,
            tool_use_counter: 0,
            max_turns: None,
            turns_taken: 0,
            max_turns_reached: false,
            shared_services,
            #[cfg(any(test, feature = "test-support"))]
            kernel_activations: 0,
            agent_settings: crate::config::settings().agent.clone(),
            provider_kind,
            cached_tool_specs: Vec::new(),
        };
        agent.rebuild_cached_tool_specs();
        agent
    }

    /// Override the provider kind used for Responses compaction routing
    /// (OpenAI → native `/responses/compact`; DeepSeek → local summary
    /// fallback because its endpoint lacks the endpoint).
    pub fn with_provider_kind(mut self, provider_kind: ProviderKind) -> Self {
        self.provider_kind = Some(provider_kind);
        self
    }

    /// The provider kind compaction is routed by.
    ///
    /// Explicitly set kinds ([`Self::with_provider_kind`]) win; otherwise the
    /// live provider is consulted, falling back to OpenAI when no provider has
    /// been installed (tests that never call `init_provider`).
    fn provider_kind(&self) -> ProviderKind {
        self.provider_kind
            .clone()
            .or_else(tact_llm::current_provider_kind)
            .unwrap_or(ProviderKind::OpenAi)
    }

    /// Rebuild the cached tool specs from the native tools plus the current
    /// MCP router. Called after any MCP router replacement.
    fn rebuild_cached_tool_specs(&mut self) {
        let native_specs = if matches!(self.runtime.client, LlmProvider::OpenAiResponses(_)) {
            self.tools
                .tool_specs()
                .into_iter()
                .filter(|spec| spec.name != "compact")
                .collect()
        } else {
            self.tools.tool_specs()
        };
        self.cached_tool_specs = native_specs
            .into_iter()
            .chain(self.mcp_router.all_tools())
            // `list_mcp_resources` / `read_mcp_resource` exist only while a server
            // is connected: with an empty router they would be tools that can
            // only ever answer "no MCP servers are connected".
            .chain(self.mcp_router.resource_tool_specs())
            // Same rule for the prompt pair.
            .chain(self.mcp_router.prompt_tool_specs())
            .collect();
    }

    /// Picks up any MCP server that announced a tool-list change.
    ///
    /// Called immediately before each request is built, because the tool list is
    /// sent per request. Quiet servers cost nothing: `refresh_changed` re-lists
    /// only the ones that sent `notifications/tools/list_changed`, so this is
    /// not a poll.
    ///
    /// A change is reported rather than applied in silence — a tool quietly
    /// appearing or disappearing is exactly the kind of thing the model's
    /// behaviour is blamed for afterwards.
    #[cfg(test)]
    async fn refresh_mcp_tools(&mut self) {
        let report = self.mcp_router.refresh_changed().await;
        if report.is_empty() {
            return;
        }
        for (server, refresh) in &report.changed {
            self.emit_update(AgentUpdate::Info(format!(
                "[mcp] {server} changed its tool list: {}",
                refresh.describe()
            )));
        }
        for (server, reason) in &report.failed {
            self.emit_update(AgentUpdate::Info(format!(
                "[mcp] {server} announced a tool-list change that could not be read: {reason}"
            )));
        }
        self.rebuild_cached_tool_specs();
    }

    /// Reload every MCP server from disk and rebuild the tool list.
    ///
    /// Used after an interactive OAuth authorization (`/mcp auth <server>`) so
    /// the newly authorized remote server becomes usable without restarting.
    /// The previous connections are shut down first; failures are reported, not
    /// propagated, mirroring startup semantics.
    pub async fn reload_mcp_router(&mut self) -> McpLoadReport {
        self.mcp_router.disconnect_all().await;
        match crate::mcp::load_mcp_router_with_report().await {
            Ok((router, report)) => {
                self.mcp_router = McpCatalogHandle::new(router);
                self.shared_services
                    .replace_mcp_catalog(self.mcp_router.clone());
                self.rebuild_cached_tool_specs();
                report
            }
            Err(error) => {
                tracing::warn!(error = %error, "failed to reload MCP servers");
                McpLoadReport {
                    failures: vec![("mcp".to_owned(), format!("{error:#}"))],
                    ..McpLoadReport::default()
                }
            }
        }
    }

    /// Override agent-loop settings (used by integration tests with custom config).
    pub fn with_agent_settings(mut self, settings: AgentSettings) -> Self {
        self.agent_settings = settings;
        self
    }

    /// Cap the number of LLM turns the agent loop will run (subagent runaway
    /// guard). `None` disables the cap.
    pub fn with_max_turns(mut self, max_turns: Option<u32>) -> Self {
        self.max_turns = max_turns;
        self
    }

    fn model_context_window(&self) -> usize {
        self.agent_settings.model_context_window
    }

    /// The best available estimate of what the context window currently holds.
    ///
    /// Prefers the provider-reported total — it is the real wire figure and the
    /// only one that survives native compaction — and falls back to the
    /// serialized-context estimate before the first response of a session.
    /// Returns 0 when neither is available, which callers must read as
    /// "unknown" rather than "empty".
    #[cfg(test)]
    fn current_context_tokens(&self) -> usize {
        if self.runtime.last_token_total > 0 {
            return self.runtime.last_token_total as usize;
        }
        estimate_context_tokens(&self.runtime.context)
    }

    /// The subagent advisory for this task, when the window is filling up.
    ///
    /// Computed once per task, at the moment the system prompt is built: the
    /// prompt is intentionally a per-task snapshot so the prefix KV-cache holds
    /// across turns, so this cannot track the window mid-task. That is
    /// acceptable because it is the *start* of a task that decides how the work
    /// gets split, and because auto-compaction resets the pressure inside a
    /// long task anyway.
    ///
    /// Requires `spawn_subagent` to actually be offered: the text tells the
    /// model to reach for that tool, so without it the line would be advice the
    /// model cannot take — and a prompt that names a nonexistent tool teaches
    /// the model to distrust the prompt. Today the main toolset always registers
    /// it; the guard is here so a future gate (a restricted toolset, a config
    /// switch) cannot leave the two out of step.
    #[cfg(test)]
    fn context_advisory(&self) -> Option<String> {
        if !self.has_tool("spawn_subagent") {
            return None;
        }
        crate::compact::context_pressure_advisory(
            self.current_context_tokens(),
            self.model_context_window(),
        )
        .then(crate::compact::subagent_advisory)
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
    #[cfg(test)]
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
    /// Budget semantics and effort semantics are mutually exclusive: setting a
    /// budget clears any stale reasoning effort, so the status bar never shows
    /// a meaningless `think high(32K)` for a budget-semantic model.
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
        self.agent_settings.reasoning_effort = None;
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

    /// The model this agent is currently calling.
    ///
    /// Configured at construction and rewritten by [`Self::set_model`], so it is
    /// the value a `SessionStart` hook payload should carry as `model`.
    #[must_use]
    pub fn model(&self) -> &str {
        &self.agent_settings.model
    }

    /// Update this agent's session reasoning effort (per-agent; never the
    /// global provider). `None` clears the explicit effort (wire omits it).
    ///
    /// Effort semantics and budget semantics are mutually exclusive: setting an
    /// effort clears any stale thinking budget, so the status bar never shows a
    /// meaningless `(32K)` next to an explicit effort for effort-semantic models
    /// (openai / deepseek / kimi k3).
    ///
    /// Same queue semantics as [`set_thinking_budget`].
    pub fn set_reasoning_effort(&mut self, effort: Option<OpenAiReasoningEffort>) {
        self.agent_settings.reasoning_effort = effort;
        self.agent_settings.thinking_budget = 0;
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

    /// Attaches a TUI update channel so the agent can stream events
    /// (token usage, thinking blocks, tool results) to the terminal.
    ///
    /// This is the *answering* channel: a frontend holding it is claiming it
    /// can answer permission prompts. A frontend that only watches must use
    /// [`Self::with_event_observer`] instead.
    pub fn with_ui_channel(mut self, tx: tokio::sync::mpsc::UnboundedSender<AgentUpdate>) -> Self {
        self.runtime.events = UpdateSink::for_ui(tx.clone());
        self.shared_services
            .set_updates(self.runtime.events.clone());
        // Keep tool_context in sync so ToolProgress / tool helpers use the same
        // sink (critical for tagged subagent traffic).
        self.tool_context.events = self.runtime.events.clone();
        self
    }

    /// Attach a read-only observer for every [`AgentUpdate`] this agent emits.
    ///
    /// Distinct from [`Self::with_ui_channel`], which also makes the frontend
    /// the answerer of permission prompts. A headless run has no one to ask, so
    /// it must not claim that channel; it only needs to *see* progress. The
    /// permission path is therefore untouched: the sink carries no `ui`
    /// channel, so every check takes the non-interactive branch.
    pub fn with_event_observer(
        mut self,
        tx: tokio::sync::mpsc::UnboundedSender<AgentUpdate>,
    ) -> Self {
        self.runtime.events = UpdateSink::observing(tx);
        self.shared_services
            .set_updates(self.runtime.events.clone());
        self.tool_context.events = self.runtime.events.clone();
        self
    }

    /// Attach the observers of `events`, dropping its answering channel.
    ///
    /// The subagent path uses this to give a child the parent's destinations
    /// without handing it the parent's permission-prompt claim.
    pub fn with_observed_events(mut self, events: UpdateSink) -> Self {
        self.runtime.events = events.observers_only();
        self.shared_services
            .set_updates(self.runtime.events.clone());
        self.tool_context.events = self.runtime.events.clone();
        self
    }

    /// Stop sending events to any observer, keeping the answering frontend.
    ///
    /// A headless run calls this once its turn is over so the log pump can
    /// drain and return: the pump waits on the channel closing, and the agent
    /// itself would otherwise keep the sender alive until it is dropped.
    pub fn detach_event_observers(&mut self) {
        let ui = self.runtime.events.ui().cloned();
        self.runtime.events = match ui {
            Some(tx) => UpdateSink::for_ui(tx),
            None => UpdateSink::default(),
        };
        self.shared_services
            .set_updates(self.runtime.events.clone());
        self.tool_context.events = self.runtime.events.clone();
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
        self.shared_services.emit_update(update);
    }

    pub(crate) fn activation_view(&self) -> TactActivationView<'_> {
        let session = SessionIdentity {
            id: self.runtime.session_id.clone(),
        };
        let model = ModelIdentity {
            name: self.agent_settings.model.clone(),
        };
        let remaining_turns = self
            .max_turns
            .map(|max| max.saturating_sub(self.turns_taken));
        TactActivationView::new(
            &self.shared_services,
            Some(&self.runtime.context),
            session,
            model,
            self.turns_taken,
            remaining_turns,
            self.runtime.session_start_source,
        )
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
            if !history.is_empty() {
                // Restoring history makes this a resume, not a fresh start —
                // the distinction a plugin's `startup|resume` matcher is for.
                self.runtime.session_start_source = SessionStartSource::Resume;
            }
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
    /// Provider and base URL must match. Model mismatches are intentionally
    /// allowed for experimentation; the provider may reject incompatible
    /// opaque state at request time.
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
        Ok(())
    }

    fn is_openai_responses(&self) -> bool {
        matches!(self.runtime.client, LlmProvider::OpenAiResponses(_))
    }

    fn kernel_model_metadata(&self) -> serde_json::Value {
        serde_json::json!({
            "reasoning_effort": self
                .agent_settings
                .reasoning_effort
                .map(|effort| effort.as_str().to_string()),
            "thinking_budget": (self.thinking_budget() > 0)
                .then_some(self.thinking_budget()),
        })
    }

    fn facade_receipts(&self) -> FacadeReceipts {
        FacadeReceipts {
            first_message_db_id: self.runtime.first_message_db_id,
            last_message_db_id: self.runtime.last_message_db_id,
            llm_call_last_message_id: self.runtime.llm_call_last_message_id,
        }
    }

    fn take_facade_state(&mut self) -> FacadeActivationState {
        let original_context = self.runtime.context.clone();
        let state = AgentState {
            transcript: self
                .runtime
                .context
                .drain(..)
                .map(|message| tact_llm::neutral::agent_message(&message))
                .collect(),
            cursor: self
                .runtime
                .provider_state
                .take()
                .as_ref()
                .map(tact_llm::neutral::model_cursor),
            total_turns: std::mem::take(&mut self.turns_taken),
            last_token_total: Some(std::mem::take(&mut self.runtime.last_token_total)),
            initialized: true,
        };
        let sidecar = FacadeSidecar {
            first_message_db_id: std::mem::take(&mut self.runtime.first_message_db_id),
            last_message_db_id: std::mem::take(&mut self.runtime.last_message_db_id),
            llm_call_last_message_id: std::mem::take(&mut self.runtime.llm_call_last_message_id),
            compact_state: std::mem::take(&mut self.runtime.compact_state),
            recovery_state: std::mem::take(&mut self.runtime.recovery_state),
            goal_yield_reason: self.runtime.goal_yield_reason.take(),
            max_turns_reached: std::mem::take(&mut self.max_turns_reached),
            tool_use_counter: std::mem::take(&mut self.tool_use_counter),
            interrupt_hooks_fired: std::mem::take(&mut self.runtime.interrupt_hooks_fired),
            session_start_hooks_pending: std::mem::take(
                &mut self.runtime.session_start_hooks_pending,
            ),
            session_start_source: self.runtime.session_start_source,
            pending_session_context: std::mem::take(&mut self.runtime.pending_session_context),
            cached_dir_snapshot: self.runtime.cached_dir_snapshot.take(),
            cached_agents_md: self.runtime.cached_agents_md.take(),
        };
        FacadeActivationState {
            state,
            sidecar,
            original_context,
        }
    }

    fn restore_facade_state(&mut self, activation: FacadeActivationState) {
        let FacadeActivationState {
            state,
            sidecar,
            original_context,
        } = activation;
        let transcript_unchanged = state.transcript.len() == original_context.len()
            && state
                .transcript
                .iter()
                .zip(&original_context)
                .all(|(current, original)| current == &tact_llm::neutral::agent_message(original));
        let context = if transcript_unchanged {
            original_context
        } else {
            let mut context = Vec::with_capacity(state.transcript.len());
            for message in &state.transcript {
                let mut restored = tact_llm::neutral::llm_message(message);
                restored.kind = match &message.origin {
                    MessageOrigin::CompactionSummary => MessageKind::Summary,
                    MessageOrigin::InjectedContext { .. } => MessageKind::HookContext,
                    MessageOrigin::GoalContext => MessageKind::GoalContext,
                    MessageOrigin::User
                    | MessageOrigin::Assistant
                    | MessageOrigin::ToolResult
                    | MessageOrigin::SubagentResult
                    | MessageOrigin::Continuation => MessageKind::Normal,
                };
                context.push(restored);
            }
            context
        };
        let provider_state = state
            .cursor
            .as_ref()
            .map(tact_llm::neutral::provider_state)
            .transpose()
            .unwrap_or_else(|error| {
                panic!("kernel returned a provider cursor Tact cannot restore: {error}")
            });

        self.runtime.context = context;
        self.runtime.provider_state = provider_state;
        self.turns_taken = state.total_turns;
        self.runtime.last_token_total = state.last_token_total.unwrap_or(0);
        self.runtime.first_message_db_id = sidecar.first_message_db_id;
        self.runtime.last_message_db_id = sidecar.last_message_db_id;
        self.runtime.llm_call_last_message_id = sidecar.llm_call_last_message_id;
        self.runtime.compact_state = sidecar.compact_state;
        self.runtime.recovery_state = sidecar.recovery_state;
        self.runtime.goal_yield_reason = sidecar.goal_yield_reason;
        self.max_turns_reached = sidecar.max_turns_reached;
        self.tool_use_counter = sidecar.tool_use_counter;
        self.runtime.interrupt_hooks_fired = sidecar.interrupt_hooks_fired;
        self.runtime.session_start_hooks_pending = sidecar.session_start_hooks_pending;
        self.runtime.session_start_source = sidecar.session_start_source;
        self.runtime.pending_session_context = sidecar.pending_session_context;
        self.runtime.cached_dir_snapshot = sidecar.cached_dir_snapshot;
        self.runtime.cached_agents_md = sidecar.cached_agents_md;
    }

    /// Preserve the caller's message shorthand across the neutral kernel round-trip.
    ///
    /// `AgentMessage` intentionally has no `Text`/`Blocks` shape flag, so a
    /// newly appended input comes back canonically as one text block. Restore
    /// the exact input shape after reconciliation; persisted user turns and
    /// injected context otherwise change wire/JSON shape just by being run.
    fn restore_input_shape(&mut self, input: &Message) {
        let neutral_input = tact_llm::neutral::agent_message(input);
        if let Some(restored) = self
            .runtime
            .context
            .iter_mut()
            .rev()
            .find(|message| tact_llm::neutral::agent_message(message) == neutral_input)
        {
            restored.content = input.content.clone();
            restored.kind = input.kind;
        }
    }

    fn kernel_prompt_assembler(&self) -> SystemPromptAssembler {
        let static_prompt = match &self.system_prompt {
            AgentSystemPrompt::Static(prompt) => Some(prompt.clone()),
            AgentSystemPrompt::Dynamic => None,
        };
        SystemPromptAssembler::new(
            static_prompt,
            self.is_openai_responses(),
            self.tool_context.work_dir.clone(),
            self.agent_settings.clone(),
            self.tool_context.skill_registry.clone(),
            self.tool_context.memory_manager.clone(),
            self.mcp_router.clone(),
            &self.cached_tool_specs,
            PromptCaches {
                directory_snapshot: self.runtime.cached_dir_snapshot.clone(),
                agents_md: self.runtime.cached_agents_md.clone(),
            },
        )
    }

    fn kernel_compactor(
        &self,
        services: Arc<TactSharedServices>,
        prompt: Arc<SystemPromptAssembler>,
    ) -> TactCompactor {
        let native_compaction =
            self.is_openai_responses() && self.provider_kind() != ProviderKind::DeepSeek;
        TactCompactor::new(
            services,
            prompt,
            self.runtime.session_id.clone().unwrap_or_default(),
            ModelIdentity {
                name: self.agent_settings.model.clone(),
            },
            self.tool_context.work_dir.clone(),
            self.agent_settings.clone(),
            self.cached_tool_specs.clone(),
            self.provider_kind(),
            native_compaction,
            self.runtime.session_start_hooks_pending,
            self.runtime.session_start_source,
            self.runtime.compact_state.clone(),
            &self.hooks,
        )
    }

    fn native_compaction_available(&self) -> bool {
        self.is_openai_responses() && self.provider_kind() != ProviderKind::DeepSeek
    }

    fn kernel_composition(&self) -> (TactKernelComposition, String, Arc<FacadeReceiptTracker>) {
        let model_name = self.agent_settings.model.clone();
        let session_id = self.runtime.session_id.clone().unwrap_or_default();
        let updates = self.runtime.events.clone();
        let services = Arc::new(self.shared_services.clone());
        let prompt = Arc::new(self.kernel_prompt_assembler());
        let compactor = Arc::new(self.kernel_compactor(services.clone(), prompt.clone()));
        let mut tool_context = self.tool_context.clone();
        tool_context.events = updates.clone();
        tool_context.cancel_flag = self.runtime.cancel_flag.clone();
        tool_context.session_id = self.runtime.session_id.clone();
        tool_context.session_store = self.runtime.session_store.clone();
        tool_context.permission_snapshot = Some(self.runtime.permission_manager.snapshot());
        let receipts = FacadeReceiptTracker::new(self.facade_receipts());

        let composition = TactKernelComposition::new(TactKernelConfig {
            provider: self.runtime.client.clone(),
            cancel_flag: self.runtime.cancel_flag.clone(),
            capabilities: ModelCapabilities {
                model: model_name.clone(),
                context_window: self.model_context_window(),
                max_output_tokens: self.max_tokens(),
                supports_streaming: true,
                supports_native_compaction: self.native_compaction_available(),
            },
            prompt,
            compactor,
            model_metadata: self.kernel_model_metadata(),
            model: ModelIdentity { name: model_name },
            session_id: session_id.clone(),
            initial_session_context: self.runtime.pending_session_context.clone(),
            services,
            hooks: self.hooks.clone(),
            ui: tool_context.ui_responder.clone(),
            is_subagent: tool_context.is_subagent,
            tools: self.tools.clone(),
            mcp: self.mcp_router.clone(),
            tool_context,
            security: self.runtime.permission_manager.security_config(),
            scanner: self.runtime.security.clone(),
            hide_compact: self.is_openai_responses(),
            store: self.runtime.session_store.clone(),
            receipts: receipts.clone(),
            updates,
        });
        (composition, session_id, receipts)
    }

    async fn compact_with_kernel(
        &mut self,
        request: tact_agent_api::CompactionRequest,
    ) -> Result<()> {
        let native_compaction = self.native_compaction_available();
        self.ensure_session().await?;
        let (composition, session_id, receipts) = self.kernel_composition();
        let bridge = CancelBridge::new(self.runtime.cancel_flag.clone());
        let cancel = bridge.token();
        let mut guard = FacadeStateGuard::take(self, receipts);
        let unwound = AssertUnwindSafe(async {
            composition
                .kernel()
                .compact(
                    guard.state_mut(),
                    &SessionRef::new(session_id),
                    request,
                    cancel,
                )
                .await
        })
        .catch_unwind()
        .await;
        guard.finish_activation(
            composition.session_start_pending(),
            composition.session_start_source(),
            composition.pending_session_context(),
            composition.compact_state(),
            composition.prompt_caches(),
        );
        guard.restore();
        drop(guard);

        match unwound {
            Ok(result) => result.map(|_| ()).map_err(|error| match error {
                KernelError::Usage(message) if native_compaction => {
                    anyhow::anyhow!("failed to persist Responses compact usage: {message}")
                }
                error => anyhow::anyhow!(error),
            }),
            Err(payload) => std::panic::resume_unwind(payload),
        }
    }

    async fn run_kernel_activation(
        &mut self,
        user_turn_message: Option<Message>,
    ) -> Result<AgentLoopOutcome> {
        let (composition, session_id, receipts) = self.kernel_composition();

        let inputs = user_turn_message
            .as_ref()
            .map(tact_llm::neutral::agent_message)
            .into_iter()
            .collect();
        let cancel_flag = self.runtime.cancel_flag.clone();
        let bridge = CancelBridge::new(cancel_flag);
        let cancel = bridge.token();
        let input = RunInput {
            session: SessionRef::new(session_id),
            inputs,
            max_total_turns: self.max_turns,
        };
        let mut guard = FacadeStateGuard::take(self, receipts);
        let unwound = AssertUnwindSafe(async {
            composition
                .kernel()
                .run(guard.state_mut(), input, cancel)
                .await
        })
        .catch_unwind()
        .await;
        guard.finish_activation(
            composition.session_start_pending(),
            composition.session_start_source(),
            composition.pending_session_context(),
            composition.compact_state(),
            composition.prompt_caches(),
        );
        guard.restore();
        drop(guard);
        if let Some(input) = user_turn_message.as_ref() {
            self.restore_input_shape(input);
        }

        let kernel_result = match unwound {
            Ok(result) => result,
            Err(payload) => std::panic::resume_unwind(payload),
        };
        let outcome = match kernel_result {
            Ok(outcome) => outcome,
            Err(error) => {
                let message = error.to_string();
                if message.contains("refused") {
                    self.emit_update(AgentUpdate::Info(
                        "Model refused this request (stop_reason=refusal). Try rephrasing, \
                         or switch to another model with different safety filters."
                            .to_string(),
                    ));
                }
                return Err(anyhow::anyhow!(error));
            }
        };
        self.runtime.recovery_state = RecoveryState {
            continuation_attempts: outcome.recovery.continuation_attempts,
            compact_attempts: outcome.recovery.compact_attempts,
            transport_attempts: outcome.recovery.transport_attempts,
        };

        let stop = match outcome.stop {
            RunStop::Completed => AgentLoopStop::Completed,
            RunStop::YieldToHost { .. } => {
                self.runtime.goal_yield_reason = Some(GoalYieldReason::NeedsUser);
                AgentLoopStop::GoalYield(GoalYieldReason::NeedsUser)
            }
            RunStop::MaxTurns => {
                self.max_turns_reached = true;
                AgentLoopStop::MaxTurns
            }
            RunStop::MaxTokens => AgentLoopStop::MaxTokens,
            RunStop::Cancelled => {
                self.dispatch_interrupt_hooks().await?;
                self.runtime.interrupt_hooks_fired = true;
                AgentLoopStop::Cancelled
            }
            RunStop::Blocked { reason, .. } => {
                self.emit_update(AgentUpdate::Info(format!(
                    "[hook stopped the turn] {reason}"
                )));
                AgentLoopStop::BlockedByHook
            }
        };
        self.tool_use_counter = self
            .tool_use_counter
            .saturating_add(usize::try_from(outcome.tool_calls).unwrap_or(usize::MAX));
        Ok(AgentLoopOutcome {
            stop,
            turns: outcome.turns,
            tool_calls: outcome.tool_calls,
        })
    }

    /// The main agent conversation loop.
    ///
    /// 1. Builds the system prompt and primes the context.
    /// 2. Loops: sends context to LLM → processes streaming response →
    ///    dispatches tool-use blocks (native or MCP) → applies permissions →
    ///    writes results back.  Continues until the LLM returns a stop reason
    ///    other than `ToolUse` or an unrecoverable error occurs.
    ///
    /// This is the plain entry point. It delegates to
    /// [`Agent::agent_loop_detailed`] and discards the typed outcome, keeping
    /// the historical `Result<()>` signature and its externally visible effects
    /// (same [`AgentUpdate`]s, same messages persisted, same `Ok(())`).
    #[tracing::instrument(skip(self), name = "agent_loop")]
    pub async fn agent_loop(&mut self, user_turn_message: Option<Message>) -> Result<()> {
        self.agent_loop_detailed(user_turn_message)
            .await
            .map(|_| ())
    }

    /// One bounded activation of the agent conversation loop.
    ///
    /// Runs the same loop as [`Agent::agent_loop`], but returns an
    /// [`AgentLoopOutcome`] describing *why* the activation returned and how
    /// much budget it consumed — model calls (`turns`) and tool calls actually
    /// executed (`tool_calls`). The driver-owned Goal Runner uses this to decide
    /// whether to run another bounded activation.
    ///
    /// 1. Builds the system prompt and primes the context.
    /// 2. Loops: sends context to LLM → processes streaming response →
    ///    dispatches tool-use blocks (native or MCP) → applies permissions →
    ///    writes results back.  Continues until the LLM returns a stop reason
    ///    other than `ToolUse` or an unrecoverable error occurs.
    #[tracing::instrument(skip(self), name = "agent_loop_detailed")]
    pub async fn agent_loop_detailed(
        &mut self,
        user_turn_message: Option<Message>,
    ) -> Result<AgentLoopOutcome> {
        // `turns` is a delta over *this* activation, measured against the
        // process-lifetime counter carried on `Agent::turns_taken`.
        self.runtime.recovery_state = RecoveryState::default();
        self.runtime.interrupt_hooks_fired = false;
        self.runtime.goal_yield_reason = None;

        self.ensure_session().await?;
        #[cfg(any(test, feature = "test-support"))]
        {
            self.kernel_activations = self.kernel_activations.saturating_add(1);
        }
        self.run_kernel_activation(user_turn_message).await
    }

    pub fn with_session_start(mut self, hook: impl SessionStartFn + 'static) -> Self {
        self.hooks.push(Hook::SessionStart(Arc::new(hook)));
        self
    }

    pub fn with_user_prompt_submit(mut self, hook: impl UserPromptSubmitFn + 'static) -> Self {
        self.hooks.push(Hook::UserPromptSubmit(Arc::new(hook)));
        self
    }

    pub fn with_post_tool(mut self, hook: impl PostToolUseFn + 'static) -> Self {
        // RTK filter is opt-in — defaults to off for privacy.
        if !config::settings().tools.rtk_filter {
            return self;
        }
        self.hooks.push(Hook::PostToolUse(Arc::new(hook)));
        self
    }

    /// Registers a `PostToolUse` hook unconditionally (used for plugin
    /// command hooks; [`Self::with_post_tool`] is gated by `rtk_filter`).
    pub fn with_post_tool_hook(mut self, hook: impl PostToolUseFn + 'static) -> Self {
        self.hooks.push(Hook::PostToolUse(Arc::new(hook)));
        self
    }

    pub fn with_pre_tool(mut self, hook: impl PreToolUseFn + 'static) -> Self {
        self.hooks.push(Hook::PreToolUse(Arc::new(hook)));
        self
    }

    /// Registers a hook that answers the approval prompt.
    pub fn with_permission_request(mut self, hook: impl PermissionRequestFn + 'static) -> Self {
        self.hooks.push(Hook::PermissionRequest(Arc::new(hook)));
        self
    }

    /// Registers a hook that observes a user interrupt.
    pub fn with_interrupt(mut self, hook: impl InterruptFn + 'static) -> Self {
        self.hooks.push(Hook::Interrupt(Arc::new(hook)));
        self
    }

    pub fn with_stop(mut self, hook: impl StopFn + 'static) -> Self {
        self.hooks.push(Hook::Stop(Arc::new(hook)));
        self
    }

    pub fn with_session_end(mut self, hook: impl SessionEndFn + 'static) -> Self {
        self.hooks.push(Hook::SessionEnd(Arc::new(hook)));
        self
    }

    pub fn with_pre_compact(mut self, hook: impl PreCompactFn + 'static) -> Self {
        self.hooks.push(Hook::PreCompact(Arc::new(hook)));
        self
    }

    pub fn with_post_compact(mut self, hook: impl PostCompactFn + 'static) -> Self {
        self.hooks.push(Hook::PostCompact(Arc::new(hook)));
        self
    }

    pub fn with_post_tool_failure(mut self, hook: impl PostToolUseFailureFn + 'static) -> Self {
        self.hooks.push(Hook::PostToolUseFailure(Arc::new(hook)));
        self
    }

    pub fn with_notification(mut self, hook: impl NotificationFn + 'static) -> Self {
        self.hooks.push(Hook::Notification(Arc::new(hook)));
        self
    }

    pub fn with_task_completed(mut self, hook: impl TaskCompletedFn + 'static) -> Self {
        self.hooks.push(Hook::TaskCompleted(Arc::new(hook)));
        self
    }

    /// Runs `SessionStart` hooks once per session.
    ///
    /// Context they collect is stashed on
    /// [`AgentRuntime::pending_session_context`] rather than recorded here: this
    /// runs before [`Self::ensure_session`] has loaded persisted history, and
    /// [`Self::push_message`] would leave the context non-empty and suppress
    /// that restore.
    ///
    /// The hooks run **concurrently** — a plugin hook is a subprocess that can
    /// take seconds (the reference `basic-memory` plugin needs ~8s warm), and
    /// the hooks are independent. Results keep registration order, so the
    /// concatenated context is deterministic.
    pub async fn dispatch_session_start_hooks(&mut self) -> Result<HookControl> {
        if !self.runtime.session_start_hooks_pending {
            return Ok(HookControl::Continue);
        }
        self.runtime.session_start_hooks_pending = false;

        let collected = {
            let view = self.activation_view();
            let runs = self
                .hooks_by_type(HookTypes::SessionStart)
                .into_iter()
                .filter_map(|hook| match hook {
                    Hook::SessionStart(run) => Some(run),
                    _ => None,
                })
                .map(|run| {
                    let view = &view;
                    async move {
                        let mut context = SessionStartContext::default();
                        let control = run(view, &mut context).await?;
                        Ok::<_, anyhow::Error>((control, context.additional_contexts))
                    }
                });
            futures_util::future::join_all(runs).await
        };

        let mut stopped = None;
        for result in collected {
            let (control, contexts) = result?;
            self.runtime.pending_session_context.extend(contexts);
            // First stop in registration order wins, like the sequential run
            // this replaced; the caller decides what a stop means.
            if let HookControl::Block(reason) = control
                && stopped.is_none()
            {
                stopped = Some(reason);
            }
        }
        Ok(match stopped {
            Some(reason) => HookControl::Block(reason),
            None => HookControl::Continue,
        })
    }

    /// Reserves the id for one hook's progress line.
    ///
    /// The caller emits [`AgentUpdate::HookStatus`] twice with the same id —
    /// once when the hook starts, once when it returns — and the TUI rewrites
    /// that one row instead of leaving a stale "Loading…" behind.
    pub fn next_hook_status_id(&self) -> u64 {
        self.shared_services.next_hook_status_id()
    }

    /// Runs [`Hook::Stop`] hooks once at the outer turn boundary and returns
    /// the aggregated control. `Continue` means "stop normally"; `Block(reason)`
    /// means "continue the turn with `reason` as the next prompt" (Codex
    /// continuation-fragment semantics).
    pub async fn dispatch_stop_hooks(&mut self) -> Result<HookControl> {
        invoke_hooks!(Stop, self)
    }

    /// Runs [`Hook::TaskCompleted`] hooks once per completed user task.
    /// Observational only: a `Block` is surfaced as info and ignored.
    pub async fn dispatch_task_completed_hooks(&mut self) -> Result<()> {
        match invoke_hooks!(TaskCompleted, self)? {
            HookControl::Continue | HookControl::Allow => Ok(()),
            HookControl::Block(reason) => {
                self.emit_update(AgentUpdate::Info(format!(
                    "[TaskCompleted hook blocked] {reason}"
                )));
                Ok(())
            }
        }
    }

    /// Runs [`Hook::Interrupt`] hooks once per turn, when the user cancels.
    ///
    /// Observational: a `Block` is surfaced as info, because the turn is being
    /// abandoned anyway.
    pub async fn dispatch_interrupt_hooks(&mut self) -> Result<()> {
        if self.runtime.interrupt_hooks_fired {
            return Ok(());
        }
        self.runtime.interrupt_hooks_fired = true;
        match invoke_hooks!(Interrupt, self)? {
            HookControl::Continue | HookControl::Allow => Ok(()),
            HookControl::Block(reason) => {
                self.emit_update(AgentUpdate::Info(format!("[Interrupt hook] {reason}")));
                Ok(())
            }
        }
    }

    /// Runs [`Hook::SessionEnd`] hooks at real teardown. Observational only:
    /// a `Block` is surfaced as info and ignored because the session is ending.
    pub async fn dispatch_session_end_hooks(&mut self) -> Result<()> {
        match invoke_hooks!(SessionEnd, self)? {
            HookControl::Continue | HookControl::Allow => Ok(()),
            HookControl::Block(reason) => {
                self.emit_update(AgentUpdate::Info(format!(
                    "[SessionEnd hook blocked] {reason}"
                )));
                Ok(())
            }
        }
    }

    /// The last assistant message text, if any — exposed so Stop/SubagentStop
    /// hooks (and the plugin payload) can report what the model just produced.
    #[must_use]
    pub fn last_assistant_message(&self) -> Option<String> {
        self.runtime
            .context
            .iter()
            .rev()
            .find(|message| matches!(message.role, Role::Assistant))
            .map(|message| crate::extract_text(&message.content))
            .filter(|text| !text.is_empty())
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

    /// Whether a tool by this name is actually offered to the model.
    ///
    /// Reads the same cache the request is built from, so guidance that names a
    /// tool cannot outlive the tool itself.
    #[cfg(test)]
    fn has_tool(&self, name: &str) -> bool {
        self.cached_tool_specs.iter().any(|spec| spec.name == name)
    }

    // TODO(compact): summarization input is a crude tail-truncation to 80k
    // chars of raw JSON; consider a smarter selection (e.g. drop tool-result
    // bodies first, keep user/assistant text).
    pub async fn compact_history(&mut self, focus: Option<&str>) -> Result<()> {
        self.compact_history_with_trigger(CompactTrigger::Auto, focus)
            .await
    }

    /// Compacts with an explicit [`CompactTrigger`] (used by the recovery /
    /// manual-compact / slash-command paths so plugin `PreCompact` /
    /// `PostCompact` matchers can distinguish them).
    pub async fn compact_history_with_trigger(
        &mut self,
        trigger: CompactTrigger,
        focus: Option<&str>,
    ) -> Result<()> {
        let focus = focus.map(str::to_string);
        let request = match trigger {
            CompactTrigger::Auto => tact_agent_api::CompactionRequest::Auto { focus },
            CompactTrigger::Manual => tact_agent_api::CompactionRequest::Manual { focus },
            CompactTrigger::Recovery => tact_agent_api::CompactionRequest::Recovery { focus },
            CompactTrigger::Command => tact_agent_api::CompactionRequest::Command { focus },
        };
        self.compact_with_kernel(request).await
    }

    #[cfg(test)]
    fn build_system_prompt(&mut self) -> Result<String> {
        if let AgentSystemPrompt::Static(system_prompt) = &self.system_prompt {
            return Ok(system_prompt.clone());
        }

        let workdir = &self.tool_context.work_dir;
        let mut prompt_builder = SystemPrompt::builder();
        if matches!(&self.runtime.client, LlmProvider::OpenAiResponses(_)) {
            prompt_builder.template(responses_prompt_template());
        }
        let memory_enabled = self.agent_settings.memory_enabled;
        let memory = if memory_enabled {
            self.load_memory_prompt()?
        } else {
            String::new()
        };
        let memory_guidance = if memory_enabled {
            MEMORY_GUIDANCE.trim()
        } else {
            ""
        };
        // Resolved before the builder chain: the chain holds
        // `cached_dir_snapshot` mutably, so `&self` is not available inside it.
        let advisory = self.context_advisory();
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
                    reg.describe_available_bounded(
                        crate::skill::catalog_budget_chars(self.model_context_window()),
                        self.agent_settings.skill_description_cap,
                    )
                }
            })
            .memory(memory)
            .additional(cached_md_section(&mut self.runtime.cached_agents_md, || {
                assemble_agents_md_prompt(workdir, &self.agent_settings.instruction_sources)
            }))
            .mcp_instructions(self.mcp_router.instructions_block())
            .dynamic_context(load_dynamic_context(
                workdir,
                &mut self.runtime.cached_dir_snapshot,
                self.agent_settings.snapshot_max_items,
                &self.agent_settings.model,
                advisory,
            ))
            .memory_guidance(memory_guidance)
            .build()?;

        prompt
            .to_prompt()
            .render()
            .context("failed to render system prompt")
    }

    /// The injected memory block: the repository's `MEMORY.md` index.
    ///
    /// Only the index is injected; topic bodies are fetched on demand with the
    /// `load_memory` tool. See `crate::memory` for why.
    #[cfg(test)]
    fn load_memory_prompt(&self) -> Result<String> {
        self.tool_context
            .memory_manager
            .lock()
            .map_err(|_| anyhow::anyhow!("memory manager lock poisoned"))
            .map(|manager| manager.load_memory_index_prompt())
    }
}

/// Build the dynamic-context block that appears after `=== DYNAMIC_BOUNDARY ===`.
///
/// The directory snapshot is expensive to compute and its output must be
/// byte-for-byte identical across requests so that DeepSeek's prefix KV-cache
/// can hit.  We compute it once per session and reuse the cached string.
pub(crate) fn load_dynamic_context(
    workdir: &Path,
    cached_snapshot: &mut Option<String>,
    snapshot_limit: usize,
    model: &str,
    advisory: Option<String>,
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

    // Kept last: it is the one line here that can change between tasks, and the
    // section is already below `=== DYNAMIC_BOUNDARY ===` for exactly that
    // reason.
    if let Some(advisory) = advisory {
        lines.push(String::new());
        lines.push(advisory);
    }

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
pub(crate) fn cached_md_section(
    cached: &mut Option<String>,
    compute: impl FnOnce() -> String,
) -> String {
    if let Some(hit) = cached.as_ref() {
        return hit.clone();
    }
    let value = compute();
    *cached = Some(value.clone());
    value
}

pub(crate) fn assemble_agents_md_prompt(
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
    use sqlx::Row;
    use tact_agent_api::{AgentContentBlock, AgentMessage, AgentRole, MessageOrigin};
    use tact_llm::{
        ContentBlock, CreateMessageParams, LlmError, LlmProvider, Message, MessageContent,
        MessageKind, MockClient, ProviderConversationState, ProviderKind, Role, StopReason,
    };
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;
    use crate::store::SessionStore;
    use crate::tool::test_support::test_context;

    fn ensure_config() {
        crate::config::test_support::install_default();
    }

    fn make_text_block(content: &str) -> ContentBlock {
        ContentBlock::Text {
            text: content.to_string(),
        }
    }

    /// Hash the canonical message shape the kernel sends to the provider.
    fn wire_context_hash(messages: &[Message]) -> String {
        let canonical = messages
            .iter()
            .map(tact_llm::neutral::agent_message)
            .map(|message| tact_llm::neutral::llm_message(&message))
            .collect::<Vec<_>>();
        tact_llm::context_hash(&canonical).unwrap()
    }

    /// Blocks of a message (`MessageContent::Text` carries none).
    fn message_blocks(message: &Message) -> &[ContentBlock] {
        match &message.content {
            MessageContent::Blocks { content } => content,
            MessageContent::Text { .. } => &[],
        }
    }

    /// `(tool_use ids, ids answered by a tool_result)` in message order.
    fn tool_pairing(messages: &[Message]) -> (Vec<String>, Vec<String>) {
        let mut uses = Vec::new();
        let mut results = Vec::new();
        for block in messages.iter().flat_map(message_blocks) {
            match block {
                ContentBlock::ToolUse { id, .. } => uses.push(id.clone()),
                ContentBlock::ToolResult { tool_use_id, .. } => results.push(tool_use_id.clone()),
                _ => {}
            }
        }
        (uses, results)
    }

    /// Content of the `ToolResult` answering `tool_use_id`.
    fn tool_result_for(messages: &[Message], tool_use_id: &str) -> Option<String> {
        messages
            .iter()
            .flat_map(message_blocks)
            .find_map(|block| match block {
                ContentBlock::ToolResult {
                    tool_use_id: id,
                    content,
                    ..
                } if id == tool_use_id => Some(content.clone()),
                _ => None,
            })
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
            max_token_usage_bodies: crate::store::session_store::MAX_TOKEN_USAGE_BODIES,
            micro_compact_enabled: true,
            memory_enabled: true,
            skill_body_auto_inject: false,
            skill_dirs: Vec::new(),
            skill_description_cap: crate::skill::DEFAULT_SKILL_DESCRIPTION_CAP,
            skill_disable: Vec::new(),
            instruction_sources: crate::config::InstructionSources::default(),
            auto_memory_directory: None,
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

    /// A one-tool MCP client whose server sent `instructions`.
    fn mcp_client_with_instructions(server: &str, instructions: &str) -> crate::mcp::McpClient {
        use std::borrow::Cow;
        use std::sync::Arc;

        use rmcp::model::{Content, JsonObject, Tool as McpTool};
        use serde_json::json;

        let tool = McpTool {
            name: Cow::Borrowed("recall"),
            title: None,
            description: Some(Cow::Borrowed("Recall a note")),
            input_schema: Arc::new(JsonObject::from_iter([(
                "type".to_string(),
                json!("object"),
            )])),
            output_schema: None,
            annotations: None,
            execution: None,
            icons: None,
            meta: None,
        };
        let service = crate::mcp::MockMcpService::new(vec![tool.clone()], |_| {
            Ok(rmcp::model::CallToolResult::success(vec![Content::text(
                "ok",
            )]))
        })
        .with_instructions(instructions);
        crate::mcp::McpClient::with_service(server, vec![tool], Arc::new(service))
    }

    #[test]
    fn the_system_prompt_carries_mcp_server_instructions() {
        ensure_config();
        let mut mcp = crate::mcp::MCPToolRouter::new();
        mcp.register_client(mcp_client_with_instructions(
            "basic-memory",
            "Call recent_activity to orient yourself.",
        ));

        let mut agent = Agent::new(
            LlmProvider::Mock(MockClient::new(vec![])),
            test_context("mcp_instructions_prompt"),
            crate::tool::toolset(),
            mcp,
            crate::permission::PermissionManager::try_new(
                crate::permission::PermissionMode::Default,
            )
            .unwrap(),
            AgentSystemPrompt::Dynamic,
        );

        let prompt = agent.build_system_prompt().unwrap();
        assert!(prompt.contains("# MCP server instructions"), "{prompt}");
        assert!(prompt.contains("## basic-memory"), "{prompt}");
        assert!(
            prompt.contains("Call recent_activity to orient yourself."),
            "{prompt}"
        );
    }

    fn assert_kernel_prompt_matches_legacy(agent: &mut Agent) -> String {
        let legacy = agent.build_system_prompt().unwrap();
        let assembler = agent.kernel_prompt_assembler();
        let transcript = agent
            .runtime
            .context
            .iter()
            .map(tact_llm::neutral::agent_message)
            .collect::<Vec<_>>();
        let capabilities = tact_agent_api::ModelCapabilities {
            model: agent.agent_settings.model.clone(),
            context_window: agent.model_context_window(),
            max_output_tokens: agent.max_tokens(),
            supports_streaming: true,
            supports_native_compaction: agent.native_compaction_available(),
        };
        let session = tact_agent_api::SessionRef::new("prompt-parity");
        let state = tact_agent_api::ContextView {
            session: &session,
            transcript: &transcript,
            cursor: None,
            total_turns: 0,
            max_total_turns: None,
            last_token_total: Some(agent.runtime.last_token_total),
            pending_inputs: &[],
            capabilities: &capabilities,
            remaining_turns: None,
        };
        let kernel = assembler.build(&state).unwrap();
        assert_eq!(kernel, legacy);
        kernel
    }

    #[test]
    fn kernel_prompt_assembler_matches_legacy_chat_completions_prompt() {
        ensure_config();
        let mut agent = chat_completions_test_agent("kernel_prompt_chat_parity");
        agent.system_prompt = AgentSystemPrompt::Dynamic;
        agent.agent_settings.model_context_window = 200_000;
        agent.runtime.last_token_total = 130_000;

        let prompt = assert_kernel_prompt_matches_legacy(&mut agent);
        assert!(prompt.contains("spawn_subagent"), "{prompt}");
        assert!(
            !prompt.contains("Skill loading policy for this Responses request."),
            "{prompt}"
        );
    }

    #[test]
    fn kernel_prompt_assembler_keeps_responses_template_without_native_compaction() {
        ensure_config();
        let agent = responses_test_agent(
            "kernel_prompt_responses_parity",
            "https://example.invalid/v1",
        )
        .with_provider_kind(ProviderKind::DeepSeek);
        let mut agent = agent;
        agent.system_prompt = AgentSystemPrompt::Dynamic;

        let prompt = assert_kernel_prompt_matches_legacy(&mut agent);
        assert!(
            prompt.contains("Skill loading policy for this Responses request."),
            "{prompt}"
        );
        assert!(!agent.native_compaction_available());
    }

    #[tokio::test]
    async fn command_compaction_uses_kernel_owned_replacement_and_events() {
        ensure_config();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mock = MockClient::new(vec![(
            vec![make_text_block("kernel command summary")],
            Some(StopReason::EndTurn),
        )]);
        let mut agent = Agent::new(
            LlmProvider::Mock(mock),
            test_context("kernel_command_compaction"),
            crate::tool::toolset(),
            crate::mcp::MCPToolRouter::new(),
            crate::permission::PermissionManager::try_new(
                crate::permission::PermissionMode::Default,
            )
            .unwrap(),
            AgentSystemPrompt::Static("test".into()),
        )
        .with_ui_channel(tx);
        agent.runtime.context = vec![
            Message::new_text(Role::User, "task"),
            Message::new_text(Role::Assistant, "result"),
        ];

        agent
            .compact_history_with_trigger(CompactTrigger::Command, None)
            .await
            .unwrap();

        assert!(agent.runtime.compact_state.has_compacted);
        assert_eq!(
            agent.runtime.session_start_source,
            SessionStartSource::Compact
        );
        assert!(agent.runtime.session_start_hooks_pending);
        assert!(
            agent
                .runtime
                .context
                .iter()
                .any(|message| message.kind == MessageKind::Summary),
            "{:?}",
            agent.runtime.context
        );
        let mut updates = Vec::new();
        while let Ok(update) = rx.try_recv() {
            updates.push(update);
        }
        assert!(
            updates
                .iter()
                .any(|update| matches!(update, AgentUpdate::Info(text) if text.contains("Compaction complete (Command)."))),
            "{updates:?}"
        );
    }

    #[tokio::test]
    async fn a_server_that_grows_its_tool_list_reaches_the_next_request() {
        // The agent sends a *cached* snapshot, so a refresh has to do both
        // halves: re-list the server, and rebuild the cache. Either alone leaves
        // the model with the list the handshake advertised.
        use std::borrow::Cow;
        use std::sync::Arc;

        use rmcp::model::{Content, JsonObject, Tool as McpTool};

        let tool = |name: &'static str| McpTool {
            name: Cow::Borrowed(name),
            title: None,
            description: None,
            input_schema: Arc::new(JsonObject::new()),
            output_schema: None,
            annotations: None,
            execution: None,
            icons: None,
            meta: None,
        };

        let service = Arc::new(crate::mcp::MockMcpService::new(
            vec![tool("search")],
            |_| {
                Ok(rmcp::model::CallToolResult::success(vec![Content::text(
                    "ok",
                )]))
            },
        ));
        let mut mcp = crate::mcp::MCPToolRouter::new();
        mcp.register_client(crate::mcp::McpClient::with_service(
            "bm",
            vec![tool("search")],
            service.clone(),
        ));

        let mut agent = Agent::new(
            LlmProvider::Mock(MockClient::new(vec![])),
            test_context("mcp_tool_list_changed"),
            crate::tool::toolset(),
            mcp,
            crate::permission::PermissionManager::try_new(
                crate::permission::PermissionMode::Default,
            )
            .unwrap(),
            AgentSystemPrompt::Static("test".to_string()),
        );

        let names = |agent: &Agent| -> Vec<String> {
            agent
                .all_tool_specs()
                .into_iter()
                .map(|spec| spec.name)
                .collect()
        };
        assert!(!names(&agent).contains(&"mcp__bm__recall".to_string()));

        // The server grows and says so.
        service.set_tools(vec![tool("search"), tool("recall")]);
        service.announce_tools_changed();

        agent.refresh_mcp_tools().await;

        assert!(
            names(&agent).contains(&"mcp__bm__recall".to_string()),
            "{:?}",
            names(&agent)
        );

        // A second pass with nothing announced must not disturb it.
        agent.refresh_mcp_tools().await;
        assert!(names(&agent).contains(&"mcp__bm__recall".to_string()));
    }

    #[tokio::test]
    async fn kernel_refreshes_a_mid_turn_mcp_catalog_before_the_next_request() {
        ensure_config();
        use std::borrow::Cow;
        use std::sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        };

        use rmcp::model::{Content, JsonObject, Tool as McpTool};

        let tool = |name: &'static str| McpTool {
            name: Cow::Borrowed(name),
            title: None,
            description: None,
            input_schema: Arc::new(JsonObject::new()),
            output_schema: None,
            annotations: None,
            execution: None,
            icons: None,
            meta: None,
        };
        let service = Arc::new(crate::mcp::MockMcpService::new(
            vec![tool("search")],
            |_| {
                Ok(rmcp::model::CallToolResult::success(vec![Content::text(
                    "ok",
                )]))
            },
        ));
        let mut mcp = crate::mcp::MCPToolRouter::new();
        mcp.register_client(crate::mcp::McpClient::with_service(
            "bm",
            vec![tool("search")],
            service.clone(),
        ));

        let first_saw_recall = Arc::new(AtomicBool::new(false));
        let first_saw_recall_for_mock = first_saw_recall.clone();
        let service_for_mock = service.clone();
        let mock = MockClient::with_responder(move |request, idx| {
            let names = request
                .tools
                .as_ref()
                .into_iter()
                .flatten()
                .map(|tool| tool.name.as_str())
                .collect::<Vec<_>>();
            match idx {
                0 => {
                    first_saw_recall_for_mock
                        .store(names.contains(&"mcp__bm__recall"), Ordering::SeqCst);
                    service_for_mock.set_tools(vec![tool("search"), tool("recall")]);
                    service_for_mock.announce_tools_changed();
                    Ok((
                        vec![ContentBlock::ToolUse {
                            id: "mcp-search".into(),
                            name: "mcp__bm__search".into(),
                            input: serde_json::json!({}),
                        }],
                        Some(StopReason::ToolUse),
                        None,
                    ))
                }
                1 => {
                    assert!(
                        names.contains(&"mcp__bm__recall"),
                        "the catalog change must reach the very next request: {names:?}"
                    );
                    Ok((
                        vec![make_text_block("refreshed")],
                        Some(StopReason::EndTurn),
                        None,
                    ))
                }
                other => panic!("unexpected request {other}"),
            }
        });
        let mut agent = Agent::new(
            LlmProvider::Mock(mock),
            test_context("kernel_mcp_tool_list_changed"),
            crate::tool::toolset(),
            mcp,
            crate::permission::PermissionManager::try_new(crate::permission::PermissionMode::Auto)
                .unwrap(),
            AgentSystemPrompt::Static("test".to_string()),
        );

        let outcome = agent
            .agent_loop_detailed(Some(Message::new_text(Role::User, "search")))
            .await
            .expect("kernel activation");

        assert_eq!(outcome.stop, AgentLoopStop::Completed);
        assert!(agent.kernel_activations_for_test() >= 1);
        assert!(!first_saw_recall.load(Ordering::SeqCst));
    }

    #[test]
    fn the_resource_tools_are_offered_only_with_a_connected_server() {
        let mut mcp = crate::mcp::MCPToolRouter::new();
        mcp.register_client(mcp_client_with_instructions("basic-memory", "guidance"));

        let mut agent = Agent::new(
            LlmProvider::Mock(MockClient::new(vec![])),
            test_context("mcp_resource_tools"),
            crate::tool::toolset(),
            mcp,
            crate::permission::PermissionManager::try_new(
                crate::permission::PermissionMode::Default,
            )
            .unwrap(),
            AgentSystemPrompt::Static("test".to_string()),
        );

        let names: Vec<String> = agent
            .all_tool_specs()
            .into_iter()
            .map(|spec| spec.name)
            .collect();
        assert!(
            names.contains(&"list_mcp_resources".to_string()),
            "{names:?}"
        );
        assert!(
            names.contains(&"read_mcp_resource".to_string()),
            "{names:?}"
        );
        // The templates listing is what makes a template-addressed server
        // discoverable at all, so it must be advertised alongside the others.
        assert!(
            names.contains(&"list_mcp_resource_templates".to_string()),
            "{names:?}"
        );

        // No servers: the names must disappear, or the model gets a tool whose
        // only possible answer is "no MCP servers are connected".
        agent.mcp_router = McpCatalogHandle::new(crate::mcp::MCPToolRouter::new());
        agent.rebuild_cached_tool_specs();
        let names: Vec<String> = agent
            .all_tool_specs()
            .into_iter()
            .map(|spec| spec.name)
            .collect();
        assert!(
            !names.contains(&"list_mcp_resources".to_string()),
            "{names:?}"
        );
        assert!(
            !names.contains(&"read_mcp_resource".to_string()),
            "{names:?}"
        );
    }

    #[test]
    fn an_agent_without_mcp_servers_has_no_instructions_section() {
        ensure_config();
        let mut agent = chat_completions_test_agent("no_mcp_instructions");
        agent.system_prompt = AgentSystemPrompt::Dynamic;

        let prompt = agent.build_system_prompt().unwrap();
        assert!(!prompt.contains("# MCP server instructions"), "{prompt}");
    }

    #[test]
    fn enabled_memory_keeps_the_guidance_in_the_system_prompt() {
        ensure_config();
        let mut agent = chat_completions_test_agent("memory_enabled_prompt");
        agent.system_prompt = AgentSystemPrompt::Dynamic;

        let prompt = agent.build_system_prompt().unwrap();
        assert!(prompt.contains("# Memory guidance"), "{prompt}");
    }

    /// The injected memory section is the **index**, reachable through
    /// `load_memory` — not the bodies.
    ///
    /// Drives the real path: a memory saved by the tool, then a prompt built
    /// from the same manager, so a regression that folds bodies back into the
    /// prompt is caught here rather than in the prompt template.
    #[tokio::test]
    async fn the_system_prompt_carries_the_index_and_points_at_load_memory() {
        ensure_config();
        let context = crate::tool::test_support::test_context("memory_index_in_prompt");
        context
            .memory_manager
            .lock()
            .unwrap()
            .save_memory(
                "Release Branch",
                "the branch to cut from",
                crate::memory::MemoryType::Project,
                "BODY_SENTINEL cut releases from main.",
            )
            .unwrap();

        let mut agent = chat_completions_test_agent_for("memory_index_in_prompt", context);
        agent.system_prompt = AgentSystemPrompt::Dynamic;

        let prompt = agent.build_system_prompt().unwrap();
        assert!(prompt.contains("## Memory"), "{prompt}");
        assert!(prompt.contains("release_branch.md"), "{prompt}");
        assert!(prompt.contains("load_memory"), "{prompt}");
        assert!(
            !prompt.contains("BODY_SENTINEL"),
            "bodies must not be injected: {prompt}"
        );
    }

    #[test]
    fn disabled_memory_is_absent_from_the_system_prompt() {
        ensure_config();
        let mut agent = chat_completions_test_agent("memory_disabled_prompt");
        agent.system_prompt = AgentSystemPrompt::Dynamic;
        agent.agent_settings.memory_enabled = false;

        let prompt = agent.build_system_prompt().unwrap();
        assert!(!prompt.contains("# Memory guidance"), "{prompt}");
        assert!(!prompt.contains("# Memory"), "{prompt}");
    }

    /// The advisory reaches the rendered prompt, and only when the window is
    /// actually filling — driven through the real `build_system_prompt`.
    #[test]
    fn a_filling_window_steers_the_prompt_at_subagents() {
        ensure_config();

        // Room to spare: no advisory.
        let mut roomy = chat_completions_test_agent("advisory_roomy");
        roomy.system_prompt = AgentSystemPrompt::Dynamic;
        roomy.agent_settings.model_context_window = 200_000;
        let prompt = roomy.build_system_prompt().unwrap();
        assert!(!prompt.contains("spawn_subagent"), "{prompt}");

        // Same agent, a window that is 65% full: the advisory appears.
        let mut full = chat_completions_test_agent("advisory_full");
        full.system_prompt = AgentSystemPrompt::Dynamic;
        full.agent_settings.model_context_window = 200_000;
        full.runtime.last_token_total = 130_000;
        let prompt = full.build_system_prompt().unwrap();
        assert!(prompt.contains("spawn_subagent"), "{prompt}");
        assert!(prompt.contains("60% full"), "{prompt}");

        // It lands in the dynamic section, which is the one allowed to change
        // between tasks without invalidating the cached prefix.
        let (_, dynamic) = prompt
            .split_once("=== DYNAMIC_BOUNDARY ===")
            .expect("template emits the boundary");
        assert!(dynamic.contains("spawn_subagent"), "{dynamic}");
    }

    /// The advice names `spawn_subagent`, so it must not appear when that tool
    /// is not offered — a prompt pointing at a tool the model does not have is
    /// worse than silence.
    #[test]
    fn a_full_window_stays_quiet_when_subagents_are_not_offered() {
        ensure_config();
        let mut agent = chat_completions_test_agent("advisory_no_subagent");
        agent.system_prompt = AgentSystemPrompt::Dynamic;
        agent.agent_settings.model_context_window = 200_000;
        agent.runtime.last_token_total = 130_000;
        // The premise of the test: a full window would otherwise trigger.
        assert!(crate::compact::context_pressure_advisory(130_000, 200_000));
        assert!(
            agent.has_tool("spawn_subagent"),
            "fixture must start with it"
        );

        // Make the premise of *this* test explicit: the same full window, with
        // the tool removed from the set the request is built from.
        agent
            .cached_tool_specs
            .retain(|spec| spec.name != "spawn_subagent");
        assert!(!agent.has_tool("spawn_subagent"));

        let prompt = agent.build_system_prompt().unwrap();
        assert!(!prompt.contains("spawn_subagent"), "{prompt}");
        assert!(!prompt.contains("Context window is over"), "{prompt}");
    }

    fn chat_completions_test_agent(context_name: &str) -> Agent {
        chat_completions_test_agent_for(context_name, test_context(context_name))
    }

    #[test]
    fn facade_state_guard_restores_every_owned_sidecar_field() {
        ensure_config();
        let mut agent = chat_completions_test_agent("facade_state_guard");
        agent.runtime.context = vec![Message::new_text(Role::User, "before")];
        agent.turns_taken = 3;
        agent.runtime.last_token_total = 7;
        agent.runtime.first_message_db_id = 11;
        agent.runtime.last_message_db_id = 20;
        agent.runtime.llm_call_last_message_id = 19;
        agent.runtime.compact_state.has_compacted = true;
        agent.runtime.compact_state.last_summary = Some("summary".into());
        agent.runtime.compact_state.recent_files = vec!["src/lib.rs".into()];
        agent.runtime.recovery_state.transport_attempts = 2;
        agent.runtime.recovery_state.compact_attempts = 1;
        agent.runtime.recovery_state.continuation_attempts = 3;
        agent.runtime.goal_yield_reason = Some(GoalYieldReason::NeedsUser);
        agent.max_turns_reached = true;
        agent.tool_use_counter = 9;
        agent.runtime.interrupt_hooks_fired = true;
        agent.runtime.session_start_hooks_pending = true;
        agent.runtime.session_start_source = SessionStartSource::Resume;
        agent.runtime.pending_session_context =
            vec![HookContextChunk::new(Some("startup"), "initial")];
        agent.runtime.cached_dir_snapshot = Some("dirs".into());
        agent.runtime.cached_agents_md = Some("agents".into());

        let expected_compact_state = agent.runtime.compact_state.clone();
        let receipts = FacadeReceiptTracker::new(agent.facade_receipts());
        let mut guard = FacadeStateGuard::take(&mut agent, receipts.clone());
        {
            let state = guard.state_mut();
            state.total_turns = 4;
            state.last_token_total = Some(42);
            state
                .transcript
                .push(tact_llm::neutral::agent_message(&Message::new_text(
                    Role::Assistant,
                    "after",
                )));
        }
        receipts.record_append(
            &tact_llm::neutral::agent_message(&Message::new_text(Role::Assistant, "after")),
            tact_agent_api::MessageRange { start: 21, end: 21 },
        );
        guard.finish_activation(
            false,
            SessionStartSource::Compact,
            vec![HookContextChunk::new(Some("request"), "remaining")],
            expected_compact_state,
            PromptCaches {
                directory_snapshot: Some("dirs-next".into()),
                agents_md: Some("agents-next".into()),
            },
        );
        guard.restore();
        drop(guard);

        assert_eq!(agent.turns_taken, 4);
        assert_eq!(agent.runtime.last_token_total, 42);
        assert_eq!(agent.runtime.context.len(), 2);
        assert_eq!(agent.runtime.first_message_db_id, 11);
        assert_eq!(agent.runtime.last_message_db_id, 21);
        assert_eq!(agent.runtime.llm_call_last_message_id, 20);
        assert!(agent.runtime.compact_state.has_compacted);
        assert_eq!(
            agent.runtime.compact_state.last_summary.as_deref(),
            Some("summary")
        );
        assert_eq!(
            agent.runtime.compact_state.recent_files,
            ["src/lib.rs".to_string()]
        );
        assert_eq!(agent.runtime.recovery_state.transport_attempts, 2);
        assert_eq!(agent.runtime.recovery_state.compact_attempts, 1);
        assert_eq!(agent.runtime.recovery_state.continuation_attempts, 3);
        assert_eq!(
            agent.runtime.goal_yield_reason,
            Some(GoalYieldReason::NeedsUser)
        );
        assert!(agent.max_turns_reached);
        assert_eq!(agent.tool_use_counter, 9);
        assert!(agent.runtime.interrupt_hooks_fired);
        assert!(!agent.runtime.session_start_hooks_pending);
        // `finish_activation` carries the compactor's post-run source back to
        // the facade; a successful replacement intentionally moves it to
        // Compact instead of restoring the pre-activation Resume value.
        assert_eq!(
            agent.runtime.session_start_source,
            SessionStartSource::Compact
        );
        assert_eq!(
            agent.runtime.pending_session_context,
            [HookContextChunk::new(Some("request"), "remaining")]
        );
        assert_eq!(
            agent.runtime.cached_dir_snapshot.as_deref(),
            Some("dirs-next")
        );
        assert_eq!(
            agent.runtime.cached_agents_md.as_deref(),
            Some("agents-next")
        );
    }

    #[test]
    fn facade_state_guard_restores_on_unwind_and_keeps_persisted_ranges() {
        ensure_config();
        let mut agent = chat_completions_test_agent("facade_state_unwind");
        agent.runtime.context = vec![Message::new_text(Role::User, "before")];
        agent.turns_taken = 2;
        agent.runtime.first_message_db_id = 3;
        agent.runtime.last_message_db_id = 4;
        agent.runtime.llm_call_last_message_id = 4;
        agent.tool_use_counter = 5;

        let receipts = FacadeReceiptTracker::new(agent.facade_receipts());
        let mut guard = FacadeStateGuard::take(&mut agent, receipts.clone());
        guard.state_mut().transcript.push(AgentMessage::text(
            AgentRole::Assistant,
            MessageOrigin::Assistant,
            "persisted before panic",
        ));
        receipts.record_append(
            &AgentMessage::text(
                AgentRole::Assistant,
                MessageOrigin::Assistant,
                "persisted before panic",
            ),
            tact_agent_api::MessageRange { start: 5, end: 5 },
        );
        drop(guard);

        assert_eq!(agent.runtime.context.len(), 2);
        assert_eq!(agent.turns_taken, 2);
        assert_eq!(agent.tool_use_counter, 5);
        assert_eq!(agent.runtime.first_message_db_id, 3);
        assert_eq!(agent.runtime.last_message_db_id, 5);
        assert_eq!(agent.runtime.llm_call_last_message_id, 4);
    }

    /// The same agent, but built over a caller-supplied context so a test can
    /// populate the memory manager before the prompt is assembled.
    fn chat_completions_test_agent_for(
        context_name: &str,
        context: crate::tool::ToolContext,
    ) -> Agent {
        Agent::new(
            LlmProvider::Mock(MockClient::new(vec![])),
            context,
            crate::tool::toolset(),
            crate::mcp::MCPToolRouter::new(),
            crate::permission::PermissionManager::try_new(
                crate::permission::PermissionMode::Default,
            )
            .unwrap(),
            AgentSystemPrompt::Static(context_name.to_string()),
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
                matches!(u, AgentUpdate::Info(msg) if msg.contains("[compact continue 1/5]"))
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
        tool_context.events = UpdateSink::for_ui(tx);

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
        tool_context.events = UpdateSink::for_ui(tx);

        // Every call is truncated: MAX_COMPACT_SUMMARY_ATTEMPTS (5)
        // continuations run, then the partial summary is accepted as
        // best-effort instead of failing the whole compaction.
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
            (
                vec![make_text_block("partial five")],
                Some(StopReason::MaxTokens),
            ),
            (
                vec![make_text_block("partial six")],
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

    #[test]
    fn compact_summary_server_default_effort_tiers() {
        use tact_llm::ProviderKind;
        // Only providers that reason by **server default** when the summary
        // request omits effort get headroom: DeepSeek and Kimi K3 default
        // thinking ON + effort high server-side, everyone else needs none.
        assert_eq!(
            compact_summary_server_default_effort(&ProviderKind::OpenAi),
            None
        );
        assert_eq!(
            compact_summary_server_default_effort(&ProviderKind::Anthropic),
            None
        );
        assert_eq!(
            compact_summary_server_default_effort(&ProviderKind::DeepSeek),
            Some(tact_llm::OpenAiReasoningEffort::High)
        );
        assert_eq!(
            compact_summary_server_default_effort(&ProviderKind::Kimi),
            Some(tact_llm::OpenAiReasoningEffort::High)
        );
        assert_eq!(
            compact_summary_server_default_effort(&ProviderKind::Custom("other".to_string())),
            None
        );
    }

    #[test]
    fn compact_effort_reserve_bucket_tiers() {
        use tact_llm::OpenAiReasoningEffort as E;
        // Absolute token buckets, independent of the summary text budget.
        assert_eq!(compact_effort_reserve_tokens(E::None), 0);
        assert_eq!(compact_effort_reserve_tokens(E::Minimal), 2_000);
        assert_eq!(compact_effort_reserve_tokens(E::Low), 2_000);
        assert_eq!(compact_effort_reserve_tokens(E::Medium), 4_000);
        assert_eq!(compact_effort_reserve_tokens(E::High), 8_000);
        assert_eq!(compact_effort_reserve_tokens(E::Xhigh), 16_000);
        assert_eq!(compact_effort_reserve_tokens(E::Max), 16_000);
    }

    #[test]
    fn effort_folds_to_what_deepseek_actually_runs() {
        use tact_llm::OpenAiReasoningEffort as E;
        // Documented compatibility folds (DeepSeek only, per the thinking-mode
        // guide): the request is forwarded verbatim, the budget is not.
        for (requested, actual) in [
            (E::Minimal, E::Low),
            (E::Medium, E::High),
            (E::Xhigh, E::High),
        ] {
            assert_eq!(
                effort_after_provider_fold(&ProviderKind::DeepSeek, requested),
                actual,
                "{requested} must be budgeted as {actual} on DeepSeek"
            );
        }
        // Real tiers pass through untouched.
        for effort in [E::None, E::Low, E::High, E::Max] {
            assert_eq!(
                effort_after_provider_fold(&ProviderKind::DeepSeek, effort),
                effort
            );
        }
        // No other kind documents a fold; OpenAI's enum is real per model.
        for effort in [E::Minimal, E::Medium, E::Xhigh] {
            assert_eq!(
                effort_after_provider_fold(&ProviderKind::OpenAi, effort),
                effort
            );
            assert_eq!(
                effort_after_provider_fold(&ProviderKind::Kimi, effort),
                effort
            );
        }
    }

    #[test]
    fn next_compaction_reserve_never_shrinks() {
        // A small observed reasoning must not shrink an already-large reserve.
        assert_eq!(next_compaction_reserve(0, 2_000, 4_000, 100), 4_000);
    }

    #[test]
    fn next_compaction_reserve_caps_growth_at_double() {
        // Even a huge observed reasoning grows the room by at most 2x per step.
        assert_eq!(next_compaction_reserve(0, 2_000, 4_000, 1_000_000), 8_000);
        // Within the cap, the target covers the observed reasoning plus headroom.
        assert_eq!(next_compaction_reserve(1_500, 2_000, 0, 1_800), 2_250);
    }

    #[test]
    fn next_compaction_reserve_keeps_minimum_headroom() {
        // No effort reserve and no history: start from a quarter of the text
        // budget so a text-only overrun still gets more room.
        assert_eq!(next_compaction_reserve(0, 2_000, 0, 0), 1_000);
        // The effort-implied reserve wins when it is larger.
        assert_eq!(next_compaction_reserve(1_500, 2_000, 0, 0), 3_000);
        // Small observed reasoning is floored at the minimum headroom.
        assert_eq!(next_compaction_reserve(0, 2_000, 0, 400), 500);
    }

    #[test]
    fn compact_summary_effort_ladder_per_provider() {
        use tact_llm::ProviderKind;
        let high = Some(tact_llm::OpenAiReasoningEffort::High);
        let low = tact_llm::OpenAiReasoningEffort::Low;
        let none = tact_llm::OpenAiReasoningEffort::None;

        // Stage 0 always inherits the session effort.
        assert_eq!(compact_summary_effort(&ProviderKind::OpenAi, high, 0), high);
        assert_eq!(
            compact_summary_effort(&ProviderKind::DeepSeek, None, 0),
            None
        );

        // Stage 1+ minimizes: DeepSeek / Kimi K3 drop to `low`.
        assert_eq!(
            compact_summary_effort(&ProviderKind::DeepSeek, None, 1),
            Some(low)
        );
        assert_eq!(
            compact_summary_effort(&ProviderKind::Kimi, high, 2),
            Some(low)
        );
        // OpenAI reasoning models send `none`; without a configured effort the
        // field stays omitted (the model may not support it).
        assert_eq!(
            compact_summary_effort(&ProviderKind::OpenAi, high, 1),
            Some(none)
        );
        assert_eq!(compact_summary_effort(&ProviderKind::OpenAi, None, 1), None);
        // Anthropic / unknown keep omitting the field.
        assert_eq!(
            compact_summary_effort(&ProviderKind::Anthropic, None, 1),
            None
        );
    }

    #[tokio::test]
    async fn local_compact_inherits_session_effort() {
        ensure_config();
        let context = test_context("local_compact_inherits_effort");
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let tool_context = context;

        // Capture the summarizer request so we can assert the first attempt
        // inherits the session's reasoning effort while never enabling a
        // Claude-style thinking budget.
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::<CreateMessageParams>::new()));
        let seen_arc = seen.clone();
        let mock = MockClient::with_responder(move |request, _idx| {
            seen_arc.lock().unwrap().push(request.clone());
            Ok((
                vec![make_text_block("reasoning-aware summary")],
                Some(StopReason::EndTurn),
                None,
            ))
        });

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
        .with_provider_kind(tact_llm::ProviderKind::OpenAi)
        .with_ui_channel(tx);
        // High effort + a Claude-style thinking budget on a 128k window: the
        // effort is inherited, the budget is not.
        agent.agent_settings.reasoning_effort = Some(tact_llm::OpenAiReasoningEffort::High);
        agent.agent_settings.thinking_budget = 4096;
        agent.agent_settings.model_context_window = 128_000;
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
            .expect("compact without thinking must succeed");

        let requests = seen.lock().unwrap();
        let request = requests
            .first()
            .expect("summarizer request must be captured");
        // Text budget = min(20% × 128k, 2000) = 2000; the inherited high effort
        // adds its 8,000-token reasoning bucket, so the wire max_tokens is
        // 10,000. No Claude-style thinking budget is forwarded.
        assert_eq!(
            request.max_tokens, 10_000,
            "wire max_tokens must be text budget + the inherited effort bucket"
        );
        assert_eq!(
            request.reasoning_effort,
            Some(tact_llm::OpenAiReasoningEffort::High),
            "the first attempt must inherit the configured reasoning effort"
        );
        assert!(
            request.thinking.is_none(),
            "compact summary must not forward a Claude-style thinking budget"
        );

        let context_text = serde_json::to_string(&agent.runtime.context).unwrap_or_default();
        assert!(
            context_text.contains("reasoning-aware summary"),
            "rebuilt context must contain the summary: {context_text}"
        );

        // A successful first attempt (0 continuations) must still log its
        // envelope, and the printed `max_tokens` must be the wire value split
        // into its text and reasoning parts.
        drop(requests);
        let mut updates = Vec::new();
        while let Ok(update) = rx.try_recv() {
            updates.push(update);
        }
        let infos: Vec<&str> = updates
            .iter()
            .filter_map(|u| match u {
                AgentUpdate::Info(msg) => Some(msg.as_str()),
                _ => None,
            })
            .collect();
        assert!(
            infos.iter().any(|msg| msg.starts_with(
                "[compact summary 1/6] request model=mock-model max_tokens=10000 (text 2000 + reasoning 8000), reasoning_effort=high, input "
            ) && msg.ends_with(" chars")),
            "first attempt must log its request envelope: {infos:?}"
        );
        assert!(
            infos.iter().any(|msg| msg
                .starts_with("[compact summary 1/6] response stop=end_turn, no usage reported")),
            "first attempt must log its response even without truncation: {infos:?}"
        );
    }

    #[test]
    fn compact_summary_envelope_ceiling_leaves_room_for_the_instructions() {
        // No window → no ceiling.
        assert_eq!(compact_summary_envelope_ceiling(0), u32::MAX);
        // A real window keeps the 10% headroom plus the fixed instructions.
        let instructions = approx_text_tokens(COMPACT_SUMMARY_INSTRUCTIONS) as u32;
        assert_eq!(
            compact_summary_envelope_ceiling(128_000),
            128_000 - 12_800 - instructions
        );
        assert!(compact_summary_envelope_ceiling(500_000) > 65_536);
        // A window too small for its own instructions cannot raise the floor.
        assert_eq!(compact_summary_envelope_ceiling(100), 0);
    }

    #[tokio::test]
    async fn local_compact_envelope_is_at_least_the_configured_output_budget() {
        ensure_config();
        let context = test_context("local_compact_envelope_floor");
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();

        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::<CreateMessageParams>::new()));
        let seen_arc = seen.clone();
        let mock = MockClient::with_responder(move |request, _idx| {
            seen_arc.lock().unwrap().push(request.clone());
            Ok((
                vec![make_text_block("floored summary")],
                Some(StopReason::EndTurn),
                None,
            ))
        });

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
        .with_provider_kind(tact_llm::ProviderKind::DeepSeek)
        .with_ui_channel(tx);
        // DeepSeek reasons by server default, so its envelope is matter of
        // billing: the computed value is text 2,000 + the high bucket 8,000 =
        // 10,000, which a reasoning-inside-`max_tokens` provider can spend
        // entirely on thinking. The configured output budget floors it.
        agent.agent_settings.max_tokens = 65_536;
        agent
            .runtime
            .context
            .push(Message::new_text(Role::User, "first turn"));

        agent
            .compact_history(None)
            .await
            .expect("compact with a floored envelope must succeed");

        let requests = seen.lock().unwrap();
        let request = requests
            .first()
            .expect("summarizer request must be captured");
        assert_eq!(
            request.max_tokens, 65_536,
            "the summarizer envelope must never be smaller than [agent] max_tokens"
        );
    }

    #[tokio::test]
    async fn local_compact_folds_deepseek_effort_for_the_reserve() {
        ensure_config();
        let context = test_context("local_compact_deepseek_fold");
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();

        let mock = MockClient::new(vec![(
            vec![make_text_block("deepseek summary")],
            Some(StopReason::EndTurn),
        )]);

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
        .with_provider_kind(tact_llm::ProviderKind::DeepSeek)
        .with_ui_channel(tx);
        // DeepSeek accepts `medium` but folds it to `high` server-side, so the
        // reserve must be the high bucket while the wire value stays verbatim.
        agent.agent_settings.reasoning_effort = Some(tact_llm::OpenAiReasoningEffort::Medium);
        agent.agent_settings.model_context_window = 128_000;
        agent
            .runtime
            .context
            .push(Message::new_text(Role::User, "first turn"));

        agent
            .compact_history(None)
            .await
            .expect("compact with a folded effort must succeed");

        let mut infos = Vec::new();
        while let Ok(update) = rx.try_recv() {
            if let AgentUpdate::Info(message) = update {
                infos.push(message);
            }
        }
        let envelope = infos
            .iter()
            .find(|msg| msg.starts_with("[compact summary 1/6] request"))
            .expect("the first attempt logs its envelope");
        assert!(
            envelope.contains("(text 2000 + reasoning 8000)"),
            "the reserve must be sized from the effort DeepSeek actually runs: {envelope}"
        );
        assert!(
            envelope.contains("reasoning_effort=medium"),
            "the configured value is still forwarded verbatim: {envelope}"
        );
    }

    #[tokio::test]
    async fn local_compact_keeps_server_default_reasoning_reserve() {
        ensure_config();
        let context = test_context("local_compact_server_default_reserve");
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let mut tool_context = context;
        tool_context.events = UpdateSink::for_ui(tx);

        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::<CreateMessageParams>::new()));
        let seen_arc = seen.clone();
        let mock = MockClient::with_responder(move |request, _idx| {
            seen_arc.lock().unwrap().push(request.clone());
            Ok((
                vec![make_text_block("deepseek summary")],
                Some(StopReason::EndTurn),
                None,
            ))
        });

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
        .with_provider_kind(tact_llm::ProviderKind::DeepSeek);
        // DeepSeek reasons at effort high by server default even without an
        // explicit effort, so the summary call reserves the high bucket.
        agent.agent_settings.reasoning_effort = None;
        agent.agent_settings.model_context_window = 128_000;
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
            .expect("compact with server-default reserve must succeed");

        let requests = seen.lock().unwrap();
        let request = requests
            .first()
            .expect("summarizer request must be captured");
        // Text budget 2000 + the high effort bucket (8000) = 10_000, so
        // DeepSeek's forced reasoning never starves the summary text; but no
        // effort and no thinking budget are sent.
        assert_eq!(
            request.max_tokens, 10_000,
            "DeepSeek keeps the server-default (high) reasoning bucket"
        );
        assert_eq!(request.reasoning_effort, None);
        assert!(request.thinking.is_none());

        let context_text = serde_json::to_string(&agent.runtime.context).unwrap_or_default();
        assert!(
            context_text.contains("deepseek summary"),
            "rebuilt context must contain the summary: {context_text}"
        );
    }

    #[tokio::test]
    async fn local_compact_omits_thinking_for_anthropic() {
        ensure_config();
        let context = test_context("local_compact_anthropic_no_thinking");
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let mut tool_context = context;
        tool_context.events = UpdateSink::for_ui(tx);

        // Capture the summarizer request so we can assert no Claude-style
        // thinking budget is forwarded even though one is configured.
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::<CreateMessageParams>::new()));
        let seen_arc = seen.clone();
        let mock = MockClient::with_responder(move |request, _idx| {
            seen_arc.lock().unwrap().push(request.clone());
            Ok((
                vec![make_text_block("plain summary")],
                Some(StopReason::EndTurn),
                None,
            ))
        });

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
        .with_provider_kind(tact_llm::ProviderKind::Anthropic);
        // Claude-style thinking budget (8k) far above the capped summarizer
        // output budget (2k): the compact call must not forward it.
        agent.agent_settings.reasoning_effort = None;
        agent.agent_settings.thinking_budget = 8_000;
        agent.agent_settings.model_context_window = 128_000;
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
            .expect("compact without thinking must succeed");

        let requests = seen.lock().unwrap();
        let request = requests
            .first()
            .expect("summarizer request must be captured");
        // Wire max_tokens = 2000 (Anthropic gets no reasoning reserve); no
        // thinking budget may be forwarded.
        assert_eq!(
            request.max_tokens, 2000,
            "Anthropic text budget stays at the classic cap"
        );
        assert!(
            request.thinking.is_none(),
            "summarizer must not receive a thinking budget"
        );
        assert_eq!(
            request.reasoning_effort, None,
            "Anthropic must not receive a reasoning effort"
        );

        let context_text = serde_json::to_string(&agent.runtime.context).unwrap_or_default();
        assert!(
            context_text.contains("plain summary"),
            "rebuilt context must contain the summary: {context_text}"
        );
    }

    #[tokio::test]
    async fn local_compact_bails_when_window_too_small_for_summary_prompt() {
        ensure_config();
        let context = test_context("local_compact_too_small_window");
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let mut tool_context = context;
        tool_context.events = UpdateSink::for_ui(tx);

        let mock = MockClient::new(vec![(
            vec![make_text_block("unused summary")],
            Some(StopReason::EndTurn),
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
        );
        // Tiny window: output + 10% headroom reservation leaves so little
        // input room that the fixed summarizer prompt cannot fit → compaction
        // must bail instead of silently over-reserving the window. (No
        // thinking budget is involved anymore — the compact call never thinks.)
        agent.agent_settings.model_context_window = 100;
        agent
            .runtime
            .context
            .push(Message::new_text(Role::User, "first turn"));

        let err = agent
            .compact_history(None)
            .await
            .expect_err("too-small window must bail from compaction");
        assert!(
            err.to_string().contains("too small"),
            "expected a too-small-window error, got: {err}"
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
        // Provider state hashes the neutral message representation that the
        // kernel sends. A Message::new_text round-trips as one text block, so
        // hashing the original Text shorthand would make this fixture stale
        // before the usage-failure path is reached.
        let old_wire_messages = old_messages
            .iter()
            .map(tact_llm::neutral::agent_message)
            .map(|message| tact_llm::neutral::llm_message(&message))
            .collect::<Vec<_>>();
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
                logical_context_hash: tact_llm::context_hash(&old_wire_messages).unwrap(),
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
            agent
                .runtime
                .stats
                .read()
                .expect("session stats lock poisoned")
                .compactions,
            0,
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
            max_token_usage_bodies: crate::store::session_store::MAX_TOKEN_USAGE_BODIES,
            micro_compact_enabled: true,
            memory_enabled: true,
            skill_body_auto_inject: false,
            skill_dirs: Vec::new(),
            skill_description_cap: crate::skill::DEFAULT_SKILL_DESCRIPTION_CAP,
            skill_disable: Vec::new(),
            instruction_sources: crate::config::InstructionSources::default(),
            auto_memory_directory: None,
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
            max_token_usage_bodies: crate::store::session_store::MAX_TOKEN_USAGE_BODIES,
            micro_compact_enabled: true,
            memory_enabled: true,
            skill_body_auto_inject: false,
            skill_dirs: Vec::new(),
            skill_description_cap: crate::skill::DEFAULT_SKILL_DESCRIPTION_CAP,
            skill_disable: Vec::new(),
            instruction_sources: crate::config::InstructionSources::default(),
            auto_memory_directory: None,
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
            wire_context_hash(&agent.runtime.context),
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
    async fn kernel_activation_uses_direct_facade_provider_state_mutations() {
        ensure_config();
        let server = MockServer::start().await;
        let completed = serde_json::json!({
            "type": "response.completed",
            "sequence_number": 1,
            "response": {
                "id": "resp_direct_state",
                "object": "response",
                "created_at": 1,
                "completed_at": 2,
                "status": "completed",
                "model": "gpt-5",
                "output": [{
                    "type": "message",
                    "id": "msg_direct_state",
                    "role": "assistant",
                    "status": "completed",
                    "content": [{
                        "type": "output_text",
                        "annotations": [],
                        "logprobs": null,
                        "text": "done"
                    }]
                }],
                "usage": {
                    "input_tokens": 2,
                    "input_tokens_details": { "cached_tokens": 0 },
                    "output_tokens": 1,
                    "output_tokens_details": { "reasoning_tokens": 0 },
                    "total_tokens": 3
                }
            }
        });
        Mock::given(method("POST"))
            .and(path("/responses"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .set_body_string(format!("data: {completed}\n\n")),
            )
            .expect(1)
            .mount(&server)
            .await;

        let mut agent = responses_test_agent("direct_provider_state", &server.uri());
        agent.runtime.provider_state = Some(ProviderConversationState::OpenAiResponses(
            tact_llm::ResponsesConversationState {
                version: 1,
                provider: "openai_responses".to_string(),
                base_url: server.uri(),
                model: agent.agent_settings.model.clone(),
                input_items: vec![serde_json::json!({
                    "type": "message",
                    "role": "user",
                    "content": [{
                        "type": "input_text",
                        "text": "DIRECT PROVIDER STATE"
                    }]
                })],
                compaction_id: None,
                is_compacted: false,
                logical_message_count: 0,
                logical_context_hash: tact_llm::context_hash(&[]).unwrap(),
            },
        ));

        agent
            .agent_loop_detailed(Some(Message::new_text(Role::User, "new turn")))
            .await
            .expect("kernel activation with direct provider state completes");
        assert!(agent.kernel_activations_for_test() >= 1);

        let requests = server.received_requests().await.expect("received requests");
        assert_eq!(requests.len(), 1);
        let body: serde_json::Value =
            serde_json::from_slice(&requests[0].body).expect("request body is JSON");
        assert!(
            body["input"]
                .as_array()
                .is_some_and(|items| items.iter().any(|item| {
                    item["content"].as_array().is_some_and(|content| {
                        content
                            .iter()
                            .any(|part| part["text"].as_str() == Some("DIRECT PROVIDER STATE"))
                    })
                })),
            "the directly mutated provider baseline must be sent: {body}"
        );
    }

    #[tokio::test]
    async fn cancellation_restores_direct_facade_context_and_provider_state() {
        ensure_config();
        const DIRECT_CONTEXT: &str = "DIRECT CANCEL CONTEXT";
        let mut agent = chat_completions_test_agent("cancel_restores_facade_state");
        let provider_state =
            ProviderConversationState::OpenAiResponses(tact_llm::ResponsesConversationState {
                version: 1,
                provider: "openai_responses".to_string(),
                base_url: "https://api.openai.com/v1".to_string(),
                model: "mock-model".to_string(),
                input_items: vec![serde_json::json!({"type": "message", "id": "cancel"})],
                compaction_id: Some("cancel-state".to_string()),
                is_compacted: true,
                logical_message_count: 0,
                logical_context_hash: tact_llm::context_hash(&[]).unwrap(),
            });
        agent.runtime.context = vec![Message::new_text(Role::User, DIRECT_CONTEXT)];
        agent.runtime.provider_state = Some(provider_state);
        agent
            .runtime
            .cancel_flag
            .store(true, std::sync::atomic::Ordering::SeqCst);

        let outcome = agent
            .agent_loop_detailed(Some(Message::new_text(Role::User, "cancel me")))
            .await
            .expect("kernel cancellation returns Ok");

        assert_eq!(outcome.stop, AgentLoopStop::Cancelled);
        assert!(agent.kernel_activations_for_test() >= 1);
        assert!(
            agent
                .runtime
                .context
                .iter()
                .any(|message| crate::extract_text(&message.content) == DIRECT_CONTEXT)
        );
        let Some(ProviderConversationState::OpenAiResponses(state)) = &agent.runtime.provider_state
        else {
            panic!("direct provider state must survive cancellation");
        };
        assert_eq!(state.compaction_id.as_deref(), Some("cancel-state"));
    }

    #[tokio::test]
    async fn kernel_error_restores_direct_facade_state_without_mid_run_fallback() {
        ensure_config();
        const DIRECT_CONTEXT: &str = "DIRECT ERROR CONTEXT";
        let mock = MockClient::with_responder(|_request, _idx| {
            Err(tact_llm::LlmError::Mock("forced kernel failure".into()))
        });
        let mut agent = Agent::new(
            LlmProvider::Mock(mock),
            test_context("error_restores_facade_state"),
            crate::tool::toolset(),
            crate::mcp::MCPToolRouter::new(),
            crate::permission::PermissionManager::try_new(
                crate::permission::PermissionMode::Default,
            )
            .unwrap(),
            AgentSystemPrompt::Static("test".to_string()),
        );
        let provider_state =
            ProviderConversationState::OpenAiResponses(tact_llm::ResponsesConversationState {
                version: 1,
                provider: "openai_responses".to_string(),
                base_url: "https://api.openai.com/v1".to_string(),
                model: "mock-model".to_string(),
                input_items: vec![serde_json::json!({"type": "message", "id": "error"})],
                compaction_id: Some("error-state".to_string()),
                is_compacted: true,
                logical_message_count: 0,
                logical_context_hash: tact_llm::context_hash(&[]).unwrap(),
            });
        agent.runtime.context = vec![Message::new_text(Role::User, DIRECT_CONTEXT)];
        agent.runtime.provider_state = Some(provider_state);

        let error = agent
            .agent_loop_detailed(Some(Message::new_text(Role::User, "fail")))
            .await
            .expect_err("kernel errors must surface");
        assert!(format!("{error:#}").contains("forced kernel failure"));
        assert!(agent.kernel_activations_for_test() >= 1);
        assert!(
            agent
                .runtime
                .context
                .iter()
                .any(|message| crate::extract_text(&message.content) == DIRECT_CONTEXT)
        );
        let Some(ProviderConversationState::OpenAiResponses(state)) = &agent.runtime.provider_state
        else {
            panic!("direct provider state must survive the error");
        };
        assert_eq!(state.compaction_id.as_deref(), Some("error-state"));
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
        tool_context.events = UpdateSink::default();
        let mut agent = Agent::new(
            LlmProvider::OpenAiResponses(
                tact_llm::openai::responses::OpenAiResponsesAdapter::new(
                    "test-key",
                    server.uri(),
                    None,
                )
                // This wiremock serves an OpenAI-shaped protocol stream
                // (reasoning + encrypted content + item ids), so reasoning
                // replay must stay enabled even though the mock base URL is
                // not api.openai.com.
                .with_replay_prior_reasoning(true),
            ),
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
            wire_context_hash(&agent.runtime.context)
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
    async fn kernel_activation_uses_direct_facade_context_mutations() {
        ensure_config();
        const DIRECT: &str = "DIRECT FACADE CONTEXT";
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let seen_for_responder = seen.clone();
        let mock = MockClient::with_responder(move |request, _idx| {
            *seen_for_responder.lock().unwrap() = request
                .messages
                .iter()
                .map(|message| crate::extract_text(&message.content))
                .collect();
            Ok((
                vec![make_text_block("done")],
                Some(StopReason::EndTurn),
                None,
            ))
        });
        let mut agent = Agent::new(
            LlmProvider::Mock(mock),
            test_context("direct_context_mutation"),
            crate::tool::toolset(),
            crate::mcp::MCPToolRouter::new(),
            crate::permission::PermissionManager::try_new(
                crate::permission::PermissionMode::Default,
            )
            .unwrap(),
            AgentSystemPrompt::Static("test".to_string()),
        );
        agent.runtime.context = vec![Message::new_text(Role::User, DIRECT)];

        let outcome = agent
            .agent_loop_detailed(Some(Message::new_text(Role::User, "next turn")))
            .await
            .expect("kernel activation completes");
        assert!(agent.kernel_activations_for_test() >= 1);
        assert_eq!(outcome.turns, 1);

        let seen = seen.lock().unwrap();
        assert_eq!(seen.first().map(String::as_str), Some(DIRECT));
        assert!(seen.iter().any(|message| message == "next turn"));
        drop(seen);
        assert!(
            agent
                .runtime
                .context
                .iter()
                .any(|message| crate::extract_text(&message.content) == DIRECT),
            "the facade context must survive activation restore"
        );
    }

    #[tokio::test]
    async fn kernel_composition_uses_current_facade_model_and_sink_each_activation() {
        ensure_config();
        let first_calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let first_calls_for_responder = first_calls.clone();
        let first = MockClient::with_responder(move |_request, _idx| {
            first_calls_for_responder.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok((
                vec![make_text_block("first model")],
                Some(StopReason::EndTurn),
                None,
            ))
        });
        let second_calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let second_calls_for_responder = second_calls.clone();
        let second = MockClient::with_responder(move |_request, _idx| {
            second_calls_for_responder.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok((
                vec![make_text_block("second model")],
                Some(StopReason::EndTurn),
                None,
            ))
        });

        let (first_tx, mut first_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut agent = Agent::new(
            LlmProvider::Mock(first),
            test_context("current_composition"),
            crate::tool::toolset(),
            crate::mcp::MCPToolRouter::new(),
            crate::permission::PermissionManager::try_new(
                crate::permission::PermissionMode::Default,
            )
            .unwrap(),
            AgentSystemPrompt::Static("test".to_string()),
        )
        .with_ui_channel(first_tx);
        agent
            .agent_loop_detailed(Some(Message::new_text(Role::User, "one")))
            .await
            .expect("first activation");
        assert_eq!(first_calls.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert!(matches!(
            first_rx.try_recv(),
            Ok(AgentUpdate::TurnStats { .. })
        ));

        let (second_tx, mut second_rx) = tokio::sync::mpsc::unbounded_channel();
        agent.runtime.client = LlmProvider::Mock(second);
        agent = agent.with_ui_channel(second_tx);
        agent
            .agent_loop_detailed(Some(Message::new_text(Role::User, "two")))
            .await
            .expect("second activation");

        assert_eq!(
            second_calls.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "the next activation must use the replacement facade model"
        );
        assert!(
            agent
                .runtime
                .context
                .iter()
                .any(|message| crate::extract_text(&message.content) == "second model")
        );
        assert!(matches!(
            second_rx.try_recv(),
            Ok(AgentUpdate::TurnStats { .. })
        ));
        assert!(
            agent.runtime.events.has_ui(),
            "the replacement answering sink must remain available"
        );
    }

    #[tokio::test]
    async fn empty_facade_context_loads_sqlite_history_before_kernel_transfer() {
        ensure_config();
        let dir = tempfile::tempdir().unwrap();
        let store = crate::store::open_sqlite_session_store(&dir.path().join("session.db"))
            .await
            .unwrap();
        store
            .create_session("session-1", dir.path().to_str().unwrap(), "")
            .await
            .unwrap();
        let history = vec![
            Message::new_text(Role::User, "PERSISTED USER"),
            Message::new_text(Role::Assistant, "PERSISTED ASSISTANT"),
        ];
        store
            .replace_session_messages("session-1", &history)
            .await
            .unwrap();

        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let seen_for_responder = seen.clone();
        let mock = MockClient::with_responder(move |request, _idx| {
            *seen_for_responder.lock().unwrap() = request
                .messages
                .iter()
                .map(|message| crate::extract_text(&message.content))
                .collect();
            Ok((
                vec![make_text_block("done")],
                Some(StopReason::EndTurn),
                None,
            ))
        });
        let mut agent = Agent::new(
            LlmProvider::Mock(mock),
            test_context("resume_before_transfer"),
            crate::tool::toolset(),
            crate::mcp::MCPToolRouter::new(),
            crate::permission::PermissionManager::try_new(
                crate::permission::PermissionMode::Default,
            )
            .unwrap(),
            AgentSystemPrompt::Static("test".to_string()),
        )
        .with_session("session-1".to_string(), store);
        assert!(agent.runtime.context.is_empty());

        agent
            .agent_loop_detailed(Some(Message::new_text(Role::User, "new turn")))
            .await
            .expect("resumed kernel activation completes");
        assert!(agent.kernel_activations_for_test() >= 1);

        let seen = seen.lock().unwrap();
        assert!(seen.iter().any(|message| message == "PERSISTED USER"));
        assert!(seen.iter().any(|message| message == "PERSISTED ASSISTANT"));
        assert!(seen.iter().any(|message| message == "new turn"));
    }

    #[tokio::test]
    async fn agent_loop_goal_request_input_yields_without_second_model_call() {
        ensure_config();
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        };

        let mut context = test_context("agent_loop_goal_request_input_yield");
        let goal = context
            .goal_manager
            .create(crate::goal::GoalCreate {
                title: "Need user input".to_string(),
                objective: "Pause for a user answer".to_string(),
                criteria: vec![crate::goal::GoalCriterion::required_with_id(
                    "criterion-1",
                    "answer received",
                )],
            })
            .await
            .unwrap();
        context.active_goal_id = Some(goal.id.clone());

        let request_count = Arc::new(AtomicUsize::new(0));
        let request_count_for_responder = request_count.clone();
        let mock = MockClient::with_responder(move |_request, idx| {
            request_count_for_responder.fetch_add(1, Ordering::SeqCst);
            if idx == 0 {
                Ok((
                    vec![ContentBlock::ToolUse {
                        id: "goal-input-1".to_string(),
                        name: "goal_request_input".to_string(),
                        input: serde_json::json!({"question": "Which option should I use?"}),
                    }],
                    Some(StopReason::ToolUse),
                    None,
                ))
            } else {
                panic!("unexpected second request at responder index {idx}");
            }
        });

        let mut agent = Agent::new(
            LlmProvider::Mock(mock),
            context,
            crate::tool::toolset(),
            crate::mcp::MCPToolRouter::new(),
            crate::permission::PermissionManager::try_new(crate::permission::PermissionMode::Auto)
                .unwrap(),
            AgentSystemPrompt::Static("test".to_string()),
        );

        let outcome = agent
            .agent_loop_detailed(Some(Message::new_text(Role::User, "continue the goal")))
            .await
            .expect("agent_loop_detailed should yield cleanly for user input");

        assert!(agent.kernel_activations_for_test() >= 1);
        assert_eq!(
            outcome.stop,
            AgentLoopStop::GoalYield(GoalYieldReason::NeedsUser)
        );
        assert_eq!(request_count.load(Ordering::SeqCst), 1);
        assert_eq!(
            agent.runtime.goal_yield_reason,
            Some(GoalYieldReason::NeedsUser)
        );

        let events = agent
            .tool_context
            .goal_manager
            .list_events(&goal.id)
            .await
            .unwrap();
        assert!(
            events
                .iter()
                .any(|event| event.kind == crate::goal::GoalEventKind::NeedsUser),
            "needs_user event should be persisted: {events:?}"
        );
        assert_eq!(
            tool_result_for(&agent.runtime.context, "goal-input-1").as_deref(),
            Some("User input requested; yield to Runner.")
        );
    }

    /// The hard cap is checked before the model call, so the model's final
    /// permitted turn is the only one that can still persist a checkpoint.
    /// The reminder must appear there (and only there), scoped to a Goal
    /// activation — a subagent also runs under `max_turns` but has no Goal.
    #[tokio::test]
    async fn agent_loop_reminds_a_goal_activation_to_checkpoint_on_its_last_turn() {
        ensure_config();
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        };

        let mut context = test_context("agent_loop_goal_final_turn_reminder");
        let goal = context
            .goal_manager
            .create(crate::goal::GoalCreate {
                title: "Reminder".to_string(),
                objective: "Checkpoint before the cap".to_string(),
                criteria: vec![crate::goal::GoalCriterion::required_with_id(
                    "criterion-1",
                    "checkpoint written",
                )],
            })
            .await
            .unwrap();
        context.active_goal_id = Some(goal.id.clone());

        let systems = Arc::new(std::sync::Mutex::new(Vec::<Option<String>>::new()));
        let systems_for_responder = systems.clone();
        let index = Arc::new(AtomicUsize::new(0));
        let index_for_responder = index.clone();
        let mock = MockClient::with_responder(move |request, _idx| {
            systems_for_responder
                .lock()
                .unwrap()
                .push(request.system.clone());
            let call = index_for_responder.fetch_add(1, Ordering::SeqCst);
            if call == 0 {
                Ok((
                    vec![ContentBlock::ToolUse {
                        id: "checkpoint-1".to_string(),
                        name: "goal_checkpoint".to_string(),
                        input: serde_json::json!({"summary": "half done"}),
                    }],
                    Some(StopReason::ToolUse),
                    None,
                ))
            } else {
                Ok((
                    vec![make_text_block("done")],
                    Some(StopReason::EndTurn),
                    None,
                ))
            }
        });

        let mut agent = Agent::new(
            LlmProvider::Mock(mock),
            context,
            crate::tool::toolset(),
            crate::mcp::MCPToolRouter::new(),
            crate::permission::PermissionManager::try_new(crate::permission::PermissionMode::Auto)
                .unwrap(),
            AgentSystemPrompt::Static("You are a test agent.".to_string()),
        )
        .with_max_turns(Some(2));

        agent
            .agent_loop(Some(Message::new_text(Role::User, "work the goal")))
            .await
            .expect("agent_loop should finish");

        assert!(agent.kernel_activations_for_test() >= 1);
        let systems = systems.lock().unwrap();
        assert_eq!(systems.len(), 2, "expected exactly two model calls");
        let first = systems[0].as_deref().unwrap_or_default();
        assert!(
            !first.contains("goal-budget-reminder"),
            "the reminder must not appear before the final turn"
        );
        let last = systems[1].as_deref().unwrap_or_default();
        assert!(
            last.contains("goal-budget-reminder"),
            "the final Goal turn must carry the reminder:\n{last}"
        );
        assert!(
            last.contains("goal_checkpoint"),
            "the reminder must name the checkpoint tool:\n{last}"
        );
    }

    /// A `goal_request_input` that arrives alongside `stop_reason=max_tokens`
    /// is still executed before the continuation prompt: the yield wins, the
    /// tool result is persisted, and no second model request is made.
    #[tokio::test]
    async fn agent_loop_max_tokens_goal_request_input_yields_without_continuation() {
        ensure_config();
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        };

        let mut context = test_context("agent_loop_max_tokens_goal_request_input_yield");
        let goal = context
            .goal_manager
            .create(crate::goal::GoalCreate {
                title: "Need user input".to_string(),
                objective: "Pause for a user answer".to_string(),
                criteria: vec![crate::goal::GoalCriterion::required_with_id(
                    "criterion-1",
                    "answer received",
                )],
            })
            .await
            .unwrap();
        context.active_goal_id = Some(goal.id.clone());

        let request_count = Arc::new(AtomicUsize::new(0));
        let request_count_for_responder = request_count.clone();
        let mock = MockClient::with_responder(move |_request, idx| {
            request_count_for_responder.fetch_add(1, Ordering::SeqCst);
            if idx == 0 {
                Ok((
                    vec![ContentBlock::ToolUse {
                        id: "goal-max-1".to_string(),
                        name: "goal_request_input".to_string(),
                        input: serde_json::json!({"question": "Which option should I use?"}),
                    }],
                    Some(StopReason::MaxTokens),
                    None,
                ))
            } else {
                panic!("unexpected second request at responder index {idx}");
            }
        });

        let mut agent = Agent::new(
            LlmProvider::Mock(mock),
            context,
            crate::tool::toolset(),
            crate::mcp::MCPToolRouter::new(),
            crate::permission::PermissionManager::try_new(crate::permission::PermissionMode::Auto)
                .unwrap(),
            AgentSystemPrompt::Static("test".to_string()),
        );

        agent
            .agent_loop(Some(Message::new_text(Role::User, "continue the goal")))
            .await
            .expect("agent_loop should yield cleanly for user input");

        assert!(agent.kernel_activations_for_test() >= 1);
        assert_eq!(request_count.load(Ordering::SeqCst), 1);
        assert_eq!(
            agent.runtime.goal_yield_reason,
            Some(GoalYieldReason::NeedsUser)
        );
        assert_eq!(
            tool_result_for(&agent.runtime.context, "goal-max-1").as_deref(),
            Some("User input requested; yield to Runner.")
        );
    }

    #[tokio::test]
    async fn max_tokens_yield_suppresses_later_tool_waves_before_continuation() {
        ensure_config();
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        };

        let mut context = test_context("agent_loop_max_tokens_goal_yield_waves");
        let goal = context
            .goal_manager
            .create(crate::goal::GoalCreate {
                title: "Need user input".to_string(),
                objective: "Pause for a user answer".to_string(),
                criteria: vec![crate::goal::GoalCriterion::required_with_id(
                    "criterion-1",
                    "answer received",
                )],
            })
            .await
            .unwrap();
        context.active_goal_id = Some(goal.id);
        let marker = context.work_dir.join("later-wave-marker");
        let marker_for_response = marker.clone();

        let requests = Arc::new(AtomicUsize::new(0));
        let requests_for_mock = requests.clone();
        let mock = MockClient::with_responder(move |_request, idx| {
            requests_for_mock.fetch_add(1, Ordering::SeqCst);
            assert_eq!(idx, 0, "a host yield must not buy a continuation request");
            Ok((
                vec![
                    ContentBlock::ToolUse {
                        id: "goal-yield".to_string(),
                        name: "goal_request_input".to_string(),
                        input: serde_json::json!({"question": "Which path?"}),
                    },
                    ContentBlock::ToolUse {
                        id: "later-wave".to_string(),
                        name: "bash".to_string(),
                        input: serde_json::json!({
                            "command": format!(
                                "printf should-not-run > {}",
                                marker_for_response.display()
                            )
                        }),
                    },
                ],
                Some(StopReason::MaxTokens),
                None,
            ))
        });
        let mut agent = Agent::new(
            LlmProvider::Mock(mock),
            context,
            crate::tool::toolset(),
            crate::mcp::MCPToolRouter::new(),
            crate::permission::PermissionManager::try_new(crate::permission::PermissionMode::Auto)
                .unwrap(),
            AgentSystemPrompt::Static("test".to_string()),
        );

        let outcome = agent
            .agent_loop_detailed(Some(Message::new_text(Role::User, "continue the goal")))
            .await
            .expect("the truncated response should yield before continuing");

        assert_eq!(
            outcome.stop,
            AgentLoopStop::GoalYield(GoalYieldReason::NeedsUser)
        );
        assert_eq!(requests.load(Ordering::SeqCst), 1);
        assert!(
            !marker.exists(),
            "a call from a later wave must not execute after YieldToHost"
        );
        assert_eq!(
            tool_result_for(&agent.runtime.context, "later-wave").as_deref(),
            Some("Not executed: the run yielded to the host")
        );
    }

    #[tokio::test]
    async fn goal_completion_on_the_final_turn_returns_completed() {
        ensure_config();
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        };

        let mut context = test_context("kernel_goal_final_completion");
        let goal = context
            .goal_manager
            .create(crate::goal::GoalCreate {
                title: "Finish".to_string(),
                objective: "Finish on the final permitted turn".to_string(),
                criteria: vec![crate::goal::GoalCriterion::required_with_id(
                    "criterion-1",
                    "complete",
                )],
            })
            .await
            .unwrap();
        context.active_goal_id = Some(goal.id);

        let requests = Arc::new(AtomicUsize::new(0));
        let requests_for_mock = requests.clone();
        let mock = MockClient::with_responder(move |_request, idx| {
            requests_for_mock.fetch_add(1, Ordering::SeqCst);
            assert_eq!(idx, 0, "final completion must not buy another turn");
            Ok((
                vec![ContentBlock::ToolUse {
                    id: "completion-1".to_string(),
                    name: "goal_propose_completion".to_string(),
                    input: serde_json::json!({
                        "summary": "done",
                        "claims": [{"criterion_id": "criterion-1", "claim": "complete"}]
                    }),
                }],
                Some(StopReason::ToolUse),
                None,
            ))
        });
        let mut agent = Agent::new(
            LlmProvider::Mock(mock),
            context,
            crate::tool::toolset(),
            crate::mcp::MCPToolRouter::new(),
            crate::permission::PermissionManager::try_new(crate::permission::PermissionMode::Auto)
                .unwrap(),
            AgentSystemPrompt::Static("test".to_string()),
        )
        .with_max_turns(Some(1));

        let outcome = agent
            .agent_loop_detailed(Some(Message::new_text(Role::User, "finish the goal")))
            .await
            .expect("final Goal completion should complete the activation");

        assert!(agent.kernel_activations_for_test() >= 1);
        assert_eq!(outcome.stop, AgentLoopStop::Completed);
        assert_eq!(outcome.turns, 1);
        assert_eq!(outcome.tool_calls, 1);
        assert_eq!(requests.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn non_final_goal_completion_continues_the_activation() {
        ensure_config();
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        };

        let mut context = test_context("kernel_goal_non_final_completion");
        let goal = context
            .goal_manager
            .create(crate::goal::GoalCreate {
                title: "Keep going".to_string(),
                objective: "Propose completion before the final turn".to_string(),
                criteria: vec![crate::goal::GoalCriterion::required_with_id(
                    "criterion-1",
                    "complete",
                )],
            })
            .await
            .unwrap();
        context.active_goal_id = Some(goal.id);

        let requests = Arc::new(AtomicUsize::new(0));
        let requests_for_mock = requests.clone();
        let mock = MockClient::with_responder(move |_request, idx| {
            requests_for_mock.fetch_add(1, Ordering::SeqCst);
            if idx == 0 {
                Ok((
                    vec![ContentBlock::ToolUse {
                        id: "completion-early".to_string(),
                        name: "goal_propose_completion".to_string(),
                        input: serde_json::json!({
                            "summary": "proposed early",
                            "claims": [{"criterion_id": "criterion-1", "claim": "complete"}]
                        }),
                    }],
                    Some(StopReason::ToolUse),
                    None,
                ))
            } else {
                Ok((
                    vec![make_text_block("continued")],
                    Some(StopReason::EndTurn),
                    None,
                ))
            }
        });
        let mut agent = Agent::new(
            LlmProvider::Mock(mock),
            context,
            crate::tool::toolset(),
            crate::mcp::MCPToolRouter::new(),
            crate::permission::PermissionManager::try_new(crate::permission::PermissionMode::Auto)
                .unwrap(),
            AgentSystemPrompt::Static("test".to_string()),
        )
        .with_max_turns(Some(2));

        let outcome = agent
            .agent_loop_detailed(Some(Message::new_text(Role::User, "work the goal")))
            .await
            .expect("a non-final proposal should continue");

        assert!(agent.kernel_activations_for_test() >= 1);
        assert_eq!(outcome.stop, AgentLoopStop::Completed);
        assert_eq!(outcome.turns, 2);
        assert_eq!(requests.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn truncated_final_goal_turn_cannot_complete_the_goal() {
        ensure_config();
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        };

        let mut context = test_context("kernel_goal_truncated_completion");
        let goal = context
            .goal_manager
            .create(crate::goal::GoalCreate {
                title: "Truncated".to_string(),
                objective: "Do not complete from a truncated response".to_string(),
                criteria: vec![crate::goal::GoalCriterion::required_with_id(
                    "criterion-1",
                    "complete",
                )],
            })
            .await
            .unwrap();
        context.active_goal_id = Some(goal.id);

        let requests = Arc::new(AtomicUsize::new(0));
        let requests_for_mock = requests.clone();
        let mock = MockClient::with_responder(move |_request, idx| {
            requests_for_mock.fetch_add(1, Ordering::SeqCst);
            assert_eq!(idx, 0, "the turn cap must stop after the truncated call");
            Ok((
                vec![ContentBlock::ToolUse {
                    id: "completion-truncated".to_string(),
                    name: "goal_propose_completion".to_string(),
                    input: serde_json::json!({
                        "summary": "truncated proposal",
                        "claims": [{"criterion_id": "criterion-1", "claim": "complete"}]
                    }),
                }],
                Some(StopReason::MaxTokens),
                None,
            ))
        });
        let mut agent = Agent::new(
            LlmProvider::Mock(mock),
            context,
            crate::tool::toolset(),
            crate::mcp::MCPToolRouter::new(),
            crate::permission::PermissionManager::try_new(crate::permission::PermissionMode::Auto)
                .unwrap(),
            AgentSystemPrompt::Static("test".to_string()),
        )
        .with_max_turns(Some(1));

        let outcome = agent
            .agent_loop_detailed(Some(Message::new_text(Role::User, "finish the goal")))
            .await
            .expect("a truncated final Goal turn returns a bounded stop");

        assert!(agent.kernel_activations_for_test() >= 1);
        assert_eq!(outcome.stop, AgentLoopStop::MaxTurns);
        assert_ne!(outcome.stop, AgentLoopStop::Completed);
        assert_eq!(requests.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn goal_context_input_keeps_its_non_user_origin_on_the_kernel_path() {
        ensure_config();
        let dir = tempfile::tempdir().unwrap();
        let store = crate::store::open_sqlite_session_store(&dir.path().join("session.db"))
            .await
            .unwrap();
        store
            .create_session("session-1", dir.path().to_str().unwrap(), "")
            .await
            .unwrap();
        let mock = MockClient::new(vec![(
            vec![make_text_block("done")],
            Some(StopReason::EndTurn),
        )]);
        let mut agent = Agent::new(
            LlmProvider::Mock(mock),
            test_context("kernel_goal_context_origin"),
            crate::tool::toolset(),
            crate::mcp::MCPToolRouter::new(),
            crate::permission::PermissionManager::try_new(crate::permission::PermissionMode::Auto)
                .unwrap(),
            AgentSystemPrompt::Static("test".to_string()),
        )
        .with_session("session-1".to_string(), store.clone());

        agent
            .agent_loop_detailed(Some(
                Message::new_text(Role::User, "<goal-context>work</goal-context>")
                    .with_kind(MessageKind::GoalContext),
            ))
            .await
            .expect("Goal context activation completes");

        let messages = store.load_agent_session("session-1").await.unwrap();
        let goal = messages
            .iter()
            .find(|message| {
                message.content.iter().any(|block| {
                    matches!(
                        block,
                        AgentContentBlock::Text { text }
                            if text.contains("<goal-context>work</goal-context>")
                    )
                })
            })
            .expect("Goal context was persisted");
        assert_eq!(goal.origin, MessageOrigin::GoalContext);
    }

    #[tokio::test]
    async fn agent_loop_respects_max_turns_cap() {
        ensure_config();
        let context = test_context("agent_loop_max_turns");
        let mock = MockClient::new(vec![(
            vec![make_text_block("done")],
            Some(StopReason::EndTurn),
        )]);
        let mut agent = Agent::new(
            LlmProvider::Mock(mock),
            context,
            crate::tool::toolset(),
            crate::mcp::MCPToolRouter::new(),
            crate::permission::PermissionManager::try_new(
                crate::permission::PermissionMode::Default,
            )
            .unwrap(),
            AgentSystemPrompt::Static("test".to_string()),
        )
        .with_max_turns(Some(1));

        agent
            .agent_loop(Some(Message::new_text(Role::User, "hi")))
            .await
            .unwrap();
        assert_eq!(agent.turns_taken, 1);
        assert!(
            !agent.max_turns_reached,
            "a child that answered inside its cap did not hit it: the summary must \
             not claim it was truncated"
        );
    }

    #[tokio::test]
    async fn agent_loop_emits_turn_stats_each_iteration() {
        ensure_config();
        let context = test_context("agent_loop_turn_stats");
        let mock = MockClient::new(vec![(
            vec![make_text_block("done")],
            Some(StopReason::EndTurn),
        )]);
        let mut agent = Agent::new(
            LlmProvider::Mock(mock),
            context,
            crate::tool::toolset(),
            crate::mcp::MCPToolRouter::new(),
            crate::permission::PermissionManager::try_new(
                crate::permission::PermissionMode::Default,
            )
            .unwrap(),
            AgentSystemPrompt::Static("test".to_string()),
        );
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        agent = agent.with_ui_channel(tx);

        agent
            .agent_loop(Some(Message::new_text(Role::User, "hi")))
            .await
            .unwrap();

        // Drain without blocking: the loop has returned, so every emitted
        // update is already queued.
        let mut stats = Vec::new();
        while let Ok(update) = rx.try_recv() {
            if let AgentUpdate::TurnStats {
                turns_taken,
                max_turns,
            } = update
            {
                stats.push((turns_taken, max_turns));
            }
        }
        assert_eq!(
            stats,
            vec![(1, None)],
            "one TurnStats per loop iteration, carrying the (absent) main-agent cap"
        );
    }

    #[tokio::test]
    async fn agent_loop_turn_stats_carries_the_cap_and_counts_the_capped_turn() {
        ensure_config();
        let context = test_context("agent_loop_turn_stats_capped");
        let mock = MockClient::new(vec![(
            vec![make_text_block("done")],
            Some(StopReason::EndTurn),
        )]);
        let mut agent = Agent::new(
            LlmProvider::Mock(mock),
            context,
            crate::tool::toolset(),
            crate::mcp::MCPToolRouter::new(),
            crate::permission::PermissionManager::try_new(
                crate::permission::PermissionMode::Default,
            )
            .unwrap(),
            AgentSystemPrompt::Static("test".to_string()),
        )
        .with_max_turns(Some(1));
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        agent = agent.with_ui_channel(tx);

        agent
            .agent_loop(Some(Message::new_text(Role::User, "hi")))
            .await
            .unwrap();

        let mut stats = Vec::new();
        while let Ok(update) = rx.try_recv() {
            if let AgentUpdate::TurnStats {
                turns_taken,
                max_turns,
            } = update
            {
                stats.push((turns_taken, max_turns));
            }
        }
        assert_eq!(
            stats,
            vec![(1, Some(1))],
            "the cap is reported and the counted turn is a real model call"
        );
    }

    #[tokio::test]
    async fn agent_loop_max_turns_zero_stops_immediately() {
        ensure_config();
        let context = test_context("agent_loop_max_turns_zero");
        // The mock is never called: the cap fires before the first LLM call.
        let mock = MockClient::new(vec![]);
        let mut agent = Agent::new(
            LlmProvider::Mock(mock),
            context,
            crate::tool::toolset(),
            crate::mcp::MCPToolRouter::new(),
            crate::permission::PermissionManager::try_new(
                crate::permission::PermissionMode::Default,
            )
            .unwrap(),
            AgentSystemPrompt::Static("test".to_string()),
        )
        .with_max_turns(Some(0));

        agent
            .agent_loop(Some(Message::new_text(Role::User, "hi")))
            .await
            .unwrap();
        assert_eq!(
            agent.turns_taken, 0,
            "the cap is checked before the counter advances: no model call happened"
        );
        assert!(
            agent.max_turns_reached,
            "the summary that explains why a child stopped reads this flag, so the \
             loop that stopped it has to set it"
        );
        // Only the user message is present — the cap stopped before any reply.
        assert_eq!(agent.runtime.context.len(), 1);
    }

    #[tokio::test]
    async fn agent_loop_detailed_reports_completed_without_tool_calls() {
        ensure_config();
        let context = test_context("agent_loop_detailed_completed");
        let mock = MockClient::new(vec![(
            vec![make_text_block("Hello, I am a coding agent.")],
            Some(StopReason::EndTurn),
        )]);
        let mut agent = Agent::new(
            LlmProvider::Mock(mock),
            context,
            crate::tool::toolset(),
            crate::mcp::MCPToolRouter::new(),
            crate::permission::PermissionManager::try_new(
                crate::permission::PermissionMode::Default,
            )
            .unwrap(),
            AgentSystemPrompt::Static("test".to_string()),
        );

        let outcome = agent
            .agent_loop_detailed(Some(Message::new_text(Role::User, "Say hello")))
            .await
            .expect("agent_loop_detailed completes");

        assert_eq!(outcome.stop, AgentLoopStop::Completed);
        assert_eq!(outcome.turns, 1);
        assert_eq!(outcome.tool_calls, 0);

        // The plain entry point still returns `()` for the same kind of script.
        let context = test_context("agent_loop_detailed_completed_unit");
        let mock = MockClient::new(vec![(
            vec![make_text_block("Hello again.")],
            Some(StopReason::EndTurn),
        )]);
        let mut agent = Agent::new(
            LlmProvider::Mock(mock),
            context,
            crate::tool::toolset(),
            crate::mcp::MCPToolRouter::new(),
            crate::permission::PermissionManager::try_new(
                crate::permission::PermissionMode::Default,
            )
            .unwrap(),
            AgentSystemPrompt::Static("test".to_string()),
        );
        let unit: () = agent
            .agent_loop(Some(Message::new_text(Role::User, "Say hello")))
            .await
            .expect("agent_loop completes");
        assert_eq!(unit, ());
    }

    /// `turns` is what *this* activation consumed, not the agent's running
    /// total: the Goal runner budgets one activation at a time and would
    /// double-count if the field were cumulative.
    #[tokio::test]
    async fn agent_loop_detailed_reports_turns_as_activation_delta() {
        ensure_config();
        let context = test_context("agent_loop_detailed_turns_delta");
        let mock = MockClient::new(vec![
            (
                vec![make_text_block("first activation")],
                Some(StopReason::EndTurn),
            ),
            (
                vec![make_text_block("second activation")],
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
            AgentSystemPrompt::Static("test".to_string()),
        );

        let first = agent
            .agent_loop_detailed(Some(Message::new_text(Role::User, "one")))
            .await
            .expect("first activation completes");
        assert_eq!(first.turns, 1);

        let second = agent
            .agent_loop_detailed(Some(Message::new_text(Role::User, "two")))
            .await
            .expect("second activation completes");
        assert_eq!(agent.turns_taken, 2, "the agent keeps a running total");
        assert_eq!(
            second.turns, 1,
            "the outcome must report the activation delta, not the running total"
        );
    }

    #[tokio::test]
    async fn agent_loop_detailed_reports_goal_yield_and_skips_unexecuted_calls() {
        ensure_config();
        let mut context = test_context("agent_loop_detailed_goal_yield");
        let goal = context
            .goal_manager
            .create(crate::goal::GoalCreate {
                title: "Need user input".to_string(),
                objective: "Pause for a user answer".to_string(),
                criteria: vec![crate::goal::GoalCriterion::required_with_id(
                    "criterion-1",
                    "answer received",
                )],
            })
            .await
            .unwrap();
        context.active_goal_id = Some(goal.id.clone());

        let mock = MockClient::new(vec![(
            vec![
                ContentBlock::ToolUse {
                    id: "goal-input-detailed".to_string(),
                    name: "goal_request_input".to_string(),
                    input: serde_json::json!({"question": "Which option should I use?"}),
                },
                ContentBlock::ToolUse {
                    id: "bash-skipped".to_string(),
                    name: "bash".to_string(),
                    input: serde_json::json!({"command": "printf hello"}),
                },
            ],
            Some(StopReason::ToolUse),
        )]);

        let mut agent = Agent::new(
            LlmProvider::Mock(mock),
            context,
            crate::tool::toolset(),
            crate::mcp::MCPToolRouter::new(),
            crate::permission::PermissionManager::try_new(crate::permission::PermissionMode::Auto)
                .unwrap(),
            AgentSystemPrompt::Static("test".to_string()),
        );

        let outcome = agent
            .agent_loop_detailed(Some(Message::new_text(Role::User, "continue the goal")))
            .await
            .expect("agent_loop_detailed yields cleanly for user input");

        assert_eq!(
            outcome.stop,
            AgentLoopStop::GoalYield(GoalYieldReason::NeedsUser)
        );
        assert_eq!(outcome.turns, 1);
        assert_eq!(
            outcome.tool_calls, 1,
            "only the goal tool ran; the skipped bash call must not count"
        );
        assert_eq!(
            agent.runtime.goal_yield_reason,
            Some(GoalYieldReason::NeedsUser)
        );
    }

    #[tokio::test]
    async fn agent_loop_detailed_counts_each_executed_tool_call() {
        ensure_config();
        let context = test_context("agent_loop_detailed_counts_tools");
        let mock = MockClient::new(vec![
            (
                vec![
                    ContentBlock::ToolUse {
                        id: "b1".to_string(),
                        name: "bash".to_string(),
                        input: serde_json::json!({"command": "printf one"}),
                    },
                    ContentBlock::ToolUse {
                        id: "b2".to_string(),
                        name: "bash".to_string(),
                        input: serde_json::json!({"command": "printf two"}),
                    },
                ],
                Some(StopReason::ToolUse),
            ),
            (vec![make_text_block("done")], Some(StopReason::EndTurn)),
        ]);

        let mut agent = Agent::new(
            LlmProvider::Mock(mock),
            context,
            crate::tool::toolset(),
            crate::mcp::MCPToolRouter::new(),
            crate::permission::PermissionManager::try_new(crate::permission::PermissionMode::Auto)
                .unwrap(),
            AgentSystemPrompt::Static("test".to_string()),
        );

        let outcome = agent
            .agent_loop_detailed(Some(Message::new_text(Role::User, "run both")))
            .await
            .expect("agent_loop_detailed completes");

        assert_eq!(outcome.tool_calls, 2);
        assert_eq!(outcome.stop, AgentLoopStop::Completed);
    }

    #[tokio::test]
    async fn agent_loop_detailed_reports_max_turns() {
        ensure_config();
        let context = test_context("agent_loop_detailed_max_turns");
        let mock = MockClient::new(vec![(
            vec![ContentBlock::ToolUse {
                id: "b1".to_string(),
                name: "bash".to_string(),
                input: serde_json::json!({"command": "printf hi"}),
            }],
            Some(StopReason::ToolUse),
        )]);
        let mut agent = Agent::new(
            LlmProvider::Mock(mock),
            context,
            crate::tool::toolset(),
            crate::mcp::MCPToolRouter::new(),
            crate::permission::PermissionManager::try_new(crate::permission::PermissionMode::Auto)
                .unwrap(),
            AgentSystemPrompt::Static("test".to_string()),
        )
        .with_max_turns(Some(1));

        let outcome = agent
            .agent_loop_detailed(Some(Message::new_text(Role::User, "hi")))
            .await
            .expect("agent_loop_detailed stops at the cap");

        assert_eq!(outcome.stop, AgentLoopStop::MaxTurns);
        assert_eq!(
            outcome.turns, 1,
            "the probe that stops an exhausted loop is not a spent turn"
        );
    }

    #[tokio::test]
    async fn agent_loop_detailed_reports_cancelled() {
        ensure_config();
        let context = test_context("agent_loop_detailed_cancelled");
        let mock = MockClient::new(vec![]);
        let mut agent = Agent::new(
            LlmProvider::Mock(mock),
            context,
            crate::tool::toolset(),
            crate::mcp::MCPToolRouter::new(),
            crate::permission::PermissionManager::try_new(
                crate::permission::PermissionMode::Default,
            )
            .unwrap(),
            AgentSystemPrompt::Static("test".to_string()),
        );
        agent
            .runtime
            .cancel_flag
            .store(true, std::sync::atomic::Ordering::SeqCst);

        let outcome = agent
            .agent_loop_detailed(Some(Message::new_text(Role::User, "hi")))
            .await
            .expect("a cancelled loop returns Ok");

        assert_eq!(outcome.stop, AgentLoopStop::Cancelled);
    }

    /// The loop-top cancel check runs *before* the model call, so a press that
    /// arrives while the final response is still streaming is only visible to
    /// the stop-reason arms — which must not book the response (and a Goal
    /// completion proposal with it) against a Goal the user just cancelled
    /// (2026-10-16).
    #[tokio::test]
    async fn agent_loop_detailed_reports_cancelled_when_the_flag_arrives_mid_response() {
        ensure_config();
        let context = test_context("agent_loop_detailed_cancel_mid_response");
        // The flag only becomes reachable once the agent exists, so the mock
        // reads it out of a slot the test fills afterwards.
        let flag_slot: std::sync::Arc<
            std::sync::Mutex<Option<std::sync::Arc<std::sync::atomic::AtomicBool>>>,
        > = std::sync::Arc::new(std::sync::Mutex::new(None));
        let slot = flag_slot.clone();
        let mock = MockClient::with_responder(move |_request, _idx| {
            if let Some(flag) = slot.lock().unwrap().as_ref() {
                flag.store(true, std::sync::atomic::Ordering::SeqCst);
            }
            Ok((
                vec![make_text_block("done")],
                Some(StopReason::EndTurn),
                None,
            ))
        });
        let mut agent = Agent::new(
            LlmProvider::Mock(mock),
            context,
            crate::tool::toolset(),
            crate::mcp::MCPToolRouter::new(),
            crate::permission::PermissionManager::try_new(
                crate::permission::PermissionMode::Default,
            )
            .unwrap(),
            AgentSystemPrompt::Static("test".to_string()),
        );
        let interrupt_calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let interrupt_calls_for_hook = interrupt_calls.clone();
        agent = agent.with_interrupt(move |_agent| {
            interrupt_calls_for_hook.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Box::pin(async { Ok(HookControl::Continue) })
        });
        *flag_slot.lock().unwrap() = Some(agent.tool_context.cancel_flag.clone());

        let outcome = agent
            .agent_loop_detailed(Some(Message::new_text(Role::User, "hi")))
            .await
            .expect("a cancelled loop returns Ok");

        assert_eq!(
            outcome.stop,
            AgentLoopStop::Cancelled,
            "a cancel that lands mid-stream must beat the EndTurn classification"
        );
        assert!(agent.kernel_activations_for_test() >= 1);
        assert_eq!(
            interrupt_calls.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "the kernel cancellation outcome must dispatch Interrupt hooks once"
        );
    }

    #[tokio::test]
    async fn agent_loop_detailed_reports_max_tokens_after_continuations_exhausted() {
        ensure_config();
        let context = test_context("agent_loop_detailed_max_tokens");
        let mock = MockClient::with_responder(|_request, _idx| {
            Ok((
                vec![make_text_block("partial output")],
                Some(StopReason::MaxTokens),
                None,
            ))
        });
        let mut agent = Agent::new(
            LlmProvider::Mock(mock),
            context,
            crate::tool::toolset(),
            crate::mcp::MCPToolRouter::new(),
            crate::permission::PermissionManager::try_new(
                crate::permission::PermissionMode::Default,
            )
            .unwrap(),
            AgentSystemPrompt::Static("test".to_string()),
        );

        let outcome = agent
            .agent_loop_detailed(Some(Message::new_text(Role::User, "hi")))
            .await
            .expect("an exhausted continuation finishes the turn");

        assert_eq!(outcome.stop, AgentLoopStop::MaxTokens);
        assert_eq!(outcome.tool_calls, 0);
        assert_eq!(agent.runtime.recovery_state.continuation_attempts, 3);
    }

    #[tokio::test]
    async fn kernel_recovery_counts_are_visible_on_the_facade_after_success() {
        ensure_config();
        let context = test_context("kernel_recovery_success");
        let mock = MockClient::with_responder(|_request, idx| match idx {
            0 => Err(LlmError::Request("temporary connection failure".into())),
            _ => Ok((
                vec![make_text_block("recovered")],
                Some(StopReason::EndTurn),
                None,
            )),
        });
        let mut agent = Agent::new(
            LlmProvider::Mock(mock),
            context,
            crate::tool::toolset(),
            crate::mcp::MCPToolRouter::new(),
            crate::permission::PermissionManager::try_new(
                crate::permission::PermissionMode::Default,
            )
            .unwrap(),
            AgentSystemPrompt::Static("test".to_string()),
        );

        let outcome = agent
            .agent_loop_detailed(Some(Message::new_text(Role::User, "hi")))
            .await
            .expect("the transient failure recovers");

        assert_eq!(outcome.stop, AgentLoopStop::Completed);
        assert_eq!(agent.runtime.recovery_state.transport_attempts, 1);
    }

    #[tokio::test]
    async fn kernel_recovery_counts_are_visible_on_the_facade_after_cancellation() {
        ensure_config();
        let context = test_context("kernel_recovery_cancellation");
        let mock =
            MockClient::with_responder(|_request, _idx| Err(LlmError::Request("retry".into())));
        let mut agent = Agent::new(
            LlmProvider::Mock(mock),
            context,
            crate::tool::toolset(),
            crate::mcp::MCPToolRouter::new(),
            crate::permission::PermissionManager::try_new(
                crate::permission::PermissionMode::Default,
            )
            .unwrap(),
            AgentSystemPrompt::Static("test".to_string()),
        );
        let cancel_flag = agent.runtime.cancel_flag.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            cancel_flag.store(true, std::sync::atomic::Ordering::Relaxed);
        });

        let outcome = agent
            .agent_loop_detailed(Some(Message::new_text(Role::User, "hi")))
            .await
            .expect("cancellation still returns a typed outcome");

        assert_eq!(outcome.stop, AgentLoopStop::Cancelled);
        assert_eq!(agent.runtime.recovery_state.transport_attempts, 1);
    }

    #[tokio::test]
    async fn max_tokens_tool_use_runs_manual_compaction_then_continues() {
        ensure_config();
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        };

        let requests = Arc::new(AtomicUsize::new(0));
        let requests_for_mock = requests.clone();
        let mock = MockClient::with_responder(move |_request, idx| {
            requests_for_mock.fetch_add(1, Ordering::SeqCst);
            match idx {
                0 => Ok((
                    vec![ContentBlock::ToolUse {
                        id: "compact-1".to_string(),
                        name: "compact".to_string(),
                        input: serde_json::json!({"focus": "keep the manual compaction"}),
                    }],
                    Some(StopReason::MaxTokens),
                    None,
                )),
                1 => Ok((
                    vec![make_text_block("compacted summary")],
                    Some(StopReason::EndTurn),
                    None,
                )),
                2 => Ok((
                    vec![make_text_block("continued after compaction")],
                    Some(StopReason::EndTurn),
                    None,
                )),
                other => panic!("unexpected model request {other}"),
            }
        });
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut agent = Agent::new(
            LlmProvider::Mock(mock),
            test_context("kernel_max_tokens_manual_compact"),
            crate::tool::toolset(),
            crate::mcp::MCPToolRouter::new(),
            crate::permission::PermissionManager::try_new(crate::permission::PermissionMode::Auto)
                .unwrap(),
            AgentSystemPrompt::Static("test".to_string()),
        )
        .with_ui_channel(tx);

        let outcome = agent
            .agent_loop_detailed(Some(Message::new_text(Role::User, "compact and continue")))
            .await
            .expect("manual compaction should precede continuation");

        assert!(agent.kernel_activations_for_test() >= 1);
        assert_eq!(outcome.stop, AgentLoopStop::Completed);
        assert_eq!(outcome.tool_calls, 1);
        assert_eq!(requests.load(Ordering::SeqCst), 3);
        let mut saw_manual_compaction = false;
        while let Ok(update) = rx.try_recv() {
            if matches!(
                update,
                AgentUpdate::Info(message) if message.contains("Compaction complete (Manual)")
            ) {
                saw_manual_compaction = true;
            }
        }
        assert!(
            saw_manual_compaction,
            "the kernel bridge must report the manual compaction"
        );
    }

    #[tokio::test]
    async fn agent_loop_detailed_reports_blocked_by_hook() {
        ensure_config();
        let context = test_context("agent_loop_detailed_blocked_by_hook");
        let mock = MockClient::new(vec![]);
        let mut agent = Agent::new(
            LlmProvider::Mock(mock),
            context,
            crate::tool::toolset(),
            crate::mcp::MCPToolRouter::new(),
            crate::permission::PermissionManager::try_new(
                crate::permission::PermissionMode::Default,
            )
            .unwrap(),
            AgentSystemPrompt::Static("test".to_string()),
        )
        .with_session_start(|_agent, _context| {
            Box::pin(async { Ok(HookControl::Block("no context yet".to_string())) })
        });

        let outcome = agent
            .agent_loop_detailed(Some(Message::new_text(Role::User, "hi")))
            .await
            .expect("a blocked turn returns Ok");

        assert_eq!(outcome.stop, AgentLoopStop::BlockedByHook);
        assert_eq!(outcome.turns, 0);
    }

    #[tokio::test]
    async fn agent_loop_drains_pending_subagent_results() {
        ensure_config();
        let context = test_context("agent_loop_drain_subagent");
        let mock = MockClient::new(vec![(
            vec![make_text_block("got it")],
            Some(StopReason::EndTurn),
        )]);
        let mut agent = Agent::new(
            LlmProvider::Mock(mock),
            context,
            crate::tool::toolset(),
            crate::mcp::MCPToolRouter::new(),
            crate::permission::PermissionManager::try_new(
                crate::permission::PermissionMode::Default,
            )
            .unwrap(),
            AgentSystemPrompt::Static("test".to_string()),
        );

        agent
            .runtime
            .pending_subagent_results
            .lock()
            .unwrap()
            .push_back(SubagentResult {
                child_id: "child-1".to_string(),
                summary: "found the bug".to_string(),
                success: true,
            });

        agent
            .agent_loop(Some(Message::new_text(Role::User, "hi")))
            .await
            .unwrap();

        let text = agent
            .runtime
            .context
            .iter()
            .map(|m| crate::extract_text(&m.content))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            text.contains("<subagent-finished id=\"child-1\" success=\"true\">"),
            "context should contain the re-injected result: {text}"
        );
        assert!(
            text.contains("found the bug"),
            "summary should be injected: {text}"
        );
    }

    /// A `SessionStart` hook's context reaches the model as a message of its
    /// own, framed so it does not read as user input.
    ///
    /// Codex records the same text as a `developer` role message; Tact's
    /// message model has only user/assistant, so the `<hook-context>` markers
    /// carry the provenance instead. Before this, the text was logged as a
    /// warning and dropped, which made the reference `basic-memory` plugin's
    /// session briefing inert.
    #[tokio::test]
    async fn agent_loop_injects_session_start_context_as_its_own_message() {
        ensure_config();
        use tact_protocol::AgentUpdate;

        const BRIEF: &str = "graph says: resume from checkpoint 7";
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mock = MockClient::new(vec![(
            vec![make_text_block("ok")],
            Some(StopReason::EndTurn),
        )]);
        let mut agent = Agent::new(
            LlmProvider::Mock(mock),
            test_context("session_start_context"),
            crate::tool::toolset(),
            crate::mcp::MCPToolRouter::new(),
            crate::permission::PermissionManager::try_new(
                crate::permission::PermissionMode::Default,
            )
            .unwrap(),
            AgentSystemPrompt::Static("test".to_string()),
        )
        .with_ui_channel(tx)
        .with_session_start(|_agent, context| {
            context.push_additional_context(Some("plugin demo"), BRIEF);
            // Blank output must not produce an empty message.
            context.push_additional_context(None, "   \n  ");
            Box::pin(async { Ok(HookControl::Continue) })
        });

        agent.dispatch_session_start_hooks().await.unwrap();
        assert_eq!(
            agent.runtime.pending_session_context,
            vec![HookContextChunk::new(Some("plugin demo"), BRIEF)]
        );

        agent
            .agent_loop(Some(Message::new_text(Role::User, "hi")))
            .await
            .unwrap();

        let injected = agent
            .runtime
            .context
            .first()
            .expect("the injected context leads the conversation");
        assert!(
            injected.is_hook_context(),
            "the cell is marked so the TUI and compaction can tell it apart"
        );
        let text = crate::extract_text(&injected.content);
        assert!(
            crate::hook::is_hook_context_text(&text)
                && text.ends_with(crate::hook::HOOK_CONTEXT_CLOSE_TAG),
            "context is framed: {text}"
        );
        assert_eq!(
            crate::hook::hook_context_source(&text),
            Some("plugin demo"),
            "the source rides on the frame so a reload still knows it: {text}"
        );
        assert_eq!(
            crate::hook::hook_context_body(&text),
            BRIEF,
            "the body is what the hook printed: {text}"
        );
        assert!(
            agent.runtime.pending_session_context.is_empty(),
            "the queue is drained"
        );

        // The turn's own message still follows it.
        assert!(matches!(
            agent.runtime.context.get(1).map(|m| m.role),
            Some(Role::User)
        ));

        // The TUI is told what was injected, without the framing.
        let mut notices = Vec::new();
        while let Ok(update) = rx.try_recv() {
            if let AgentUpdate::HookContext { source, text } = update {
                notices.push((source, text));
            }
        }
        assert_eq!(
            notices,
            vec![(Some("plugin demo".to_string()), BRIEF.to_string())]
        );
    }

    /// The briefing outlives the pre-turn compaction it arrives with.
    ///
    /// It is recorded *after* `auto_compact_due` / `compact_history`, because
    /// `build_compacted_history` keeps only real user turns and would drop a
    /// `<hook-context>` cell — the same reason Codex runs its start hooks after
    /// `run_pre_sampling_compact`.
    #[tokio::test]
    async fn session_start_context_survives_the_pre_turn_compaction() {
        ensure_config();
        const BRIEF: &str = "graph says: resume from checkpoint 7";
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let mock = MockClient::new(vec![
            (
                vec![make_text_block("## Handoff\nwe were mid-refactor")],
                Some(StopReason::EndTurn),
            ),
            (vec![make_text_block("ok")], Some(StopReason::EndTurn)),
        ]);
        let mut agent = Agent::new(
            LlmProvider::Mock(mock),
            test_context("session_start_survives_compact"),
            crate::tool::toolset(),
            crate::mcp::MCPToolRouter::new(),
            crate::permission::PermissionManager::try_new(
                crate::permission::PermissionMode::Default,
            )
            .unwrap(),
            AgentSystemPrompt::Static("test".to_string()),
        )
        .with_ui_channel(tx)
        .with_session_start(|_agent, context| {
            context.push_additional_context(Some("plugin demo"), BRIEF);
            Box::pin(async { Ok(HookControl::Continue) })
        });
        agent.agent_settings.model_context_window = 60_000;
        agent.agent_settings.max_tokens = 100;

        // A restored history large enough to trip the pre-turn compaction.
        agent
            .runtime
            .context
            .push(Message::new_text(Role::User, "x".repeat(300_000)));
        assert!(agent.auto_compact_due(0), "the fixture must compact");

        agent.dispatch_session_start_hooks().await.unwrap();
        agent
            .agent_loop(Some(Message::new_text(Role::User, "hi")))
            .await
            .unwrap();

        let brief = agent
            .runtime
            .context
            .iter()
            .find(|message| message.is_hook_context())
            .expect("the briefing must outlive the compaction it arrived with");
        assert!(
            crate::extract_text(&brief.content).contains(BRIEF),
            "and carry its text: {}",
            crate::extract_text(&brief.content)
        );

        // The compaction re-queues the hooks as `compact` — Codex's checkpoint
        // trigger — so the next turn re-orients against the new history.
        assert!(agent.runtime.session_start_hooks_pending);
        assert_eq!(
            agent.runtime.session_start_source,
            crate::hook::SessionStartSource::Compact
        );
    }

    /// A compaction re-queues the `SessionStart` hooks as `compact`; the
    /// *next* turn must actually re-run them with that source and record the
    /// new briefing. The sibling test only asserts the re-queue flag/source,
    /// so this one drives the second turn to prove the checkpoint is
    /// delivered rather than merely armed.
    #[tokio::test]
    async fn session_start_hooks_rerun_as_compact_after_a_compaction() {
        ensure_config();
        use std::sync::{Arc, Mutex};

        let seen: Arc<Mutex<Vec<crate::hook::SessionStartSource>>> =
            Arc::new(Mutex::new(Vec::new()));
        let recorder = seen.clone();
        let mock = MockClient::new(vec![
            (
                vec![make_text_block("## Handoff\nwe were mid-refactor")],
                Some(StopReason::EndTurn),
            ),
            (vec![make_text_block("first")], Some(StopReason::EndTurn)),
            (vec![make_text_block("second")], Some(StopReason::EndTurn)),
        ]);
        let mut agent = Agent::new(
            LlmProvider::Mock(mock),
            test_context("session_start_compact_rerun"),
            crate::tool::toolset(),
            crate::mcp::MCPToolRouter::new(),
            crate::permission::PermissionManager::try_new(
                crate::permission::PermissionMode::Default,
            )
            .unwrap(),
            AgentSystemPrompt::Static("test".to_string()),
        )
        .with_session_start(move |agent, context| {
            let recorder = recorder.clone();
            let source = agent.session_start_source;
            context.push_additional_context(
                Some("plugin demo"),
                &format!("brief:{}", source.as_str()),
            );
            Box::pin(async move {
                recorder.lock().unwrap().push(source);
                Ok(HookControl::Continue)
            })
        });
        agent.agent_settings.model_context_window = 60_000;
        agent.agent_settings.max_tokens = 100;

        // A restored history large enough to trip the pre-turn compaction.
        agent
            .runtime
            .context
            .push(Message::new_text(Role::User, "x".repeat(300_000)));
        assert!(agent.auto_compact_due(0), "the fixture must compact");

        agent
            .agent_loop(Some(Message::new_text(Role::User, "hi")))
            .await
            .unwrap();
        assert_eq!(
            &*seen.lock().unwrap(),
            &[crate::hook::SessionStartSource::Startup],
            "the first turn runs the startup hooks once"
        );
        assert!(
            agent.runtime.session_start_hooks_pending,
            "the compaction re-queues them for the next turn"
        );

        agent
            .agent_loop(Some(Message::new_text(Role::User, "again")))
            .await
            .unwrap();

        assert_eq!(
            &*seen.lock().unwrap(),
            &[
                crate::hook::SessionStartSource::Startup,
                crate::hook::SessionStartSource::Compact,
            ],
            "the re-queued hooks run with `compact` on the next turn"
        );
        let briefings: Vec<String> = agent
            .runtime
            .context
            .iter()
            .filter(|message| message.is_hook_context())
            .map(|message| crate::extract_text(&message.content))
            .collect();
        assert!(
            briefings
                .iter()
                .any(|brief| brief.contains("brief:startup")),
            "the startup briefing is recorded: {briefings:?}"
        );
        assert!(
            briefings
                .iter()
                .any(|brief| brief.contains("brief:compact")),
            "the compact briefing is recorded: {briefings:?}"
        );
    }

    /// Startup must not wait for a plugin hook: the first turn runs them, which
    /// is still ahead of that turn's user message but no longer ahead of the
    /// first frame. `basic-memory`'s hook measured ~8s warm here.
    #[tokio::test]
    async fn session_start_hooks_run_on_the_first_turn_and_only_once() {
        ensure_config();
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        };

        let calls = Arc::new(AtomicUsize::new(0));
        let seen = calls.clone();
        let mock = MockClient::new(vec![
            (vec![make_text_block("first")], Some(StopReason::EndTurn)),
            (vec![make_text_block("second")], Some(StopReason::EndTurn)),
        ]);
        let mut agent = Agent::new(
            LlmProvider::Mock(mock),
            test_context("session_start_first_turn"),
            crate::tool::toolset(),
            crate::mcp::MCPToolRouter::new(),
            crate::permission::PermissionManager::try_new(
                crate::permission::PermissionMode::Default,
            )
            .unwrap(),
            AgentSystemPrompt::Static("test".to_string()),
        )
        .with_session_start(move |_agent, context| {
            let seen = seen.clone();
            Box::pin(async move {
                seen.fetch_add(1, Ordering::SeqCst);
                context.push_additional_context(None, "brief");
                Ok(HookControl::Continue)
            })
        });

        assert_eq!(calls.load(Ordering::SeqCst), 0, "startup must not run them");
        assert!(agent.runtime.session_start_hooks_pending);

        agent
            .agent_loop(Some(Message::new_text(Role::User, "hi")))
            .await
            .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1, "the first turn runs them");

        agent
            .agent_loop(Some(Message::new_text(Role::User, "again")))
            .await
            .unwrap();
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "a later turn must not re-run them"
        );
    }

    /// Codex's `continue: false` on `SessionStart` stops the turn: the user
    /// message is never pushed, rather than pushed with a notice in the log.
    #[tokio::test]
    async fn session_start_stop_skips_the_turn() {
        ensure_config();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mock = MockClient::new(vec![(
            vec![make_text_block("should not run")],
            Some(StopReason::EndTurn),
        )]);
        let mut agent = Agent::new(
            LlmProvider::Mock(mock),
            test_context("session_start_stop_skips"),
            crate::tool::toolset(),
            crate::mcp::MCPToolRouter::new(),
            crate::permission::PermissionManager::try_new(
                crate::permission::PermissionMode::Default,
            )
            .unwrap(),
            AgentSystemPrompt::Static("test".to_string()),
        )
        .with_ui_channel(tx)
        .with_session_start(|_agent, _context| {
            Box::pin(async { Ok(HookControl::Block("no context yet".to_string())) })
        });

        agent
            .agent_loop(Some(Message::new_text(Role::User, "hi")))
            .await
            .unwrap();

        assert!(
            agent.runtime.context.is_empty(),
            "the turn must not run: {:?}",
            agent.runtime.context.len()
        );
        let mut notice = None;
        while let Ok(update) = rx.try_recv() {
            if let AgentUpdate::Info(text) = update {
                notice = Some(text);
            }
        }
        assert!(
            notice
                .as_deref()
                .is_some_and(|text| text.contains("no context yet")),
            "{notice:?}"
        );
    }

    /// Context the tool hooks collected is recorded before the next request, so
    /// the model reads it alongside the tool call it annotates.
    #[tokio::test]
    async fn agent_loop_records_tool_hook_context() {
        ensure_config();
        let mock = MockClient::new(vec![(
            vec![make_text_block("ok")],
            Some(StopReason::EndTurn),
        )]);
        let mut agent = Agent::new(
            LlmProvider::Mock(mock),
            test_context("agent_loop_tool_hook_context"),
            crate::tool::toolset(),
            crate::mcp::MCPToolRouter::new(),
            crate::permission::PermissionManager::try_new(
                crate::permission::PermissionMode::Default,
            )
            .unwrap(),
            AgentSystemPrompt::Static("test".to_string()),
        );
        agent
            .runtime
            .pending_hook_context
            .lock()
            .unwrap()
            .push_back(HookContextChunk::new(Some("plugin demo"), "pre context"));

        agent
            .agent_loop(Some(Message::new_text(Role::User, "hi")))
            .await
            .unwrap();

        let injected: Vec<String> = agent
            .runtime
            .context
            .iter()
            .filter(|message| message.is_hook_context())
            .map(|message| crate::extract_text(&message.content))
            .collect();
        assert_eq!(injected.len(), 1, "one cell: {injected:?}");
        assert!(injected[0].contains("pre context"), "{injected:?}");
    }

    #[tokio::test]
    async fn agent_loop_surfaces_refusal_as_error() {
        ensure_config();
        use tact_protocol::AgentUpdate;

        let context = test_context("agent_loop_refusal");
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut tool_context = context;
        tool_context.events = UpdateSink::for_ui(tx.clone());

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
    fn with_ui_channel_syncs_tool_context_events() {
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

        assert!(agent.runtime.events.has_ui());
        assert!(agent.tool_context.events.has_ui());
    }

    #[test]
    fn an_observer_receives_events_and_cannot_answer_prompts() {
        ensure_config();
        use crate::tool::test_support::test_context;

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let context = test_context("observer_events");
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
        .with_event_observer(tx);

        agent.emit_update(AgentUpdate::Info("progress".into()));

        assert!(
            matches!(rx.try_recv(), Ok(AgentUpdate::Info(text)) if text == "progress"),
            "the observer sees every event"
        );
        assert!(
            !agent.runtime.events.has_ui(),
            "an observer is not an answering frontend"
        );
        assert!(
            agent.tool_context.events.ui().is_none(),
            "tool prompts must not treat an observer as an answerer"
        );
    }

    #[test]
    fn detaching_observers_closes_their_channel() {
        ensure_config();
        use crate::tool::test_support::test_context;

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let context = test_context("detach_observers");
        let mut agent = Agent::new(
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
        .with_event_observer(tx);

        agent.detach_event_observers();

        assert!(
            rx.try_recv().is_err(),
            "the channel is closed, so a pump awaiting it can return"
        );
    }

    /// A clone handed to a detached background task keeps the observer channel
    /// open, so detaching alone does not end the pump.
    ///
    /// This is the headless hang: `background_run` gives the spawned task a
    /// `BackgroundProgressSink` built from the invocation's reporter, and that
    /// reporter holds a clone of the observer sender. The task outlives the
    /// tool call by design ("there is no time limit"), so the sender outlives
    /// `detach_event_observers` too, and a pump that waits for the channel to
    /// close waits for the command.
    #[test]
    fn a_cloned_reporter_keeps_the_observer_channel_open_after_detach() {
        ensure_config();
        use crate::tool::test_support::test_context;

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let context = test_context("detach_with_cloned_reporter");
        let mut agent = Agent::new(
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
        .with_event_observer(tx);

        // What `background_run` does: a per-invocation clone, moved into a task
        // that outlives the call.
        let detached = agent.tool_context.progress_reporter.clone();

        agent.detach_event_observers();

        assert!(
            rx.try_recv().is_err(),
            "the channel is still open: the cloned reporter holds a sender"
        );

        // The channel closes only once that clone is dropped — i.e. when the
        // background command ends, which may be never.
        drop(detached);
        assert!(rx.try_recv().is_err(), "now it is closed");
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
        tool_context.events = UpdateSink::for_ui(tx.clone());

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

    /// A cancel that lands after the assistant message was persisted but before
    /// tool execution must still answer every `ToolUse` with a `ToolResult`.
    /// Otherwise the stored history has a dangling tool call, which providers
    /// that enforce pairing reject on the next request (and which a resume or a
    /// later fork would inherit).
    #[tokio::test]
    async fn cancel_before_tool_execution_pairs_every_tool_use() {
        ensure_config();
        use crate::tool::test_support::test_context;

        let dir = tempfile::tempdir().unwrap();
        let store = crate::store::open_sqlite_session_store(&dir.path().join("session.db"))
            .await
            .unwrap();
        store
            .create_session("session-1", dir.path().to_str().unwrap(), "")
            .await
            .unwrap();

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut tool_context = test_context("cancel_before_tools");
        tool_context.events = UpdateSink::for_ui(tx.clone());

        // The responder sets the agent's cancel flag while producing the
        // tool-call turn, so the flag is guaranteed to be set when the loop
        // reaches its pre-execution cancel check. `Agent::new` allocates the
        // real flag, so it is installed into the slot after construction.
        use std::sync::{
            Arc, Mutex,
            atomic::{AtomicBool, Ordering},
        };
        let flag_slot: Arc<Mutex<Option<Arc<AtomicBool>>>> = Arc::new(Mutex::new(None));
        let slot = flag_slot.clone();
        let mock = MockClient::with_responder(move |_request, idx| {
            if idx > 0 {
                // Bound the loop if the cancel path regresses: answer later
                // calls with a plain end-of-turn.
                return Ok((
                    vec![make_text_block("done")],
                    Some(StopReason::EndTurn),
                    None,
                ));
            }
            if let Some(flag) = slot.lock().unwrap().as_ref() {
                flag.store(true, Ordering::SeqCst);
            }
            Ok((
                vec![ContentBlock::ToolUse {
                    id: "t1".to_string(),
                    name: "read_file".to_string(),
                    input: serde_json::json!({ "path": "a.txt" }),
                }],
                Some(StopReason::ToolUse),
                None,
            ))
        });

        let mut agent = Agent::new(
            LlmProvider::Mock(mock),
            tool_context,
            crate::tool::toolset(),
            crate::mcp::MCPToolRouter::new(),
            crate::permission::PermissionManager::try_new(crate::permission::PermissionMode::Auto)
                .unwrap(),
            AgentSystemPrompt::Static("test".to_string()),
        )
        .with_ui_channel(tx)
        .with_session("session-1".to_string(), store.clone());
        *flag_slot.lock().unwrap() = Some(agent.tool_context.cancel_flag.clone());

        agent
            .agent_loop(Some(Message::new_text(Role::User, "read a.txt")))
            .await
            .expect("cancelled loop returns Ok");

        // The tool must not have run.
        let mut updates = Vec::new();
        while let Ok(u) = rx.try_recv() {
            updates.push(u);
        }
        assert!(
            !updates
                .iter()
                .any(|u| matches!(u, AgentUpdate::StepFinished { .. })),
            "a cancelled-before-execution tool must not report a finished step"
        );
        assert!(agent.kernel_activations_for_test() >= 1);
        assert_eq!(
            updates
                .iter()
                .filter(|update| matches!(update, AgentUpdate::Info(text) if text == "Cancelled by user"))
                .count(),
            1,
            "facade cancellation must emit one terminal notice"
        );

        // Every persisted `ToolUse` has a matching `ToolResult`, carrying the
        // cancel reason.
        let messages = store.load_session("session-1").await.unwrap();
        assert_eq!(
            tool_pairing(&messages),
            (vec!["t1".to_string()], vec!["t1".to_string()]),
            "the persisted history must pair the cancelled tool call with a result"
        );
        assert_eq!(
            tool_result_for(&messages, "t1").as_deref(),
            Some("Cancelled by user")
        );
    }

    /// The cancel flag can also flip *during* pre-flight: the driver sets it
    /// from its own command loop while the agent awaits a hook or a permission
    /// prompt, so calls earlier in the same turn are already resolved when the
    /// remaining ones are stubbed out. Every call still needs exactly one
    /// result — including the one the user just denied.
    #[tokio::test]
    async fn cancel_mid_preflight_answers_every_tool_use_once() {
        ensure_config();
        use crate::tool::test_support::test_context;
        use tact_protocol::{AgentUpdate, UiResponse};

        let dir = tempfile::tempdir().unwrap();
        let store = crate::store::open_sqlite_session_store(&dir.path().join("session.db"))
            .await
            .unwrap();
        store
            .create_session("session-1", dir.path().to_str().unwrap(), "")
            .await
            .unwrap();

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut tool_context = test_context("cancel_mid_preflight_deny");
        tool_context.events = UpdateSink::for_ui(tx.clone());

        // Two calls in one turn behind the usual leading text block, so the
        // cancel lands after `t1` was already resolved.
        let mock = MockClient::with_responder(|_request, idx| {
            if idx > 0 {
                return Ok((
                    vec![make_text_block("done")],
                    Some(StopReason::EndTurn),
                    None,
                ));
            }
            Ok((
                vec![
                    make_text_block("writing both files"),
                    ContentBlock::ToolUse {
                        id: "t1".to_string(),
                        name: "write_file".to_string(),
                        input: serde_json::json!({ "path": "a.txt", "content": "a" }),
                    },
                    ContentBlock::ToolUse {
                        id: "t2".to_string(),
                        name: "write_file".to_string(),
                        input: serde_json::json!({ "path": "b.txt", "content": "b" }),
                    },
                ],
                Some(StopReason::ToolUse),
                None,
            ))
        });

        let mut agent = Agent::new(
            LlmProvider::Mock(mock),
            tool_context,
            crate::tool::toolset(),
            crate::mcp::MCPToolRouter::new(),
            crate::permission::PermissionManager::try_new(
                crate::permission::PermissionMode::Default,
            )
            .unwrap(),
            AgentSystemPrompt::Static("test".to_string()),
        )
        .with_ui_channel(tx)
        .with_session("session-1".to_string(), store.clone());

        let flag = agent.tool_context.cancel_flag.clone();
        let responder = agent.tool_context.ui_responder.clone();
        tokio::spawn(async move {
            while let Some(update) = rx.recv().await {
                if let AgentUpdate::RequestSelect { request_id, .. } = update {
                    // Deny `t1`, then cancel: the flag is first observed at the
                    // top of the `t2` iteration, i.e. mid-pre-flight.
                    flag.store(true, std::sync::atomic::Ordering::SeqCst);
                    responder.respond(UiResponse::Select {
                        request_id,
                        choice: Some(1),
                    });
                    return;
                }
            }
        });

        agent
            .agent_loop(Some(Message::new_text(Role::User, "write both")))
            .await
            .expect("a cancelled loop returns Ok");

        assert!(agent.kernel_activations_for_test() >= 1);
        let messages = store.load_session("session-1").await.unwrap();
        let (uses, results) = tool_pairing(&messages);
        assert_eq!(uses, vec!["t1".to_string(), "t2".to_string()]);
        assert_eq!(results, uses, "each tool use must be answered exactly once");
    }

    /// As above, but the first call was *approved* before the cancel landed, so
    /// it is still sitting in `PreparedState::Run` with no output. It has to be
    /// answered as cancelled rather than indexed out of an empty output list.
    #[tokio::test]
    async fn cancel_mid_preflight_answers_an_approved_but_unrun_call() {
        ensure_config();
        use crate::tool::test_support::test_context;
        use tact_protocol::{AgentUpdate, UiResponse};

        let dir = tempfile::tempdir().unwrap();
        let store = crate::store::open_sqlite_session_store(&dir.path().join("session.db"))
            .await
            .unwrap();
        store
            .create_session("session-1", dir.path().to_str().unwrap(), "")
            .await
            .unwrap();

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut tool_context = test_context("cancel_mid_preflight_allow");
        tool_context.events = UpdateSink::for_ui(tx.clone());

        let mock = MockClient::with_responder(|_request, idx| {
            if idx > 0 {
                return Ok((
                    vec![make_text_block("done")],
                    Some(StopReason::EndTurn),
                    None,
                ));
            }
            Ok((
                vec![
                    make_text_block("writing both files"),
                    ContentBlock::ToolUse {
                        id: "t1".to_string(),
                        name: "write_file".to_string(),
                        input: serde_json::json!({ "path": "a.txt", "content": "a" }),
                    },
                    ContentBlock::ToolUse {
                        id: "t2".to_string(),
                        name: "write_file".to_string(),
                        input: serde_json::json!({ "path": "b.txt", "content": "b" }),
                    },
                ],
                Some(StopReason::ToolUse),
                None,
            ))
        });

        let mut agent = Agent::new(
            LlmProvider::Mock(mock),
            tool_context,
            crate::tool::toolset(),
            crate::mcp::MCPToolRouter::new(),
            crate::permission::PermissionManager::try_new(
                crate::permission::PermissionMode::Default,
            )
            .unwrap(),
            AgentSystemPrompt::Static("test".to_string()),
        )
        .with_ui_channel(tx)
        .with_session("session-1".to_string(), store.clone());

        let flag = agent.tool_context.cancel_flag.clone();
        let responder = agent.tool_context.ui_responder.clone();
        tokio::spawn(async move {
            while let Some(update) = rx.recv().await {
                if let AgentUpdate::RequestSelect { request_id, .. } = update {
                    // Allow `t1` once, then cancel.
                    flag.store(true, std::sync::atomic::Ordering::SeqCst);
                    responder.respond(UiResponse::Select {
                        request_id,
                        choice: Some(0),
                    });
                    return;
                }
            }
        });

        agent
            .agent_loop(Some(Message::new_text(Role::User, "write both")))
            .await
            .expect("a cancelled loop returns Ok");

        assert!(agent.kernel_activations_for_test() >= 1);
        let messages = store.load_session("session-1").await.unwrap();
        let (uses, results) = tool_pairing(&messages);
        assert_eq!(uses, vec!["t1".to_string(), "t2".to_string()]);
        assert_eq!(results, uses, "each tool use must be answered exactly once");
        assert_eq!(
            tool_result_for(&messages, "t1").as_deref(),
            Some("Cancelled by user"),
            "an approved call the cancel caught before execution is answered, not run"
        );
    }

    #[tokio::test]
    async fn facade_cancel_while_bash_is_in_flight_drains_and_pairs_the_result() {
        ensure_config();

        let dir = tempfile::tempdir().unwrap();
        let store = crate::store::open_sqlite_session_store(&dir.path().join("session.db"))
            .await
            .unwrap();
        store
            .create_session("session-1", dir.path().to_str().unwrap(), "")
            .await
            .unwrap();

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut tool_context = test_context("cancel_during_bash");
        tool_context.events = UpdateSink::for_ui(tx.clone());
        let mock = MockClient::with_responder(|_request, idx| {
            if idx > 0 {
                return Ok((
                    vec![make_text_block("done")],
                    Some(StopReason::EndTurn),
                    None,
                ));
            }
            Ok((
                vec![ContentBlock::ToolUse {
                    id: "bash-1".into(),
                    name: "bash".into(),
                    input: serde_json::json!({
                        "command": "printf started; sleep 0.2; printf finished"
                    }),
                }],
                Some(StopReason::ToolUse),
                None,
            ))
        });
        let mut agent = Agent::new(
            LlmProvider::Mock(mock),
            tool_context,
            crate::tool::toolset(),
            crate::mcp::MCPToolRouter::new(),
            crate::permission::PermissionManager::try_new(crate::permission::PermissionMode::Auto)
                .unwrap(),
            AgentSystemPrompt::Static("test".to_string()),
        )
        .with_ui_channel(tx)
        .with_session("session-1".to_string(), store.clone());

        let flag = agent.tool_context.cancel_flag.clone();
        let listener = tokio::spawn(async move {
            let mut updates = Vec::new();
            while let Some(update) = rx.recv().await {
                let cancelling = matches!(
                    &update,
                    AgentUpdate::Info(text) if text == "Cancelled by user"
                );
                if matches!(
                    &update,
                    AgentUpdate::ToolProgress { tool_id, .. } if tool_id == "bash-1"
                ) {
                    flag.store(true, std::sync::atomic::Ordering::SeqCst);
                }
                updates.push(update);
                if cancelling {
                    break;
                }
            }
            updates
        });

        let outcome = tokio::time::timeout(
            std::time::Duration::from_secs(3),
            agent.agent_loop_detailed(Some(Message::new_text(Role::User, "run it"))),
        )
        .await
        .expect("in-flight bash cancellation must be bounded")
        .expect("cancelled loop returns Ok");
        assert_eq!(outcome.stop, AgentLoopStop::Cancelled);
        assert!(agent.kernel_activations_for_test() >= 1);

        let updates = tokio::time::timeout(std::time::Duration::from_secs(1), listener)
            .await
            .expect("cancellation notice must close the bounded update listener")
            .expect("update listener");
        assert_eq!(
            updates
                .iter()
                .filter(|update| matches!(update, AgentUpdate::Info(text) if text == "Cancelled by user"))
                .count(),
            1
        );
        assert!(
            updates.iter().any(|update| matches!(
                update,
                AgentUpdate::StepFinished { tool_id, .. } if tool_id == "bash-1"
            )),
            "the drained bash wave must produce a terminal tool event"
        );

        let messages = store.load_session("session-1").await.unwrap();
        assert_eq!(
            tool_pairing(&messages),
            (vec!["bash-1".to_string()], vec!["bash-1".to_string()])
        );
    }

    #[tokio::test]
    async fn facade_cancel_while_another_tool_is_in_flight_drains_without_another_turn() {
        ensure_config();

        let dir = tempfile::tempdir().unwrap();
        let store = crate::store::open_sqlite_session_store(&dir.path().join("session.db"))
            .await
            .unwrap();
        store
            .create_session("session-1", dir.path().to_str().unwrap(), "")
            .await
            .unwrap();

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut tool_context = test_context("cancel_during_sleep");
        tool_context.events = UpdateSink::for_ui(tx.clone());
        let mock = MockClient::with_responder(|_request, idx| {
            if idx > 0 {
                return Ok((
                    vec![make_text_block("must not run")],
                    Some(StopReason::EndTurn),
                    None,
                ));
            }
            Ok((
                vec![ContentBlock::ToolUse {
                    id: "sleep-1".into(),
                    name: "sleep".into(),
                    input: serde_json::json!({"ms": 200}),
                }],
                Some(StopReason::ToolUse),
                None,
            ))
        });
        let mut agent = Agent::new(
            LlmProvider::Mock(mock),
            tool_context,
            crate::tool::toolset(),
            crate::mcp::MCPToolRouter::new(),
            crate::permission::PermissionManager::try_new(crate::permission::PermissionMode::Auto)
                .unwrap(),
            AgentSystemPrompt::Static("test".to_string()),
        )
        .with_ui_channel(tx)
        .with_session("session-1".to_string(), store.clone());

        let flag = agent.tool_context.cancel_flag.clone();
        let listener = tokio::spawn(async move {
            let mut updates = Vec::new();
            while let Some(update) = rx.recv().await {
                if matches!(
                    &update,
                    AgentUpdate::StepStarted { tool_id, .. } if tool_id == "sleep-1"
                ) {
                    tokio::time::sleep(std::time::Duration::from_millis(30)).await;
                    flag.store(true, std::sync::atomic::Ordering::SeqCst);
                }
                let cancelling = matches!(
                    &update,
                    AgentUpdate::Info(text) if text == "Cancelled by user"
                );
                updates.push(update);
                if cancelling {
                    break;
                }
            }
            updates
        });

        let outcome = tokio::time::timeout(
            std::time::Duration::from_secs(3),
            agent.agent_loop_detailed(Some(Message::new_text(Role::User, "wait"))),
        )
        .await
        .expect("in-flight sleep cancellation must be bounded")
        .expect("cancelled loop returns Ok");
        assert_eq!(outcome.stop, AgentLoopStop::Cancelled);
        assert_eq!(
            outcome.turns, 1,
            "cancellation must not dispatch another turn"
        );
        assert!(agent.kernel_activations_for_test() >= 1);

        let updates = tokio::time::timeout(std::time::Duration::from_secs(1), listener)
            .await
            .expect("cancellation notice must close the bounded update listener")
            .expect("update listener");
        assert!(
            updates.iter().any(|update| matches!(
                update,
                AgentUpdate::StepFinished { .. } | AgentUpdate::StepFailed { .. }
            )),
            "the in-flight sleep must drain to a terminal tool event"
        );

        let messages = store.load_session("session-1").await.unwrap();
        assert_eq!(
            tool_pairing(&messages),
            (vec!["sleep-1".to_string()], vec!["sleep-1".to_string()])
        );
    }

    #[tokio::test]
    async fn facade_cancel_between_waves_suppresses_later_side_effects_and_pairs_calls() {
        ensure_config();

        let dir = tempfile::tempdir().unwrap();
        let store = crate::store::open_sqlite_session_store(&dir.path().join("session.db"))
            .await
            .unwrap();
        store
            .create_session("session-1", dir.path().to_str().unwrap(), "")
            .await
            .unwrap();

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut tool_context = test_context("cancel_between_waves");
        tool_context.events = UpdateSink::for_ui(tx.clone());
        let work_dir = tool_context.work_dir.clone();
        let flag_slot: Arc<Mutex<Option<Arc<std::sync::atomic::AtomicBool>>>> =
            Arc::new(Mutex::new(None));
        let hook_flag = flag_slot.clone();
        let mock = MockClient::with_responder(|_request, idx| {
            if idx > 0 {
                return Ok((
                    vec![make_text_block("done")],
                    Some(StopReason::EndTurn),
                    None,
                ));
            }
            Ok((
                vec![
                    ContentBlock::ToolUse {
                        id: "w1".into(),
                        name: "bash".into(),
                        input: serde_json::json!({"command": "printf first > wave1.txt"}),
                    },
                    ContentBlock::ToolUse {
                        id: "w2".into(),
                        name: "bash".into(),
                        input: serde_json::json!({"command": "printf second > wave2.txt"}),
                    },
                ],
                Some(StopReason::ToolUse),
                None,
            ))
        });
        let mut agent = Agent::new(
            LlmProvider::Mock(mock),
            tool_context,
            crate::tool::toolset(),
            crate::mcp::MCPToolRouter::new(),
            crate::permission::PermissionManager::try_new(crate::permission::PermissionMode::Auto)
                .unwrap(),
            AgentSystemPrompt::Static("test".to_string()),
        )
        .with_ui_channel(tx)
        .with_session("session-1".to_string(), store.clone())
        .with_post_tool_hook(move |_view, tool, _result, _status| {
            let flag_slot = hook_flag.clone();
            Box::pin(async move {
                if tool.id == "w1"
                    && let Some(flag) = flag_slot.lock().unwrap().as_ref()
                {
                    flag.store(true, std::sync::atomic::Ordering::SeqCst);
                }
                Ok(HookControl::Continue)
            })
        });
        *flag_slot.lock().unwrap() = Some(agent.tool_context.cancel_flag.clone());

        let listener = tokio::spawn(async move {
            let mut updates = Vec::new();
            while let Some(update) = rx.recv().await {
                let cancelling = matches!(
                    &update,
                    AgentUpdate::Info(text) if text == "Cancelled by user"
                );
                updates.push(update);
                if cancelling {
                    break;
                }
            }
            updates
        });

        let outcome = tokio::time::timeout(
            std::time::Duration::from_secs(3),
            agent.agent_loop_detailed(Some(Message::new_text(Role::User, "write twice"))),
        )
        .await
        .expect("between-wave cancellation must be bounded")
        .expect("cancelled loop returns Ok");
        assert_eq!(outcome.stop, AgentLoopStop::Cancelled);
        assert!(agent.kernel_activations_for_test() >= 1);

        let updates = tokio::time::timeout(std::time::Duration::from_secs(1), listener)
            .await
            .expect("cancellation notice must close the bounded update listener")
            .expect("update listener");
        assert_eq!(
            updates
                .iter()
                .filter(|update| matches!(
                    update,
                    AgentUpdate::StepFinished { tool_id, .. } if tool_id == "w1"
                ))
                .count(),
            1
        );
        assert!(
            !updates.iter().any(|update| matches!(
                update,
                AgentUpdate::StepFinished { tool_id, .. } if tool_id == "w2"
            )),
            "the later wave must not execute after cancellation"
        );
        assert!(
            updates.iter().any(|update| matches!(
                update,
                AgentUpdate::StepFailed { tool_id, .. } if tool_id == "w2"
            )),
            "the suppressed call still needs one paired terminal result"
        );

        let messages = store.load_session("session-1").await.unwrap();
        let (uses, results) = tool_pairing(&messages);
        assert_eq!(uses, vec!["w1".to_string(), "w2".to_string()]);
        assert_eq!(results, uses);
        assert_eq!(
            tool_result_for(&messages, "w2").as_deref(),
            Some("Cancelled by user")
        );
        assert_eq!(
            std::fs::read_to_string(work_dir.join("wave1.txt")).unwrap(),
            "first"
        );
        assert!(
            !work_dir.join("wave2.txt").exists(),
            "the later wave must not reach the shell"
        );
    }

    /// A refusal is surfaced as an error, but the assistant message that asked
    /// for a tool is already persisted: it still has to be answered.
    #[tokio::test]
    async fn refusal_pairs_every_tool_use() {
        ensure_config();
        use crate::tool::test_support::test_context;

        let dir = tempfile::tempdir().unwrap();
        let store = crate::store::open_sqlite_session_store(&dir.path().join("session.db"))
            .await
            .unwrap();
        store
            .create_session("session-1", dir.path().to_str().unwrap(), "")
            .await
            .unwrap();

        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let mut tool_context = test_context("refusal_pairs_tools");
        tool_context.events = UpdateSink::for_ui(tx.clone());

        let mock = MockClient::new(vec![(
            vec![
                make_text_block("I will not do that"),
                ContentBlock::ToolUse {
                    id: "t1".to_string(),
                    name: "read_file".to_string(),
                    input: serde_json::json!({ "path": "a.txt" }),
                },
            ],
            Some(StopReason::Refusal),
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
        .with_ui_channel(tx)
        .with_session("session-1".to_string(), store.clone());

        agent
            .agent_loop(Some(Message::new_text(Role::User, "read a.txt")))
            .await
            .expect_err("a refusal is surfaced as an error");

        let messages = store.load_session("session-1").await.unwrap();
        assert_eq!(
            tool_pairing(&messages),
            (vec!["t1".to_string()], vec!["t1".to_string()]),
            "a refused turn must still answer the tool call it asked for"
        );
        assert_eq!(
            tool_result_for(&messages, "t1").as_deref(),
            Some("Not executed: the model refused this request")
        );
    }

    /// Once the continuation budget is exhausted, a truncated response finishes
    /// the turn — including the tool call it may have emitted mid-truncation,
    /// which the recovery branch above would otherwise have executed.
    #[tokio::test]
    async fn truncated_turn_pairs_every_tool_use() {
        ensure_config();
        use crate::tool::test_support::test_context;

        let dir = tempfile::tempdir().unwrap();
        let store = crate::store::open_sqlite_session_store(&dir.path().join("session.db"))
            .await
            .unwrap();
        store
            .create_session("session-1", dir.path().to_str().unwrap(), "")
            .await
            .unwrap();

        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let mut tool_context = test_context("truncated_pairs_tools");
        tool_context.events = UpdateSink::for_ui(tx.clone());

        // Every call is truncated with a fresh tool call, so the continuation
        // path runs the tool for the first `MAX_CONTINUATION_ATTEMPTS` turns and
        // the last one is left for the give-up arm.
        let mock = MockClient::with_responder(move |_request, idx| {
            Ok((
                vec![ContentBlock::ToolUse {
                    id: format!("t{idx}"),
                    name: "read_file".to_string(),
                    input: serde_json::json!({ "path": "missing.txt" }),
                }],
                Some(StopReason::MaxTokens),
                None,
            ))
        });

        let mut agent = Agent::new(
            LlmProvider::Mock(mock),
            tool_context,
            crate::tool::toolset(),
            crate::mcp::MCPToolRouter::new(),
            crate::permission::PermissionManager::try_new(crate::permission::PermissionMode::Auto)
                .unwrap(),
            AgentSystemPrompt::Static("test".to_string()),
        )
        .with_ui_channel(tx)
        .with_session("session-1".to_string(), store.clone());

        agent
            .agent_loop(Some(Message::new_text(Role::User, "read a.txt")))
            .await
            .expect("an exhausted continuation finishes the turn");

        let messages = store.load_session("session-1").await.unwrap();
        let (uses, results) = tool_pairing(&messages);
        assert!(
            uses.len() > 1,
            "the continuation path must have executed at least one turn: {uses:?}"
        );
        assert_eq!(
            uses, results,
            "every truncated turn's tool call needs a matching result"
        );
        let last = uses.last().expect("at least one tool call");
        assert_eq!(
            tool_result_for(&messages, last).as_deref(),
            Some("Not executed: output was truncated before this tool ran"),
            "the call the agent gave up on is answered, not executed"
        );
    }

    #[tokio::test]
    async fn agent_loop_plan_mode_denies_write() {
        ensure_config();
        use tact_protocol::AgentUpdate;

        use crate::tool::test_support::test_context;

        let context = test_context("agent_plan_deny");
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut tool_context = context;
        tool_context.events = UpdateSink::for_ui(tx.clone());

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
        tool_context.events = UpdateSink::for_ui(tx.clone());

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
            crate::runtime::provider_token_usage(&usage),
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
        tool_context.events = UpdateSink::for_ui(tx.clone());

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
        let sources = crate::config::InstructionSources { agents_md: false };
        assert!(assemble_agents_md_prompt(dir.path(), &sources).is_empty());
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

    async fn seed_unmatched_assistant_tool_calls(
        store: &crate::store::DynSessionStore,
        session_id: &str,
        call_ids: &[&str],
    ) {
        let assistant = AgentMessage::new(
            AgentRole::Assistant,
            MessageOrigin::Assistant,
            call_ids
                .iter()
                .map(|call_id| AgentContentBlock::ToolUse {
                    id: (*call_id).to_string(),
                    name: "read_file".to_string(),
                    input: serde_json::json!({"path": "missing.txt"}),
                })
                .collect(),
        );
        store
            .append_agent_message(session_id, &assistant, 0)
            .await
            .unwrap();
    }

    async fn assert_one_grouped_interrupted_result(
        store: &crate::store::DynSessionStore,
        session_id: &str,
        expected_ids: &[&str],
    ) {
        let messages = store.load_agent_session(session_id).await.unwrap();
        let grouped = messages
            .iter()
            .filter(|message| message.origin == MessageOrigin::ToolResult)
            .collect::<Vec<_>>();
        assert_eq!(
            grouped.len(),
            1,
            "crash reconciliation must persist one grouped result: {messages:?}"
        );
        assert_eq!(
            grouped[0].tool_result_ids().collect::<Vec<_>>(),
            expected_ids
        );
        assert!(
            grouped[0].content.iter().all(|block| matches!(
                block,
                AgentContentBlock::ToolResult { content, .. }
                    if content.contains("interrupted before result commit")
            )),
            "every missing call must carry the interrupted reason: {:?}",
            grouped[0].content
        );
    }

    #[tokio::test]
    async fn local_session_reconciles_a_durable_unmatched_tool_call_before_the_request() {
        ensure_config();
        let dir = tempfile::tempdir().unwrap();
        let store = crate::store::open_sqlite_session_store(&dir.path().join("session.db"))
            .await
            .unwrap();
        store
            .create_session("session-1", dir.path().to_str().unwrap(), "")
            .await
            .unwrap();
        seed_unmatched_assistant_tool_calls(&store, "session-1", &["orphan-1", "orphan-2"]).await;

        let requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::<Vec<Message>>::new()));
        let captured = requests.clone();
        let mock = MockClient::with_responder(move |request, _idx| {
            captured
                .lock()
                .expect("request capture")
                .push(request.messages.clone());
            Ok((
                vec![make_text_block("resumed")],
                Some(StopReason::EndTurn),
                None,
            ))
        });
        let mut agent = Agent::new(
            LlmProvider::Mock(mock),
            test_context("local_crash_recovery"),
            crate::tool::toolset(),
            crate::mcp::MCPToolRouter::new(),
            crate::permission::PermissionManager::try_new(
                crate::permission::PermissionMode::Default,
            )
            .unwrap(),
            AgentSystemPrompt::Static("test".to_string()),
        )
        .with_session("session-1".to_string(), store.clone());

        agent
            .agent_loop_detailed(Some(Message::new_text(Role::User, "resume")))
            .await
            .expect("resumed local activation completes");

        let first_request = requests
            .lock()
            .expect("request capture")
            .first()
            .cloned()
            .expect("resumed request");
        let (uses, results) = tool_pairing(&first_request);
        assert_eq!(uses, ["orphan-1", "orphan-2"]);
        assert_eq!(results, ["orphan-1", "orphan-2"]);
        assert_eq!(
            tool_pairing(&store.load_session("session-1").await.unwrap()),
            (
                vec!["orphan-1".to_string(), "orphan-2".to_string()],
                vec!["orphan-1".to_string(), "orphan-2".to_string()]
            )
        );
        assert_one_grouped_interrupted_result(&store, "session-1", &["orphan-1", "orphan-2"]).await;
    }

    #[tokio::test]
    async fn responses_session_reconciles_a_durable_unmatched_tool_call_before_the_request() {
        ensure_config();
        let server = MockServer::start().await;
        let completed = serde_json::json!({
            "type": "response.completed",
            "sequence_number": 1,
            "response": {
                "id": "resp_resume",
                "object": "response",
                "created_at": 1,
                "completed_at": 2,
                "status": "completed",
                "model": "gpt-5",
                "output": [{
                    "type": "message",
                    "id": "msg_resume",
                    "role": "assistant",
                    "status": "completed",
                    "content": [{
                        "type": "output_text",
                        "annotations": [],
                        "logprobs": null,
                        "text": "resumed"
                    }]
                }],
                "usage": {
                    "input_tokens": 4,
                    "input_tokens_details": { "cached_tokens": 0 },
                    "output_tokens": 1,
                    "output_tokens_details": { "reasoning_tokens": 0 },
                    "total_tokens": 5
                }
            }
        });
        Mock::given(method("POST"))
            .and(path("/responses"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .set_body_string(format!("data: {completed}\n\n")),
            )
            .expect(1)
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
        seed_unmatched_assistant_tool_calls(&store, "session-1", &["orphan-r1", "orphan-r2"]).await;

        let mut agent = responses_test_agent("responses_crash_recovery", &server.uri())
            .with_session("session-1".to_string(), store.clone());
        agent
            .agent_loop_detailed(Some(Message::new_text(Role::User, "resume")))
            .await
            .expect("resumed Responses activation completes");

        let requests = server.received_requests().await.expect("received requests");
        assert_eq!(requests.len(), 1);
        let body: serde_json::Value =
            serde_json::from_slice(&requests[0].body).expect("request body is JSON");
        let outputs = body["input"]
            .as_array()
            .expect("Responses input")
            .iter()
            .filter(|item| item["type"] == "function_call_output")
            .collect::<Vec<_>>();
        assert_eq!(outputs.len(), 2, "both missing calls need outputs: {body}");
        for call_id in ["orphan-r1", "orphan-r2"] {
            assert!(
                outputs.iter().any(|item| {
                    item["call_id"] == call_id
                        && item["output"].as_str().is_some_and(|output| {
                            output.contains("interrupted before result commit")
                        })
                }),
                "missing interrupted output for {call_id}: {body}"
            );
        }
        assert_one_grouped_interrupted_result(&store, "session-1", &["orphan-r1", "orphan-r2"])
            .await;
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

    // ——— lifecycle hook wiring (Stop / SessionEnd / PreCompact / PostCompact) ———

    fn hook_test_agent(context_name: &str) -> Agent {
        ensure_config();
        let context = test_context(context_name);
        Agent::new(
            LlmProvider::Mock(MockClient::new(vec![])),
            context,
            crate::tool::toolset(),
            crate::mcp::MCPToolRouter::new(),
            crate::permission::PermissionManager::try_new(
                crate::permission::PermissionMode::Default,
            )
            .unwrap(),
            AgentSystemPrompt::Static("hook test".into()),
        )
    }

    #[tokio::test]
    async fn stop_hook_block_requests_continuation() {
        let agent = hook_test_agent("stop_hook_block").with_stop(|_agent| {
            Box::pin(async move { Ok(HookControl::Block("keep going".into())) })
        });
        let mut agent = agent;
        let control = agent.dispatch_stop_hooks().await.unwrap();
        assert_eq!(control, HookControl::Block("keep going".into()));
    }

    #[tokio::test]
    async fn stop_hook_continue_defaults_to_stop() {
        let mut agent = hook_test_agent("stop_hook_continue");
        let control = agent.dispatch_stop_hooks().await.unwrap();
        assert_eq!(control, HookControl::Continue);
    }

    #[tokio::test]
    async fn session_end_hook_is_observational() {
        let mut agent = hook_test_agent("session_end_obs").with_session_end(|_agent| {
            Box::pin(async move { Ok(HookControl::Block("ignored".into())) })
        });
        // A block must not fail the dispatch — the session is ending.
        agent.dispatch_session_end_hooks().await.unwrap();
    }

    #[tokio::test]
    async fn task_completed_hook_is_observational() {
        let mut agent = hook_test_agent("task_completed_obs").with_task_completed(|_agent| {
            Box::pin(async move { Ok(HookControl::Block("ignored".into())) })
        });
        // A block must not fail the dispatch — the task already completed.
        agent.dispatch_task_completed_hooks().await.unwrap();
    }

    #[test]
    fn new_hook_builders_register_variants() {
        let agent = hook_test_agent("new_hook_builders")
            .with_post_tool_failure(|_agent, _tool_use, _error| {
                Box::pin(async { Ok(HookControl::Continue) })
            })
            .with_notification(|_agent, _ctx| Box::pin(async { Ok(HookControl::Continue) }))
            .with_task_completed(|_agent| Box::pin(async { Ok(HookControl::Continue) }));
        assert_eq!(agent.hooks_by_type(HookTypes::PostToolUseFailure).len(), 1);
        assert_eq!(agent.hooks_by_type(HookTypes::Notification).len(), 1);
        assert_eq!(agent.hooks_by_type(HookTypes::TaskCompleted).len(), 1);
    }

    #[tokio::test]
    async fn pre_compact_block_vetoes_compaction() {
        let mut agent = hook_test_agent("pre_compact_veto").with_pre_compact(|_agent, trigger| {
            assert_eq!(trigger, CompactTrigger::Auto);
            Box::pin(async move { Ok(HookControl::Block("no auto compact".into())) })
        });
        // PreCompact veto returns Ok without compacting (has_compacted stays false).
        agent
            .compact_history_with_trigger(CompactTrigger::Auto, None)
            .await
            .unwrap();
        assert!(!agent.runtime.compact_state.has_compacted);
    }
}
