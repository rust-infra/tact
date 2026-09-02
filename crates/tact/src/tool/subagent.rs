use crate::tool::{
    ArgumentSummaryPolicy, DetailPolicy, LiveOutputPolicy, OutputPolicy, PermissionPolicy,
    PermissionPromptPolicy, PopupPolicy, ResourcePolicy, ToolDomain, ToolMetadata,
    ToolPresentation,
};
use std::{
    path::PathBuf,
    sync::{Arc, atomic::Ordering},
};

use anyhow::{Context, Result, bail};
use schemars::JsonSchema;
use serde::Deserialize;
use tact_llm::{ApiKeyProvider, Client, Message, Role, get_llm_client};
use tact_protocol::{AgentUpdate, ToolVisualKind};
use tool_refactor_macros::tool;
use tracing::warn;

use crate::{
    Agent, AgentSystemPrompt,
    consts::TactPath,
    extract_text,
    mcp::MCPToolRouter,
    permission::{PermissionManager, PermissionMode, settings::PermissionSettings},
    store::{SessionLock, open_sqlite_session_store},
    subagent::{SubagentResult, SubagentStatus},
    tool::ToolContext,
    worktree::WorktreeRecord,
};

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SubagentInput {
    #[schemars(description = "Prompt for the subagent.")]
    pub prompt: String,
    #[schemars(description = "Short description of the task.")]
    #[allow(dead_code)]
    pub description: Option<String>,
    /// When true, return `async_launched { id }` immediately; the subagent
    /// keeps running and its result is re-injected into the parent context
    /// on completion.
    #[schemars(description = "Run the subagent in the background and return an async handle.")]
    #[serde(default)]
    pub run_in_background: Option<bool>,
    /// Cap on nested agent-loop turns (prevents runaway subagents).
    #[schemars(description = "Maximum number of agent-loop turns for this subagent.")]
    #[serde(default)]
    pub max_turns: Option<u32>,
    /// Resume an existing subagent session id (from a prior `async_launched`).
    #[schemars(description = "Resume an existing subagent session by id.")]
    #[serde(default)]
    pub resume: Option<String>,
    /// When true, the subagent runs inside a fresh git worktree lane
    /// (`subagent-<child_id>`) instead of the shared workspace, so parallel
    /// subagents / same-turn edits do not race the parent tree. Requires the
    /// work_dir to be a git repository.
    #[schemars(description = "Run the subagent inside an isolated git worktree.")]
    #[serde(default)]
    pub worktree: Option<bool>,
    /// Reference a declarative subagent definition by name (`plugin:<name>`
    /// for plugin agents, or a unique local name). The definition body
    /// becomes the subagent's system prompt, and its `tools` / `model` /
    /// `permissionMode` frontmatter override the defaults.
    #[schemars(description = "Name of a declarative agent definition to run.")]
    #[serde(default)]
    pub agent: Option<String>,
}

/// Resolves a declarative agent's `model:` frontmatter to a Tact model name.
///
/// Claude Code agents use aliases: `inherit` means "keep the parent's model"
/// (no override); `sonnet` / `opus` / `haiku` name Claude models that Tact
/// cannot address without config mapping, so they are ignored with a warning.
/// Any other value is treated as a concrete model id and passed through.
fn resolve_agent_model(model: &str, agent_name: &str) -> Option<String> {
    match model.to_ascii_lowercase().as_str() {
        "" | "inherit" => None,
        "sonnet" | "opus" | "haiku" => {
            warn!(
                "declarative agent '{agent_name}' requests Claude model '{model}'; \
                 ignoring (Tact has no alias mapping)"
            );
            None
        }
        _ => Some(model.to_string()),
    }
}

/// Worktree lane name prefix for isolated subagents.
const SUBAGENT_WORKTREE_PREFIX: &str = "subagent";

/// A finished subagent session may only be resumed within this window; older
/// sessions are rejected because their context is stale (Claude's 24h expiry).
const RESUME_EXPIRY_HOURS: i64 = 24;

/// Creates (or, on `resume`, reuses) the isolation worktree for a child.
///
/// Returns `Ok(None)` when the caller did not request worktree isolation.
/// Runs synchronously inside the handler so creation failures surface
/// immediately instead of inside a detached background task.
async fn ensure_subagent_worktree(
    ctx: &ToolContext,
    child_id: &str,
    resume: bool,
) -> Result<Option<WorktreeRecord>> {
    let name = format!("{SUBAGENT_WORKTREE_PREFIX}-{child_id}");
    if resume {
        // Reuse the lane from the original run when it exists; otherwise fall
        // through and create one (the original run may not have used
        // isolation).
        if let Ok(record) = ctx.worktree_manager.get(&name).await {
            return Ok(Some(record));
        }
    }
    ctx.worktree_manager
        .create(name.clone(), None, "HEAD".to_string(), child_id.to_string())
        .await
        .with_context(|| {
            format!(
                "failed to create isolation worktree for subagent {child_id} \
                 (worktree isolation requires a git repository at {})",
                ctx.work_dir.display()
            )
        })?;
    ctx.worktree_manager
        .get(&name)
        .await
        .context("failed to read created worktree")
        .map(Some)
}

