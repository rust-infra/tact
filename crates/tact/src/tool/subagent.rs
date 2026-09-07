use crate::tool::{
    ArgumentSummaryPolicy, DetailPolicy, LiveOutputPolicy, OutputPolicy, PermissionPolicy,
    PermissionPromptPolicy, PopupPolicy, ResourcePolicy, ToolDomain, ToolMetadata,
    ToolPresentation,
};
use std::{
    path::{Path, PathBuf},
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
    /// Attach an isolated subagent skill card (`~/.tact/subagent/<name>.md`);
    /// its body is appended to the child's system prompt as the role.
    #[schemars(description = "Name of a subagent skill card to attach as the role.")]
    #[serde(default)]
    pub skill: Option<String>,
}

/// Worktree lane name prefix for isolated subagents.
const SUBAGENT_WORKTREE_PREFIX: &str = "subagent";

/// A finished subagent session may only be resumed within this window; older
/// sessions are rejected because their context is stale (Claude's 24h expiry).
const RESUME_EXPIRY_HOURS: i64 = 24;

/// Cap for a skill-card description rendered in the spawn tool description /
/// error listings — each line ships with every request, so keep it tight.
const SKILL_CARD_DESC_CAP: usize = 60;

/// Cap for the number of cards rendered into the `spawn_subagent` tool
/// description. The whole catalog ships with every main-loop request, so a
/// description length cap alone (60 chars/line) is not enough to bound the
/// per-request token cost once many cards exist.
const MAX_SKILL_CARD_CATALOG_LINES: usize = 30;

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
    description: "Spawn a subagent with its own fresh session that cannot see this conversation, so make the prompt self-contained. By default it runs in the current working directory, blocks until it finishes, and returns the subagent's final summary. Set run_in_background: true to return immediately as async_launched { id } and have the summary re-injected on a later turn; use it to fan out several independent subagents in parallel, then wait_subagent on each (only honored in interactive sessions, otherwise the call degrades to blocking). Set worktree: true to run in an isolated git worktree lane subagent-<id> (requires a git repo), safe for parallel edits; the lane path is appended to the returned summary. Set skill: <name> to attach an isolated subagent skill card from ~/.tact/subagent/<name>.md; its body is appended to the child system prompt as the role.",
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

/// Optional frontmatter for a subagent skill card. Only `description` is
/// consumed — for error listings and the discovery catalog appended to the
/// `spawn_subagent` tool description. The registry key is always the file
/// stem, so a `name:` frontmatter field is intentionally ignored.
#[derive(Debug, Default, Deserialize)]
struct SkillCardFrontmatter {
    description: Option<String>,
}

/// A resolved subagent skill card: role body plus an optional description.
#[derive(Debug)]
struct SkillCard {
    description: String,
    body: String,
}

/// Splits an optional `---`-fenced YAML header from the role body. Fails open:
/// a missing fence keeps the whole file as the body; a malformed frontmatter
/// block falls back to defaults but still yields the post-fence body.
fn parse_skill_card_frontmatter(text: &str) -> (SkillCardFrontmatter, String) {
    let text = text.replace("\r\n", "\n");
    let Some(rest) = text.strip_prefix("---\n") else {
        return (SkillCardFrontmatter::default(), text.trim().to_string());
    };
    let Some((frontmatter, body)) = rest.split_once("\n---\n") else {
        return (SkillCardFrontmatter::default(), text.trim().to_string());
    };
    let meta = serde_yaml::from_str::<SkillCardFrontmatter>(frontmatter).unwrap_or_default();
    (meta, body.trim().to_string())
}

/// Directory holding isolated subagent skill cards, user-global under
/// `~/.tact/subagent/` — deliberately separate from the main-agent skill roots.
fn subagent_skill_dir() -> Option<PathBuf> {
    TactPath::home_tact_dir().map(|dir| dir.join("subagent"))
}

/// Rejects names that could escape the skill-card directory. Cards live flat
/// under `~/.tact/subagent/`, so a valid key is a single file stem — no path
/// separators, no `.`/`..`, no NUL.
fn is_plain_stem(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && !name.contains('/')
        && !name.contains('\\')
        && !name.contains('\0')
}

