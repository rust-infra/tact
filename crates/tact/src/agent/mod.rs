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
use tact_agent_api::{
    AgentState, MessageOrigin, ModelCallOptions, ModelCapabilities, ReasoningEffort, RunId,
    RunInput, RunStop, SessionRef,
};
use tact_agent_core::KernelError;
use tact_llm::{
    LlmProvider, Message, MessageKind, OpenAiReasoningEffort, ProviderConversationState,
    ProviderInfo, ProviderKind, Role,
};
use tact_protocol::{AgentUpdate, UpdateSender, UpdateSink};

use crate::{
    ToolSpec,
    cancel::CancelController,
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
        compaction_adapter::TactCompactor,
        composition::{TactKernelComposition, TactKernelConfig},
        facade_state::{
            FacadeActivationState, FacadeReceiptTracker, FacadeReceipts, FacadeSidecar,
        },
        model_adapter::ActiveModelSnapshot,
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

fn new_run_id() -> RunId {
    RunId::new(uuid::Uuid::new_v4().to_string())
}

/// Shared state for a running agent session.
///
/// Holds the LLM client, conversation context, compaction and recovery
/// state, the permission manager, and an optional TUI update channel.
pub struct AgentRuntime {
    pub client: LlmProvider,
    pub(crate) context: Vec<Message>,
    pub compact_state: CompactState,
    pub recovery_state: RecoveryState,
    pub(crate) permission_manager: PermissionAuthority,
    /// The sensitive-path guard, derived from the permission settings at
    /// construction and inherited by subagents through the same snapshot that
    /// carries the permission context. Evaluated **before** modes, hooks and
    /// rules, so nothing can reach a credential file.
    pub security: crate::security::sensitive::Scanner,
    pub(crate) stats: Arc<RwLock<SessionStats>>,
    /// Where this agent's updates go: the answering frontend, if any, plus
    /// any read-only observers.
    pub(crate) events: UpdateSink,
    pub(crate) cancel_flag: Arc<CancelController>,
    pub session_store: Option<DynSessionStore>,
    /// Set together with [`Self::session_store`] via [`Agent::with_session`] at startup.
    pub(crate) session_id: Option<String>,
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
    pub(crate) pending_subagent_results: Arc<Mutex<VecDeque<SubagentResult>>>,
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
    model_provider_info: Option<ProviderInfo>,
    stable_provider_enabled: bool,
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
        let cancel_flag = Arc::new(CancelController::new());
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
            model_provider_info: None,
            stable_provider_enabled: false,
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

    pub(crate) fn with_provider_info(mut self, provider_info: ProviderInfo) -> Self {
        self.model_provider_info = Some(provider_info);
        self
    }

    /// Enable the stable provider snapshot for a production agent.
    pub fn with_stable_provider(mut self) -> Self {
        self.stable_provider_enabled = true;
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
        let _ = self.try_set_model(model);
    }

    /// Prepare and commit a model switch. The previous model remains active
    /// when the replacement provider cannot be built.
    pub fn try_set_model(&mut self, model: String) -> Result<()> {
        self.try_set_model_with_provider_info(model, None)
    }

    /// Prepare and commit a model switch with the provider snapshot resolved
    /// from the candidate config, without publishing that config first.
    pub fn try_set_model_with_provider_info(
        &mut self,
        model: String,
        provider_info: Option<ProviderInfo>,
    ) -> Result<()> {
        if model.trim().is_empty() {
            return Ok(());
        }
        let previous_model = self.agent_settings.model.clone();
        let previous_provider_info = self.model_provider_info.clone();
        self.agent_settings.model = model;
        if provider_info.is_some() {
            self.model_provider_info = provider_info;
        }
        if !matches!(self.runtime.client, LlmProvider::Mock(_))
            && self.active_model_snapshot().is_none()
        {
            self.agent_settings.model = previous_model;
            self.model_provider_info = previous_provider_info;
            anyhow::bail!("replacement provider could not be built for the selected model");
        }
        self.emit_model_status();
        Ok(())
    }

    /// The model this agent is currently calling.
    ///
    /// Configured at construction and rewritten by [`Self::set_model`], so it is
    /// the value a `SessionStart` hook payload should carry as `model`.
    #[must_use]
    pub fn model(&self) -> &str {
        &self.agent_settings.model
    }

    /// The sink for this agent's UI and observer updates.
    pub fn events(&self) -> &UpdateSink {
        &self.runtime.events
    }

    /// The current persisted session id, when this agent has a session store.
    pub fn session_id(&self) -> Option<&str> {
        self.runtime.session_id.as_deref()
    }

    /// Read-only access to the current transcript.
    pub fn context(&self) -> &[Message] {
        &self.runtime.context
    }

    /// Replace the transcript for integration-test setup.
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn replace_context_for_test(&mut self, context: Vec<Message>) {
        self.runtime.context = context;
    }

    /// Append one transcript message for integration-test setup.
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn push_context_for_test(&mut self, message: Message) {
        self.runtime.context.push(message);
    }

    /// Shared session statistics for read-only frontends.
    pub fn stats(&self) -> Arc<RwLock<SessionStats>> {
        self.runtime.stats.clone()
    }

    /// Read-only access to the session cancellation authority.
    pub fn cancel_controller(&self) -> &Arc<CancelController> {
        &self.runtime.cancel_flag
    }

    /// A cloneable cancellation handle for background tasks.
    pub fn cancel_handle(&self) -> Arc<CancelController> {
        self.runtime.cancel_flag.clone()
    }

    pub fn cancel(&self) {
        self.runtime.cancel_flag.cancel();
    }

    pub fn is_cancelled(&self) -> bool {
        self.runtime.cancel_flag.is_cancelled()
    }

    /// Change the live permission policy without exposing the manager itself.
    pub fn set_permission_mode(&mut self, mode: crate::permission::PermissionMode) {
        self.runtime.permission_manager.set_mode(mode);
    }

    /// Shared background-subagent result queue for detached child tasks.
    pub fn pending_subagent_results(&self) -> Arc<Mutex<VecDeque<SubagentResult>>> {
        self.runtime.pending_subagent_results.clone()
    }

    /// Starts a fresh user-committed activation.
    ///
    /// Call this before [`Self::agent_loop`] or [`Self::agent_loop_detailed`]
    /// when the call represents a new user turn. It intentionally does nothing
    /// while another activation is running, so a late command cannot revive a
    /// cancelled activation. A cancelled idle session keeps its cancellation
    /// until this method is called; direct low-level callers therefore observe
    /// `Cancelled` rather than silently starting a new turn.
    pub fn begin_user_turn(&self) {
        self.runtime.cancel_flag.begin_turn();
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
    pub fn set_reasoning_effort(&mut self, effort: Option<ReasoningEffort>) {
        self.agent_settings.reasoning_effort = effort.map(Into::into);
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
            reasoning_effort: self.agent_settings.reasoning_effort.map(Into::into),
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
    pub fn with_ui_channel(mut self, tx: impl Into<UpdateSender>) -> Self {
        let tx = tx.into();
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
    pub fn with_event_observer(mut self, tx: impl Into<UpdateSender>) -> Self {
        self.runtime.events = UpdateSink::observing(tx.into());
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
        if self.stable_provider_enabled {
            let snapshot = self
                .active_model_snapshot()
                .context("active provider snapshot is unavailable")?;
            if snapshot.metadata.protocol() != tact_llm::WireProtocol::OpenAiResponses {
                anyhow::bail!(
                    "provider state is stored for provider '{}', but the active provider is not Responses",
                    inner.provider
                );
            }
            if inner.provider != "openai_responses" {
                anyhow::bail!(
                    "provider state is bound to provider '{}', expected 'openai_responses'",
                    inner.provider
                );
            }
            if !snapshot.metadata.endpoint().matches(&inner.base_url) {
                anyhow::bail!("provider state is bound to a different canonical endpoint");
            }
            return Ok(());
        }
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
        if self.stable_provider_enabled
            && let Some(snapshot) = self.active_model_snapshot()
        {
            return snapshot.metadata.protocol() == tact_llm::WireProtocol::OpenAiResponses;
        }
        matches!(self.runtime.client, LlmProvider::OpenAiResponses(_))
    }

    fn kernel_model_options(&self) -> ModelCallOptions {
        ModelCallOptions {
            reasoning_effort: self.agent_settings.reasoning_effort.map(Into::into),
            thinking_budget: (self.thinking_budget() > 0).then_some(self.thinking_budget() as u32),
            ..ModelCallOptions::default()
        }
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
            session: self.runtime.session_id.clone().map(SessionRef::new),
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

    fn active_model_snapshot(&self) -> Option<ActiveModelSnapshot> {
        if !self.stable_provider_enabled || matches!(self.runtime.client, LlmProvider::Mock(_)) {
            return None;
        }
        let info = self
            .model_provider_info
            .clone()
            .or_else(tact_llm::try_get_provider)?;
        let limits = tact_llm::ModelLimits::new(
            (self.model_context_window() != 0).then_some(self.model_context_window()),
            self.max_tokens(),
        )
        .ok()?;
        let auth = if info.api_key.is_empty() {
            tact_llm::try_active_credential_provider()
                .map(tact_llm::legacy_credential_source)
                .map(tact_llm::Authentication::Refreshable)
                .unwrap_or(tact_llm::Authentication::None)
        } else {
            tact_llm::Authentication::Static(tact_llm::Credential::api_key(&info.api_key).ok()?)
        };
        let builder = match (info.provider.clone(), info.protocol) {
            (ProviderKind::Anthropic, _) => tact_llm::anthropic(self.agent_settings.model.clone()),
            (ProviderKind::DeepSeek, tact_llm::OpenAiProtocol::ChatCompletions) => {
                tact_llm::deepseek_chat(self.agent_settings.model.clone())
            }
            (ProviderKind::DeepSeek, tact_llm::OpenAiProtocol::Responses) => {
                tact_llm::deepseek_responses(self.agent_settings.model.clone())
            }
            (ProviderKind::Kimi, tact_llm::OpenAiProtocol::ChatCompletions) => {
                tact_llm::kimi_chat(self.agent_settings.model.clone())
            }
            (ProviderKind::Kimi, tact_llm::OpenAiProtocol::Responses) => {
                tact_llm::kimi_responses(self.agent_settings.model.clone())
            }
            (ProviderKind::OpenAi, tact_llm::OpenAiProtocol::ChatCompletions) => {
                tact_llm::openai_chat(self.agent_settings.model.clone())
            }
            (ProviderKind::OpenAi, tact_llm::OpenAiProtocol::Responses) => {
                tact_llm::openai_responses(self.agent_settings.model.clone())
            }
            (ProviderKind::Custom(name), tact_llm::OpenAiProtocol::ChatCompletions) => {
                tact_llm::custom_chat(
                    name,
                    self.agent_settings.model.clone(),
                    info.base_url.clone(),
                )
            }
            (ProviderKind::Custom(name), tact_llm::OpenAiProtocol::Responses) => {
                tact_llm::custom_responses(
                    name,
                    self.agent_settings.model.clone(),
                    info.base_url.clone(),
                )
            }
        };
        let mut builder = builder.authentication(auth).limits(limits);
        if !info.base_url.is_empty() {
            builder = builder.base_url(info.base_url.clone());
        }
        if let Some(threshold) = info.responses_compact_threshold {
            builder = builder.responses_compact_threshold(threshold);
        }
        if let Some(session_id) = self.runtime.session_id.as_deref() {
            builder = builder.session_id(tact_llm::SessionId::new(session_id).ok()?);
        }
        let provider = builder.build_product().ok()?;
        let metadata = provider.metadata().clone();
        Some(ActiveModelSnapshot {
            model: Arc::new(provider),
            metadata,
        })
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
        let model_snapshot = self.active_model_snapshot();
        if let Some(snapshot) = &model_snapshot {
            tool_context.vision = snapshot.metadata.supports_vision();
        }
        let capabilities = model_snapshot
            .as_ref()
            .map(|snapshot| snapshot.metadata.capabilities().clone())
            .unwrap_or_else(|| ModelCapabilities {
                model: model_name.clone(),
                context_window: (self.model_context_window() != 0)
                    .then_some(self.model_context_window()),
                max_output_tokens: self.max_tokens(),
                supports_streaming: true,
                supports_native_compaction: self.native_compaction_available(),
            });

        let composition = TactKernelComposition::new(TactKernelConfig {
            provider: self.runtime.client.clone(),
            capabilities,
            model_snapshot,
            prompt,
            compactor,
            model_options: self.kernel_model_options(),
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
        let activation = self.runtime.cancel_flag.begin_activation();
        let cancel = activation.token();
        let run_id = new_run_id();
        let mut guard = FacadeStateGuard::take(self, receipts);
        let unwound = AssertUnwindSafe(async {
            composition
                .kernel()
                .compact(
                    guard.state_mut(),
                    &SessionRef::new(session_id),
                    run_id,
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
        let activation = self.runtime.cancel_flag.begin_activation();
        let cancel = activation.token();
        let input = RunInput {
            run_id: new_run_id(),
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
    ///
    /// For a new user turn, call [`Self::begin_user_turn`] first. Keeping that
    /// reset explicit preserves cancel-before-activation semantics for direct
    /// callers instead of silently clearing a pending cancellation.
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
mod tests;