/// Appends a structured worktree note to a subagent summary so the parent LLM
/// knows where the isolated work landed.
fn with_worktree_note(summary: String, worktree: Option<&WorktreeRecord>) -> String {
    match worktree {
        Some(wt) => format!("{summary}\n\n(worktree: {} at {})", wt.name, wt.path),
        None => summary,
    }
}

pub const SPAWN_SUBAGENT_METADATA: ToolMetadata = ToolMetadata {
    name: "spawn_subagent",
    description: "Spawn a subagent with fresh context. By default the call blocks until the subagent finishes and returns its summary. Set run_in_background: true to return immediately and have the summary re-injected later — use this to run several independent subagents in parallel. Set worktree: true to isolate the subagent in its own git worktree (safe for parallel edits). Set agent: <name> to run a declarative agent definition from .tact/agents/*.md or an installed plugin (plugin:<name>); its body becomes the system prompt and its tools/model/permissionMode frontmatter apply.",
    permission: PermissionPolicy::High,
    permission_prompt: PermissionPromptPolicy::Json,
    resources: ResourcePolicy::Barrier,
    domain: ToolDomain::Subagent,
    presentation: ToolPresentation {
        visual_kind: ToolVisualKind::Subagent,
        display_name: "🤖 Subagent",
        live_output: LiveOutputPolicy::FullTranscript,
        detail: DetailPolicy::Result,
        popup: PopupPolicy::SubagentTranscript,
        compact_result_to_meta: false,
    },
    output: OutputPolicy::PersistLargeOutput,
    argument_summary: ArgumentSummaryPolicy::SubagentPrompt { field: "prompt" },
};

/// Extract the subagent's final summary from its last assistant message.
fn extract_summary(subagent: &Agent, max_turns_reached: bool) -> String {
    let summary = subagent
        .runtime
        .context
        .iter()
        .rev()
        .find(|message| matches!(message.role, Role::Assistant))
        .map(|message| extract_text(&message.content))
        .filter(|text| !text.is_empty())
        .unwrap_or_else(|| "(no summary)".to_string());
    if max_turns_reached {
        format!("{summary} (max_turns reached)")
    } else {
        summary
    }
}

/// A subagent is only reported successful when its agent loop returned `Ok`
/// **and** it was not cooperatively cancelled. A cancelled child can still
/// exit its loop cleanly (`Ok`) after the flag is set, so the raw result alone
/// would misreport a cancellation as success.
fn terminal_success(success: bool, cancelled: bool) -> bool {
    success && !cancelled
}