/// Reads a single skill card keyed by file stem. Names that are not plain
/// stems (and could therefore escape `dir`) resolve to `None` — the registry
/// is flat by construction, mirroring the listing in [`list_skill_cards`].
fn read_skill_card(dir: &Path, name: &str) -> Option<SkillCard> {
    if !is_plain_stem(name) {
        return None;
    }
    let content = std::fs::read_to_string(dir.join(format!("{name}.md"))).ok()?;
    let (meta, body) = parse_skill_card_frontmatter(&content);
    Some(SkillCard {
        description: meta.description.unwrap_or_else(|| "No description".to_string()),
        body,
    })
}

/// Formats a catalog line as `- stem: <flattened, length-capped description>`.
fn format_skill_card_line(stem: &str, description: &str) -> String {
    let flat = description.split_whitespace().collect::<Vec<_>>().join(" ");
    let flat_len = flat.chars().count();
    let capped: String = flat.chars().take(SKILL_CARD_DESC_CAP).collect();
    let capped = if flat_len > SKILL_CARD_DESC_CAP {
        format!("{capped}…")
    } else {
        capped
    };
    format!("- {stem}: {capped}")
}

/// Lists available skill cards as sorted `- stem: description` lines
/// (error path and the spawn-description catalog).
fn list_skill_cards(dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut cards = entries
        .filter_map(|entry| entry.ok())
        // `fs::metadata(entry.path())` (unlike `DirEntry::metadata`, which does
        // not traverse links) follows symlinks, so a symlinked card that
        // `read_skill_card` can read is also listed here — the catalog and the
        // resolver never disagree.
        .filter(|entry| entry.path().metadata().map(|m| m.is_file()).unwrap_or(false))
        .filter_map(|entry| {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("md") {
                return None;
            }
            let stem = path.file_stem()?.to_str()?.to_string();
            let card = read_skill_card(dir, &stem)?;
            Some(format_skill_card_line(&stem, &card.description))
        })
        .collect::<Vec<_>>();
    cards.sort();
    cards
}

/// Resolves `name` to a skill card and appends its body to `system_prompt`.
/// Fails closed on an unknown name (lists available cards) and on any name
/// that is not a plain file stem (would escape the card directory).
fn apply_skill_card(system_prompt: &mut String, dir: &Path, name: &str) -> Result<()> {
    let card = read_skill_card(dir, name).ok_or_else(|| {
        if !is_plain_stem(name) {
            anyhow::anyhow!(
                "invalid subagent skill name '{name}': names are plain file stems under the \
                 skill-card directory"
            )
        } else {
            let available = list_skill_cards(dir).join("\n");
            let available = if available.is_empty() {
                "(none found)".to_string()
            } else {
                available
            };
            anyhow::anyhow!("unknown subagent skill '{name}'; available:\n{available}")
        }
    })?;
    system_prompt.push_str(&format!(
        "\n\n<skill name=\"{name}\">\n{}\n</skill>",
        card.body
    ));
    Ok(())
}

/// Appends the available subagent skill cards (`~/.tact/subagent/`) to the
/// `spawn_subagent` tool description so the main agent can discover valid
/// `skill:` names on every request. No-op when there is no card home.
pub fn annotate_spawn_subagent_skill_catalog(tools: &mut crate::tool::ToolRouter) {
    if let Some(dir) = subagent_skill_dir() {
        annotate_spawn_subagent_skill_catalog_with_dir(tools, &dir);
    }
}