#[tool]
/// # Errors
///
/// Returns an error if:
/// - The LLM client cannot be obtained.
/// - The permission manager cannot be created.
/// - The session store cannot be opened.
/// - `run_in_background` is requested without a parent agent runtime.
/// - The subagent agent loop encounters an error.
pub async fn spawn_subagent(mut ctx: ToolContext, input: SubagentInput) -> Result<String> {
    let settings = crate::config::settings();

    let (client, agent_overrides) = if let Some(sa) = &settings.agent.subagent {
        let client = Client::new(
            sa.provider.to_profile(),
            Arc::new(ApiKeyProvider::new(sa.provider.api_key.clone())),
        )
        .await?;
        let mut agent_settings = settings.agent.clone();
        agent_settings.model = sa.provider.model.clone();
        agent_settings.max_tokens = sa.max_tokens;
        agent_settings.thinking_budget = sa.thinking_budget;
        agent_settings.reasoning_effort = sa.reasoning_effort;
        (client, agent_settings)
    } else {
        let client = get_llm_client().await?;
        (client, settings.agent.clone())
    };

    // Inherit the parent's permission context (Claude-style). The snapshot is
    // stamped by `execute_tool_call` just before this tool runs, so it carries
    // the parent's *current* mode / allow-list / settings. Orphan/test
    // contexts without a parent agent fall back to the pre-inheritance
    // behavior: a fresh `Default` manager with settings loaded from disk.
    let pm = match ctx.permission_snapshot.clone() {
        Some(snapshot) => PermissionManager::from_snapshot(snapshot),
        None => PermissionManager::try_new_with_settings(
            PermissionMode::Default,
            PermissionSettings::load(&TactPath::new(&ctx.work_dir)),
        )?,
    };

    // Resume an existing child, or mint a fresh session id.
    let child_id = match input.resume.clone() {
        Some(id) => id,
        None => uuid::Uuid::new_v4().to_string(),
    };
    let resume = input.resume.is_some();

    // A resume must target a prior run that has already reached a terminal
    // state and has not expired. Resuming a still-running child would race the
    // session lock (two loops on one session); resuming an unknown id would
    // silently mint a fresh child instead of a follow-up; resuming a session
    // older than `RESUME_EXPIRY_HOURS` would feed stale context to the model.
    if resume {
        match ctx.subagent_manager.get(&child_id).await? {
            Some(record) if record.status == SubagentStatus::Running => {
                bail!(
                    "cannot resume subagent {child_id}: it is still running; \
                     cancel it or wait for it to finish first"
                );
            }
            Some(record) => {
                if let Some(finished_at) = record.finished_at
                    && chrono::Utc::now() - finished_at
                        > chrono::Duration::hours(RESUME_EXPIRY_HOURS)
                {
                    bail!(
                        "cannot resume subagent {child_id}: it finished more than \
                         {RESUME_EXPIRY_HOURS}h ago; spawn a new subagent instead"
                    );
                }
            }
            None => {
                bail!(
                    "cannot resume subagent {child_id}: no prior run record; \
                     resume only works on a finished `async_launched` child"
                );
            }
        }
    }

    // The child's session identity belongs to the main workspace, not the
    // isolation lane — capture it before `work_dir` may be overridden below.
    let ref_id = ctx.session_id.as_deref().unwrap_or("").to_string();
    let root_dir = ctx.work_dir.display().to_string();
    let fallback_db = TactPath::new(&ctx.work_dir).session_db_path();

    // Optional worktree isolation: create/reuse the lane synchronously so
    // failures surface immediately, then point the child at the lane.
    let worktree = if input.worktree == Some(true) {
        ensure_subagent_worktree(&ctx, &child_id, resume).await?
    } else {
        None
    };
    if let Some(wt) = worktree.as_ref() {
        ctx.work_dir = PathBuf::from(&wt.path);
    }

    // The child's system prompt is built after the work_dir override so a
    // worktree-isolated subagent is told exactly where it is working. A
    // declarative agent definition replaces the default generic prompt with
    // its own body; the caller's `prompt` becomes the user task.
    let mut pm = pm;
    let agent_definition = match input.agent.as_deref() {
        Some(name) => {
            let registry = crate::agent_def::lock_agent_definitions(&ctx.agent_registry);
            Some(registry.get(name).cloned().with_context(|| {
                format!(
                    "unknown declarative agent '{name}'; available:\n{}",
                    registry.describe_available()
                )
            })?)
        }
        None => None,
    };
    if let Some(definition) = agent_definition.as_ref()
        && let Some(mode) = definition.permission_mode
        // Auto stays sticky; other inherited modes may be overridden.
        && pm.mode() != PermissionMode::Auto
    {
        pm.set_mode(mode);
    }
    let mut system_prompt = match agent_definition.as_ref() {
        Some(definition) => {
            format!(
                "{}\n\nUser task:\n{}",
                definition.body.trim(),
                input.prompt.trim()
            )
        }
        None => format!(
            "You are a coding subagent at {}. Complete the given task, then summarize your findings.",
            ctx.work_dir.display()
        ),
    };

    // Claude Code plugin `SubagentStart` hooks may inject context into the
    // child system prompt (e.g. ponytail injects its mode). A `Block` from a
    // Rust-registered hook fails the spawn; plugin command hooks already
    // normalize failures to Continue.
    if !ctx.subagent_start_hooks.is_empty() {
        let mut start_ctx = crate::hook::SubagentStartContext {
            name: input.agent.clone().unwrap_or_else(|| child_id.clone()),
            prompt: input.prompt.clone(),
            system_prompt: system_prompt.clone(),
        };
        for hook in &ctx.subagent_start_hooks {
            match hook(&mut start_ctx).await? {
                crate::hook::HookControl::Continue => {}
                crate::hook::HookControl::Block(reason) => {
                    bail!("subagent start blocked by plugin hook: {reason}");
                }
            }
        }
        system_prompt = start_ctx.system_prompt;
    }

    let store = if let Some(store) = &ctx.session_store {
        store.clone()
    } else {
        open_sqlite_session_store(&fallback_db)
            .await
            .with_context(|| format!("failed to open session store at {}", fallback_db.display()))?
    };
    store
        .ensure_session_row(&child_id, &root_dir, &ref_id)
        .await?;

    let mut agent_overrides = agent_overrides;
    if let Some(definition) = agent_definition.as_ref()
        && let Some(model) = definition.model.as_deref()
        && let Some(resolved) = resolve_agent_model(model, &definition.name)
    {
        agent_overrides.model = resolved;
    }

    let mut subagent = Agent::new(
        client,
        ctx.clone(),
        {
            let definition = agent_definition.as_ref();
            let router = crate::tool::registry::subagent_toolset_for(
                definition.and_then(|d| d.tools.as_deref()),
            );
            if router.tool_specs().is_empty()
                && definition
                    .and_then(|d| d.tools.as_deref())
                    .is_some_and(|tools| !tools.is_empty())
            {
                bail!(
                    "declarative agent '{}' declares tools: {} but none map to a Tact \
                     subagent tool (supported: Read/Glob/Grep, Bash, Edit, Write, Sleep)",
                    definition.expect("definition exists").name,
                    definition
                        .expect("definition exists")
                        .tools
                        .as_ref()
                        .expect("tools exist")
                        .join(", ")
                );
            }
            router
        },
        MCPToolRouter::new(),
        pm,
        AgentSystemPrompt::Static(system_prompt),
    )
    .with_agent_settings(agent_overrides)
    .with_max_turns(input.max_turns)
    .with_session(child_id.clone(), store.clone());

    // Expose the child's cooperative cancel flag so `cancel_subagent` /
    // `/subagent_cancel` / the TUI button can request cancellation. The
    // handle is unregistered when the child finishes.
    let cancel_flag = subagent.runtime.cancel_flag.clone();
    ctx.subagent_manager
        .register_cancel_handle(&child_id, cancel_flag.clone());

    // Tag UI traffic so the TUI routes stream/steps into the parent tool-card
    // via ToolProgress. RequestSelect* still passes through for permission popups.
    if let Some(tx) = ctx.ui_tx.clone() {
        let tagged = crate::tool::subagent_ui::tagged_ui_channel_with_progress(
            tx,
            ctx.progress_reporter.clone(),
        );
        subagent = subagent.with_ui_channel(tagged);
    }

    // Resume reuses a finished child session: hold its process lock for the
    // duration of the follow-up run so two runs can't operate on it at once.
    let lock = if input.resume.is_some() {
        Some(SessionLock::acquire(store, &child_id).await?)
    } else {
        None
    };

    // Record the run start for BOTH sync and async children so `check_subagent`
    // / `cancel_subagent` / `wait_subagent` see every child uniformly (a sync
    // child can also be cancelled via `/subagent_cancel` or parent exit). On
    // `resume` this resets the prior terminal record to Running for the
    // follow-up run.
    ctx.subagent_manager.start(child_id.clone()).await?;

    // True background dispatch needs a UI channel: without a driver there is
    // no one to submit a wake-up turn, and headless exits by cancelling the
    // child. Degrade to synchronous so the summary still reaches the parent.
    let run_async = input.run_in_background == Some(true) && ctx.ui_tx.is_some();
    if input.run_in_background == Some(true) && !run_async {
        warn!(
            "run_in_background requested without an interactive UI channel; \
             running subagent synchronously"
        );
    }

    if run_async {
        // Async: return immediately; the detached task finalizes the card,
        // persists the lifecycle, and re-injects the summary.
        let results = ctx
            .subagent_results
            .clone()
            .ok_or_else(|| anyhow::anyhow!("run_in_background requires a parent agent runtime"))?;
        let manager = ctx.subagent_manager.clone();
        let ui_tx = ctx.ui_tx.clone();
        let tool_id = ctx.progress_reporter.tool_id().to_string();
        let prompt = input.prompt;
        let worktree_for_task = worktree.clone();
        let launched = format!("async_launched {{ {child_id} }}");
        tokio::spawn(async move {
            let result = subagent
                .agent_loop(Some(Message::new_text(Role::User, prompt)))
                .await;
            let success = result.is_ok();
            let max_turns_reached = subagent.max_turns.is_some_and(|m| subagent.turns_taken > m);
            let summary = with_worktree_note(
                extract_summary(&subagent, max_turns_reached),
                worktree_for_task.as_ref(),
            );
            if let Some(lock) = lock {
                let _ = lock.release().await;
            }
            // A cancellation request flips the child flag; the agent loop
            // exits cooperatively at its next check point. Mark the run as
            // Cancelled instead of Completed so the record tells the truth.
            let cancelled = cancel_flag.load(Ordering::Relaxed);
            let succeeded = terminal_success(success, cancelled);
            let summary = if cancelled {
                format!("(cancelled by user)\n{summary}")
            } else {
                summary
            };
            let _ = if cancelled {
                manager.cancel(&child_id).await
            } else {
                manager.finish(&child_id, success, summary.clone()).await
            };
            // Keep the handle registered until the terminal state is durable;
            // shutdown can otherwise miss this child in the small window
            // between unregistering and persisting completion.
            manager.unregister_cancel_handle(&child_id);
            // Persist the lifecycle and enqueue the result BEFORE emitting the
            // TUI notification: the driver may immediately submit a wake-up
            // turn on receipt, and that turn's drain must already see the
            // queued result.
            if let Ok(mut queue) = results.lock() {
                queue.push_back(SubagentResult {
                    child_id: child_id.clone(),
                    summary: summary.clone(),
                    success: succeeded,
                });
            }
            // Emit on the PARENT ui_tx (not the child's tagged forwarder,
            // which drops unknown variants).
            if let Some(tx) = &ui_tx {
                let _ = tx.send(AgentUpdate::SubagentFinished {
                    tool_id: tool_id.clone(),
                    child_id: child_id.clone(),
                    success: succeeded,
                    summary: summary.clone(),
                });
            }
        });
        Ok(launched)
    } else {
        // Sync: block on the nested loop; tool result = last assistant text.
        let result = subagent
            .agent_loop(Some(Message::new_text(Role::User, input.prompt)))
            .await;
        let success = result.is_ok();
        let cancelled = cancel_flag.load(Ordering::Relaxed);
        let max_turns_reached = subagent.max_turns.is_some_and(|m| subagent.turns_taken > m);
        let summary = extract_summary(&subagent, max_turns_reached);
        ctx.subagent_manager.unregister_cancel_handle(&child_id);
        // Persist the terminal state so a cancelled / failed sync child is
        // visible to `check_subagent` (it previously left no `subagent_runs`
        // row at all).
        let _ = if cancelled {
            ctx.subagent_manager.cancel(&child_id).await
        } else {
            ctx.subagent_manager
                .finish(&child_id, success, summary.clone())
                .await
        };
        if let Some(lock) = lock {
            lock.release().await?;
        }
        // Propagate the agent-loop error after the lifecycle record is
        // durable, so a failed sync spawn does not leave a stale Running row.
        result?;
        let summary = if cancelled {
            format!("(cancelled by user)\n{summary}")
        } else {
            summary
        };
        Ok(with_worktree_note(summary, worktree.as_ref()))
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CheckSubagentInput {
    #[schemars(description = "Optional subagent child session id.")]
    pub child_id: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CancelSubagentInput {
    #[schemars(description = "Subagent child session id to cancel.")]
    pub child_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct WaitSubagentInput {
    #[schemars(description = "Subagent child session id to wait for.")]
    pub child_id: String,
    /// Maximum time to wait in milliseconds before returning a "still
    /// running" note (default 60000 = 1 minute).
    #[schemars(description = "Maximum wait in milliseconds (default 60000).")]
    pub timeout_ms: Option<u64>,
}

pub const CHECK_SUBAGENT_METADATA: ToolMetadata = ToolMetadata {
    name: "check_subagent",
    description: "Check subagent run status.",
    permission: PermissionPolicy::Read,
    permission_prompt: PermissionPromptPolicy::Json,
    resources: ResourcePolicy::Independent,
    domain: ToolDomain::Generic,
    presentation: ToolPresentation {
        visual_kind: ToolVisualKind::Generic,
        display_name: "🤖 Subagent Check",
        live_output: LiveOutputPolicy::Standard,
        detail: DetailPolicy::Result,
        popup: PopupPolicy::None,
        compact_result_to_meta: false,
    },
    output: OutputPolicy::KeepInline,
    argument_summary: ArgumentSummaryPolicy::Json,
};

pub const CANCEL_SUBAGENT_METADATA: ToolMetadata = ToolMetadata {
    name: "cancel_subagent",
    description: "Cancel a running background subagent by child session id. Flips the child's cooperative cancel flag so its agent loop exits at the next check point; the run record is marked Cancelled.",
    permission: PermissionPolicy::High,
    permission_prompt: PermissionPromptPolicy::Json,
    resources: ResourcePolicy::Independent,
    domain: ToolDomain::Generic,
    presentation: ToolPresentation {
        visual_kind: ToolVisualKind::Generic,
        display_name: "⏹ Subagent Cancel",
        live_output: LiveOutputPolicy::Standard,
        detail: DetailPolicy::Result,
        popup: PopupPolicy::None,
        compact_result_to_meta: false,
    },
    output: OutputPolicy::KeepInline,
    argument_summary: ArgumentSummaryPolicy::Json,
};

pub const WAIT_SUBAGENT_METADATA: ToolMetadata = ToolMetadata {
    name: "wait_subagent",
    description: "Block until a background subagent finishes (or times out), returning its summary. Polls the subagent run record so the parent can spawn several subagents in parallel, then wait on each instead of burning turns polling check_subagent.",
    permission: PermissionPolicy::Read,
    permission_prompt: PermissionPromptPolicy::Json,
    resources: ResourcePolicy::Independent,
    domain: ToolDomain::Generic,
    presentation: ToolPresentation {
        visual_kind: ToolVisualKind::Generic,
        display_name: "⏳ Subagent Wait",
        live_output: LiveOutputPolicy::Standard,
        detail: DetailPolicy::Result,
        popup: PopupPolicy::None,
        compact_result_to_meta: false,
    },
    output: OutputPolicy::KeepInline,
    argument_summary: ArgumentSummaryPolicy::Json,
};

#[tool]
/// # Errors
///
/// Returns an error if the provided child id does not exist or the subagent
/// manager encounters an internal error.
pub async fn check_subagent(ctx: ToolContext, input: CheckSubagentInput) -> Result<String> {
    ctx.subagent_manager.check(input.child_id.as_deref()).await
}

#[tool]
/// # Errors
///
/// Returns an error when the requested child id has no live cancel handle
/// (unknown or already finished).
pub async fn cancel_subagent(ctx: ToolContext, input: CancelSubagentInput) -> Result<String> {
    let cancelled = ctx.subagent_manager.request_cancel(input.child_id.as_str());
    if cancelled {
        // Best-effort: mark the run record Cancelled now; the child's finish
        // path keeps it Cancelled when the flag is set.
        let _ = ctx.subagent_manager.cancel(&input.child_id).await;
        Ok(format!("cancel requested for subagent {}", input.child_id))
    } else {
        bail!(
            "no running subagent {} to cancel (already finished or unknown); \
             use check_subagent to list runs",
            input.child_id
        )
    }
}

#[tool]
/// # Errors
///
/// Returns an error if the child id is unknown or the subagent manager
/// encounters an internal error. A timeout returns an `Ok` "still running"
/// note rather than an error.
pub async fn wait_subagent(ctx: ToolContext, input: WaitSubagentInput) -> Result<String> {
    let timeout_ms = input.timeout_ms.unwrap_or(60_000);
    ctx.subagent_manager.wait(&input.child_id, timeout_ms).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool::test_support::test_context;
    use tempfile::TempDir;

    #[test]
    fn main_toolset_registers_cancel_subagent() {
        let router = crate::tool::registry::toolset();
        let specs = router.tool_specs();
        assert!(
            specs.iter().any(|s| s.name == "cancel_subagent"),
            "cancel_subagent must be in the main toolset"
        );
        assert!(specs.iter().any(|s| s.name == "check_subagent"));
        assert!(
            specs.iter().any(|s| s.name == "wait_subagent"),
            "wait_subagent must be in the main toolset"
        );
    }

    #[test]
    fn cancelled_subagent_is_not_reported_as_successful() {
        // A cancelled child may still exit its loop cleanly (success=true),
        // but must never be reported successful to the parent / TUI.
        assert!(!terminal_success(true, true));
        assert!(terminal_success(true, false));
        assert!(!terminal_success(false, false));
        assert!(!terminal_success(false, true));
    }

    #[tokio::test]
    async fn resume_expiry_uses_finished_at() {
        let context = test_context("subagent-resume-expiry");
        // A terminal record with a fresh `finished_at` passes the expiry check
        // (the guard compares now - finished_at against RESUME_EXPIRY_HOURS).
        context
            .subagent_manager
            .start("child-done".to_string())
            .await
            .unwrap();
        context
            .subagent_manager
            .finish("child-done", true, "done".to_string())
            .await
            .unwrap();
        let record = context
            .subagent_manager
            .get("child-done")
            .await
            .unwrap()
            .unwrap();
        assert_ne!(record.status, SubagentStatus::Running);
        assert!(
            record.finished_at.is_some(),
            "terminal record must carry finished_at for the expiry check"
        );
        // A record older than the window fails the guard: finished_at must be
        // present and the age comparison must use it, not default to "ok".
        assert!(
            chrono::Utc::now() - record.finished_at.expect("finished_at present")
                < chrono::Duration::hours(RESUME_EXPIRY_HOURS),
            "fresh record must be within the resume window"
        );
    }

    #[test]
    fn subagent_toolset_has_five_tools() {
        let router = crate::tool::registry::subagent_toolset();
        let specs = router.tool_specs();
        assert_eq!(specs.len(), 5, "subagent should have exactly 5 tools");
        let names: Vec<&str> = specs.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"bash"));
        assert!(names.contains(&"read_file"));
        assert!(names.contains(&"write_file"));
        assert!(names.contains(&"edit_file"));
        assert!(names.contains(&"sleep"));
    }

    #[test]
    fn subagent_toolset_for_filters_by_claude_tool_names() {
        let router = crate::tool::registry::subagent_toolset_for(Some(&[
            "Read".to_string(),
            "Bash".to_string(),
        ]));
        let specs = router.tool_specs();
        let names: Vec<&str> = specs.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names.len(), 2);
        assert!(names.contains(&"bash"));
        assert!(names.contains(&"read_file"));

        // Edit / Write map to edit_file / write_file; Sleep is Tact-specific.
        let router = crate::tool::registry::subagent_toolset_for(Some(&[
            "Edit".to_string(),
            "Write".to_string(),
            "Sleep".to_string(),
        ]));
        let specs = router.tool_specs();
        let names: Vec<&str> = specs.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"edit_file"));
        assert!(names.contains(&"write_file"));
        assert!(names.contains(&"sleep"));
    }

    #[test]
    fn subagent_toolset_for_unknown_names_returns_empty_router() {
        // All-declared-tools-unknown must NOT silently widen to the default
        // set — the caller (spawn_subagent) errors out instead.
        let router = crate::tool::registry::subagent_toolset_for(Some(&["NotATool".to_string()]));
        assert!(
            router.tool_specs().is_empty(),
            "fully-unknown tools must yield an empty router, got {} tools",
            router.tool_specs().len()
        );

        let router = crate::tool::registry::subagent_toolset_for(None);
        assert_eq!(router.tool_specs().len(), 5);
    }

    #[test]
    fn subagent_toolset_for_mixed_known_unknown_keeps_known() {
        let router = crate::tool::registry::subagent_toolset_for(Some(&[
            "Read".to_string(),
            "TotallyBogus".to_string(),
            "Bash".to_string(),
        ]));
        let specs = router.tool_specs();
        assert_eq!(specs.len(), 2, "unknown names must be ignored, known kept");
    }

    #[test]
    fn subagent_toolset_for_empty_list_keeps_default() {
        // Claude semantics: an empty `tools:` list does not restrict — keep
        // the default five-tool set (not a zero-tool subagent).
        let router = crate::tool::registry::subagent_toolset_for(Some(&[]));
        assert_eq!(router.tool_specs().len(), 5);
    }

    #[test]
    fn resolve_agent_model_handles_claude_aliases() {
        assert_eq!(resolve_agent_model("inherit", "a"), None);
        assert_eq!(resolve_agent_model("", "a"), None);
        // Claude aliases are ignored (Tact has no mapping) — never passed through.
        assert_eq!(resolve_agent_model("sonnet", "a"), None);
        assert_eq!(resolve_agent_model("Opus", "a"), None);
        assert_eq!(resolve_agent_model("haiku", "a"), None);
        // Concrete model ids pass through unchanged.
        assert_eq!(
            resolve_agent_model("deepseek-v4", "a").as_deref(),
            Some("deepseek-v4")
        );
        assert_eq!(
            resolve_agent_model("claude-sonnet-4-5", "a").as_deref(),
            Some("claude-sonnet-4-5")
        );
    }

    #[test]
    fn subagent_input_deserialization() {
        let json = serde_json::json!({
            "prompt": "Fix the bug in main.rs",
            "description": "rust bugfix"
        });
        let input: SubagentInput = serde_json::from_value(json).unwrap();
        assert_eq!(input.prompt, "Fix the bug in main.rs");
        assert_eq!(input.description, Some("rust bugfix".to_string()));
        assert_eq!(input.run_in_background, None);
        assert_eq!(input.max_turns, None);
        assert_eq!(input.resume, None);
        assert_eq!(input.worktree, None);
        assert_eq!(input.agent, None);
    }

    #[test]
    fn subagent_input_deserializes_agent_field() {
        let json = serde_json::json!({
            "prompt": "Review the diff",
            "agent": "claude-security:code-reviewer",
            "worktree": true
        });
        let input: SubagentInput = serde_json::from_value(json).unwrap();
        assert_eq!(
            input.agent.as_deref(),
            Some("claude-security:code-reviewer")
        );
        assert_eq!(input.worktree, Some(true));
    }

    #[test]
    fn subagent_input_without_description() {
        let json = serde_json::json!({
            "prompt": "Just do it"
        });
        let input: SubagentInput = serde_json::from_value(json).unwrap();
        assert_eq!(input.prompt, "Just do it");
        assert_eq!(input.description, None);
    }

    #[test]
    fn subagent_input_async_fields() {
        let json = serde_json::json!({
            "prompt": "Background me",
            "run_in_background": true,
            "max_turns": 3,
            "resume": "child-abc"
        });
        let input: SubagentInput = serde_json::from_value(json).unwrap();
        assert_eq!(input.run_in_background, Some(true));
        assert_eq!(input.max_turns, Some(3));
        assert_eq!(input.resume.as_deref(), Some("child-abc"));
    }

    #[tokio::test]
    async fn ensure_child_session_row_links_parent_ref() {
        let tmp = TempDir::new().unwrap();
        let db = tmp.path().join("tact.db");
        let store = open_sqlite_session_store(&db).await.unwrap();
        store
            .ensure_session_row("parent", "/tmp/p", "")
            .await
            .unwrap();
        store
            .ensure_session_row("child", "/tmp/p", "parent")
            .await
            .unwrap();

        let listed = store.list_sessions(None).await.unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, "parent");

        store.delete_session("parent").await.unwrap();
        assert!(store.list_sessions(None).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn task_context_without_parent_opens_workdir_db() {
        let ctx = test_context("subagent-orphan-session");
        let db_path = TactPath::new(&ctx.work_dir).session_db_path();
        let store = open_sqlite_session_store(&db_path).await.unwrap();
        let child_id = uuid::Uuid::new_v4().to_string();
        store
            .ensure_session_row(&child_id, &ctx.work_dir.display().to_string(), "")
            .await
            .unwrap();
        let listed = store.list_sessions(None).await.unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, child_id);
    }

    #[test]
    fn subagent_input_worktree_field() {
        let json = serde_json::json!({ "prompt": "p", "worktree": true });
        let input: SubagentInput = serde_json::from_value(json).unwrap();
        assert_eq!(input.worktree, Some(true));

        let json = serde_json::json!({ "prompt": "p" });
        let input: SubagentInput = serde_json::from_value(json).unwrap();
        assert_eq!(input.worktree, None);
    }

    #[test]
    fn worktree_note_appended_to_summary() {
        let wt = WorktreeRecord {
            name: "subagent-child-1".into(),
            path: "/tmp/.worktrees/subagent-child-1".into(),
            branch: "wt/subagent-child-1".into(),
            task_id: None,
            status: "active".into(),
        };
        assert_eq!(
            with_worktree_note("done".to_string(), Some(&wt)),
            "done\n\n(worktree: subagent-child-1 at /tmp/.worktrees/subagent-child-1)"
        );
        assert_eq!(with_worktree_note("done".to_string(), None), "done");
    }

    /// Runs a git command in `dir`, asserting success.
    async fn git_run(dir: &std::path::Path, args: &[&str]) {
        let out = tokio::process::Command::new("git")
            .current_dir(dir)
            .args(args)
            .output()
            .await
            .expect("git command should run");
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// Initialises a git repo with one commit so `git worktree add` has a HEAD.
    async fn init_git_repo(dir: &std::path::Path) {
        git_run(dir, &["init", "-q"]).await;
        std::fs::write(dir.join("README.md"), "hello\n").unwrap();
        git_run(dir, &["add", "."]).await;
        git_run(
            dir,
            &[
                "-c",
                "user.email=test@example.com",
                "-c",
                "user.name=tact-test",
                "commit",
                "-m",
                "init",
            ],
        )
        .await;
    }

    #[tokio::test]
    async fn ensure_subagent_worktree_creates_and_reuses_lane() {
        let context = test_context("subagent-worktree-create");
        init_git_repo(&context.work_dir).await;

        let created = ensure_subagent_worktree(&context, "child-abc", false)
            .await
            .unwrap()
            .expect("worktree requested");
        assert_eq!(created.name, "subagent-child-abc");
        assert_eq!(created.branch, "wt/subagent-child-abc");
        let lane_dir = std::path::PathBuf::from(&created.path);
        assert!(lane_dir.is_dir(), "lane dir should exist");
        assert!(
            lane_dir.join("README.md").exists(),
            "lane should have the commit checked out"
        );

        // Resume reuses the existing lane instead of failing on the unique name.
        let reused = ensure_subagent_worktree(&context, "child-abc", true)
            .await
            .unwrap()
            .expect("worktree requested");
        assert_eq!(reused.path, created.path);

        // Non-resume on the same id errors (unique worktree name).
        let dup = ensure_subagent_worktree(&context, "child-abc", false).await;
        assert!(dup.is_err(), "duplicate lane should error");
    }

    #[tokio::test]
    async fn ensure_subagent_worktree_requires_git_repo() {
        let context = test_context("subagent-worktree-no-git");
        let err = ensure_subagent_worktree(&context, "child-xyz", false)
            .await
            .unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("git repository"), "unexpected error: {msg}");
    }
}