/// Directory-scoped core of [`annotate_spawn_subagent_skill_catalog`]; no-op
/// when the directory holds no cards.
fn annotate_spawn_subagent_skill_catalog_with_dir(
    tools: &mut crate::tool::ToolRouter,
    dir: &Path,
) {
    let cards = list_skill_cards(dir);
    if cards.is_empty() {
        return;
    }
    // The catalog ships with every main-loop request, so bound how many card
    // lines are rendered (descriptions are already length-capped per line).
    let total = cards.len();
    let rendered = cards
        .iter()
        .take(MAX_SKILL_CARD_CATALOG_LINES)
        .cloned()
        .collect::<Vec<_>>();
    let mut suffix = format!("\nAvailable subagent skill cards:\n{}", rendered.join("\n"));
    if total > MAX_SKILL_CARD_CATALOG_LINES {
        suffix.push_str(&format!("\n… and {} more", total - MAX_SKILL_CARD_CATALOG_LINES));
    }
    tools.set_tool_description(
        "spawn_subagent",
        format!("{}{}", SPAWN_SUBAGENT_METADATA.description, suffix),
    );
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
    // worktree-isolated subagent is told exactly where it is working; the
    // caller's `prompt` becomes the user task.
    let mut system_prompt = format!(
        "You are a coding subagent at {}. Complete the given task, then summarize your findings.",
        ctx.work_dir.display()
    );

    // Optional subagent skill card: attach an isolated role body before hooks
    // run, so hooks can still append/override afterward.
    if let Some(skill) = input.skill.as_deref() {
        let dir = subagent_skill_dir().ok_or_else(|| {
            anyhow::anyhow!("subagent skill '{skill}' requested but $HOME is unavailable")
        })?;
        apply_skill_card(&mut system_prompt, &dir, skill)?;
    }

    // Claude Code plugin `SubagentStart` hooks may inject context into the
    // child system prompt (e.g. ponytail injects its mode). A `Block` from a
    // Rust-registered hook fails the spawn; plugin command hooks already
    // normalize failures to Continue.
    if !ctx.subagent_start_hooks.is_empty() {
        let mut start_ctx = crate::hook::SubagentStartContext {
            name: child_id.clone(),
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

    let mut subagent = Agent::new(
        client,
        ctx.clone(),
        crate::tool::registry::subagent_toolset(),
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
    // The sticky overview only shows children started by this process;
    // register now and broadcast the snapshot (no-op without ui_tx).
    ctx.subagent_manager.note_started(&child_id);
    crate::subagent::emit_subagents_changed(&ctx.ui_tx, &ctx.subagent_manager).await;

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
            // Broadcast the sticky snapshot after the row is durable, before
            // the parent tool card is finalized.
            crate::subagent::emit_subagents_changed(&ui_tx, &manager).await;
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
        crate::subagent::emit_subagents_changed(&ctx.ui_tx, &ctx.subagent_manager).await;
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
        crate::subagent::emit_subagents_changed(&ctx.ui_tx, &ctx.subagent_manager).await;
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

    fn write_skill_card(dir: &Path, stem: &str, content: &str) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join(format!("{stem}.md")), content).unwrap();
    }

    #[test]
    fn apply_skill_card_appends_body_after_base_prompt() {
        let dir = tempfile::tempdir().unwrap();
        write_skill_card(
            dir.path(),
            "reviewer",
            "---\ndescription: Adversarial review\n---\n\nYou are a principal reviewer.",
        );
        let mut prompt = "You are a coding subagent at /repo. Complete the given task, then summarize your findings.".to_string();
        apply_skill_card(&mut prompt, dir.path(), "reviewer").unwrap();
        assert!(
            prompt.starts_with("You are a coding subagent at /repo"),
            "base template must stay first: {prompt}"
        );
        assert!(prompt.contains("<skill name=\"reviewer\">"), "{prompt}");
        assert!(prompt.contains("You are a principal reviewer."), "{prompt}");
        assert!(prompt.ends_with("</skill>"), "{prompt}");
    }

    #[test]
    fn apply_skill_card_unknown_name_lists_available() {
        let dir = tempfile::tempdir().unwrap();
        write_skill_card(dir.path(), "a", "body a");
        write_skill_card(dir.path(), "b", "---\ndescription: B role\n---\nbody b");
        let err = apply_skill_card(&mut String::new(), dir.path(), "missing").unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("unknown subagent skill 'missing'"), "{msg}");
        assert!(msg.contains("- a:"), "{msg}");
        assert!(msg.contains("- b: B role"), "{msg}");
    }

    #[test]
    fn skill_card_key_is_stem_and_bad_frontmatter_fails_open() {
        let dir = tempfile::tempdir().unwrap();
        // Malformed YAML but a valid fence: body is the post-fence text.
        write_skill_card(dir.path(), "fixer", "---\nnot a mapping\n---\nrole body");
        let card = read_skill_card(dir.path(), "fixer").unwrap();
        assert_eq!(card.body, "role body");
        assert_eq!(card.description, "No description");
        // No fence: whole file is the body.
        write_skill_card(dir.path(), "plain", "just a role body");
        let card = read_skill_card(dir.path(), "plain").unwrap();
        assert_eq!(card.body, "just a role body");
    }

    #[test]
    fn skill_card_description_defaults_when_absent() {
        let dir = tempfile::tempdir().unwrap();
        write_skill_card(dir.path(), "architect", "---\ndescription: Arch review\n---\nbody");
        let card = read_skill_card(dir.path(), "architect").unwrap();
        assert_eq!(card.description, "Arch review");
    }

    #[test]
    fn skill_cards_are_isolated_from_main_skill_registry() {
        let subagent_dir = tempfile::tempdir().unwrap();
        write_skill_card(subagent_dir.path(), "reviewer", "role body");
        let skills_dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(skills_dir.path().join("other")).unwrap();
        std::fs::write(
            skills_dir.path().join("other/SKILL.md"),
            "---\ndescription: o\n---\nskill body",
        )
        .unwrap();
        let mut reg = crate::skill::SkillRegistry::new([skills_dir.path().to_path_buf()]);
        reg.load_skills().unwrap();
        assert!(reg.skills().contains_key("other"));
        assert!(
            !reg.skills().contains_key("reviewer"),
            "subagent skill cards must not enter the main skill registry"
        );
    }

    #[test]
    fn catalog_lines_are_flattened_and_capped() {
        let description =
            "line one\nline two with a long tail that keeps going well beyond the cap for display";
        let line = format_skill_card_line("long", description);
        assert!(!line.contains('\n'), "catalog lines must be single-line: {line}");
        assert!(line.starts_with("- long: line one line two with a long tail"));
        let body = line.trim_start_matches("- long: ");
        assert!(
            body.chars().count() <= SKILL_CARD_DESC_CAP + 1,
            "capped body too long: {body}"
        );
        assert!(body.ends_with('…'), "truncated lines end with ellipsis: {body}");
    }

    #[test]
    fn annotate_spawn_description_appends_available_cards() {
        let dir = tempfile::tempdir().unwrap();
        write_skill_card(
            dir.path(),
            "reviewer",
            "---\ndescription: Adversarial code review for subagents\n---\nrole",
        );
        write_skill_card(dir.path(), "fixer", "fixer role");
        let mut tools = crate::tool::registry::toolset();
        annotate_spawn_subagent_skill_catalog_with_dir(&mut tools, dir.path());
        let specs = tools.tool_specs();
        let spawn = specs
            .iter()
            .find(|spec| spec.name == "spawn_subagent")
            .expect("spawn_subagent must be registered");
        let description = spawn.description.as_deref().expect("spawn has a description");
        assert!(description.contains("Available subagent skill cards:"), "{description}");
        assert!(
            description.contains("- reviewer: Adversarial code review for subagents"),
            "{description}"
        );
        assert!(description.contains("- fixer:"), "{description}");
    }

    #[test]
    fn annotate_spawn_description_skips_when_no_cards() {
        let dir = tempfile::tempdir().unwrap();
        let mut tools = crate::tool::registry::toolset();
        annotate_spawn_subagent_skill_catalog_with_dir(&mut tools, dir.path());
        let specs = tools.tool_specs();
        let spawn = specs
            .iter()
            .find(|spec| spec.name == "spawn_subagent")
            .expect("spawn_subagent must be registered");
        let description = spawn.description.as_deref().expect("spawn has a description");
        assert!(
            !description.contains("Available subagent skill cards:"),
            "an empty catalog must not rewrite the description: {description}"
        );
    }

    #[test]
    fn skill_card_name_rejects_directory_escape() {
        let dir = tempfile::tempdir().unwrap();
        // `cards` is the skill-card home; the secret sits next to it so a
        // `../secret` traversal would reach it if the name were not validated.
        let cards = dir.path().join("cards");
        write_skill_card(&cards, "reviewer", "role body");
        let secret = dir.path().join("secret.txt");
        std::fs::write(&secret, "TOP SECRET").unwrap();

        let mut prompt = String::new();
        let err = apply_skill_card(&mut prompt, &cards, "../secret").unwrap_err();
        let msg = format!("{err:#}");
        assert!(
            !msg.contains("TOP SECRET"),
            "traversal must not read outside the card directory: {msg}"
        );
        assert!(msg.contains("invalid subagent skill name"), "{msg}");
        assert!(prompt.is_empty(), "no role body may be appended on escape: {prompt}");

        // Separator- and NUL-containing names are rejected too.
        for bad in ["a/b", "..", "a\\b", "a\0b"] {
            let err = apply_skill_card(&mut prompt, &cards, bad).unwrap_err();
            let msg = format!("{err:#}");
            assert!(msg.contains("invalid subagent skill name"), "{bad}: {msg}");
            assert!(prompt.is_empty(), "{bad} must not append a role body: {prompt}");
        }

        // A legit stem still resolves.
        apply_skill_card(&mut prompt, &cards, "reviewer").unwrap();
        assert!(prompt.contains("<skill name=\"reviewer\">"), "{prompt}");
    }

    #[cfg(unix)]
    #[test]
    fn catalog_lists_symlinked_cards_like_read_resolves_them() {
        use std::os::unix::fs::symlink;
        let dir = tempfile::tempdir().unwrap();
        write_skill_card(dir.path(), "reviewer", "role body");
        // A card whose file is a symlink: `read_skill_card` follows links, so
        // the catalog must list it too (otherwise the resolver and the catalog
        // disagree about which names are valid). The target lives in a
        // subdirectory so it is not itself a top-level card.
        let target_dir = dir.path().join("targets");
        std::fs::create_dir_all(&target_dir).unwrap();
        let external = target_dir.join("external-card.md");
        std::fs::write(
            &external,
            "---\ndescription: Linked review role\n---\nlinked body",
        )
        .unwrap();
        symlink(&external, dir.path().join("reviewer-link.md")).unwrap();

        let lines = list_skill_cards(dir.path());
        assert!(
            lines.iter().any(|l| l.starts_with("- reviewer-link: Linked review role")),
            "symlinked card must appear in the catalog: {lines:?}"
        );
        let card = read_skill_card(dir.path(), "reviewer-link").unwrap();
        assert_eq!(card.body, "linked body");
    }

    #[test]
    fn annotate_spawn_description_caps_catalog_size() {
        let dir = tempfile::tempdir().unwrap();
        // Exceed the render cap so the annotation must truncate.
        for i in 0..(MAX_SKILL_CARD_CATALOG_LINES + 2) {
            write_skill_card(dir.path(), &format!("card{i:02}"), &format!("card {i} role"));
        }
        let mut tools = crate::tool::registry::toolset();
        annotate_spawn_subagent_skill_catalog_with_dir(&mut tools, dir.path());
        let specs = tools.tool_specs();
        let spawn = specs
            .iter()
            .find(|spec| spec.name == "spawn_subagent")
            .expect("spawn_subagent must be registered");
        let description = spawn.description.as_deref().expect("spawn has a description");
        assert!(
            description.contains("Available subagent skill cards:"),
            "{description}"
        );
        let body = description
            .split("Available subagent skill cards:\n")
            .nth(1)
            .unwrap_or("");
        let rendered_lines = body.lines().filter(|l| l.starts_with("- ")).count();
        assert!(
            rendered_lines <= MAX_SKILL_CARD_CATALOG_LINES,
            "catalog must be capped at {MAX_SKILL_CARD_CATALOG_LINES} lines: {description}"
        );
        assert!(
            description.contains("… and 2 more"),
            "catalog must advertise the omitted count: {description}"
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
