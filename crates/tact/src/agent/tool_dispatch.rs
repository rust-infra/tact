//! Tool-call dispatch: pre-flight, parallel execution, and result assembly.
//!
//! After Task 5, all semantic decisions flow through typed metadata instead of
//! matching native tool-name strings.

use anyhow::Result;
use futures_util::{StreamExt, stream::FuturesUnordered};
use tact_llm::ContentBlock;
use tact_protocol::{AgentUpdate, StepResult, StepStatus, ToolPresentationInfo};

use super::Agent;
use crate::{
    compact::persist_large_output,
    hook::{HookControl, NotificationContext, ToolResult, ToolUse},
    invoke_hooks,
    mcp::MCPToolRouter,
    permission::{
        CapabilityRisk, PermissionBehavior, format_permission_prompt, normalize_mcp_capability,
    },
    tool::{
        ArgumentSummaryPolicy, DetailPolicy, OutputPolicy, TaskOperation, ToolDomain, ToolRouter,
    },
    utils::RwLockExt,
};

/// A resolved tool — either native (with owned metadata copy) or MCP.
enum ResolvedTool {
    Native {
        metadata: &'static crate::tool::ToolMetadata,
    },
    Mcp {
        full_name: String,
        server: String,
        tool: String,
    },
    Unknown {
        name: String,
    },
}

/// A tool call after phase-1 pre-flight.
struct PreparedTool {
    id: String,
    name: String,
    input: serde_json::Value,
    step_idx: usize,
    permission_label: Option<String>,
    state: PreparedState,
    resolved: ResolvedTool,
    task_before: Option<crate::task::TaskRecord>,
}

enum PreparedState {
    Run,
    Resolved(String),
}

const TOOL_CANCELLED_MSG: &str = "Cancelled by user";
const MAX_TOOL_ARG_SUMMARY_CHARS: usize = 120;

fn build_tool_results(
    prepared: Vec<PreparedTool>,
    outputs: Vec<Option<ExecResult>>,
) -> Vec<ContentBlock> {
    let mut blocks = Vec::with_capacity(prepared.len());
    for (idx, prep) in prepared.into_iter().enumerate() {
        let content = match &prep.state {
            PreparedState::Resolved(msg) => msg.clone(),
            PreparedState::Run => outputs[idx]
                .as_ref()
                .map(|r| r.content.clone())
                .unwrap_or_else(|| TOOL_CANCELLED_MSG.to_string()),
        };
        let image = match &prep.state {
            PreparedState::Resolved(_) => None,
            PreparedState::Run => outputs[idx].as_ref().and_then(|r| r.image.clone()),
        };
        blocks.push(ContentBlock::ToolResult {
            tool_use_id: prep.id,
            content,
        });
        if let Some(image) = image {
            // Harness-style: the image cannot ride `role:tool` (string-only), so
            // it is emitted as a companion block; `convert.rs` folds it into a
            // following `user` message.
            blocks.push(ContentBlock::Image { source: image });
        }
    }
    blocks
}

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

/// The `arg_full` cap for Task-domain titles. Larger than
/// [`MAX_TOOL_ARG_SUMMARY_CHARS`] because a task title is already prose, not a
/// raw JSON argument dump.
const TASK_SUMMARY_CHARS: usize = 240;

/// Reads a string field, mapping absent/non-string to `""`.
fn str_field<'a>(input: &'a serde_json::Value, key: &str) -> &'a str {
    input.get(key).and_then(|v| v.as_str()).unwrap_or("")
}

/// `(arg_full, arg_summary)` for a static [`ArgumentSummaryPolicy`].
fn arg_pair_for(policy: ArgumentSummaryPolicy, input: &serde_json::Value) -> (String, String) {
    let full = tool_arg_full(policy, input);
    let summary = truncate_tool_arg_summary(&full);
    (full, summary)
}

/// `(arg_full, arg_summary)` for a Task-domain tool.
///
/// The title depends on the task record *before* and *after* the call
/// ([`crate::task::format_task_tool_title`] prefers `after`), which no static
/// [`ArgumentSummaryPolicy`] can express.
fn task_arg_pair(
    op: TaskOperation,
    input: &serde_json::Value,
    before: Option<&crate::task::TaskRecord>,
    after: Option<&crate::task::TaskRecord>,
) -> (String, String) {
    let full = crate::task::format_task_tool_title(op, input, before, after);
    let summary = if full.chars().count() <= TASK_SUMMARY_CHARS {
        full.clone()
    } else {
        format!(
            "{}...",
            full.chars()
                .take(TASK_SUMMARY_CHARS.saturating_sub(3))
                .collect::<String>()
        )
    };
    (full, summary)
}

/// `(arg_full, arg_summary)` for a resolved tool, dispatching to
/// [`task_arg_pair`] for Task-domain tools and [`arg_pair_for`] otherwise.
fn arg_pair(
    resolved: &ResolvedTool,
    input: &serde_json::Value,
    before: Option<&crate::task::TaskRecord>,
    after: Option<&crate::task::TaskRecord>,
) -> (String, String) {
    if let ResolvedTool::Native { metadata } = resolved
        && let ToolDomain::Task(op) = metadata.domain
    {
        return task_arg_pair(op, input, before, after);
    }
    let policy = match resolved {
        ResolvedTool::Native { metadata } => metadata.argument_summary,
        _ => ArgumentSummaryPolicy::Json,
    };
    arg_pair_for(policy, input)
}

/// The task a Task-domain call reads back through `task_id` (used for both the
/// pre-call snapshot and the post-call one).
async fn task_by_id(
    manager: &crate::task::SharedTaskManager,
    input: &serde_json::Value,
) -> Option<crate::task::TaskRecord> {
    let id = input.get("task_id").and_then(|v| v.as_u64())?;
    manager.get(id).await.ok()
}

/// The record a `TaskOperation::Create` call just created, identified by
/// subject because the store assigns the id.
async fn task_created(
    manager: &crate::task::SharedTaskManager,
    input: &serde_json::Value,
) -> Option<crate::task::TaskRecord> {
    let subject = str_field(input, "subject");
    manager.list().await.ok().and_then(|list| {
        list.into_iter()
            .filter(|t| t.subject == subject)
            .max_by_key(|t| t.id)
    })
}

// ── Argument formatting from metadata ────────────────────────────────────

fn tool_arg_full(policy: ArgumentSummaryPolicy, input: &serde_json::Value) -> String {
    fn patch_title(input: &serde_json::Value) -> String {
        let patch = str_field(input, "patch");
        let dry = input
            .get("dry_run")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let label = if dry { "dry-run" } else { "patch" };
        let first_line = patch.lines().next().unwrap_or("").trim().to_string();
        if first_line.is_empty() {
            label.to_string()
        } else if first_line.len() <= 78 {
            format!("{label}: {first_line}")
        } else {
            format!("{label}: {}...", &first_line[..75])
        }
    }

    match policy {
        ArgumentSummaryPolicy::Json => input.to_string(),
        ArgumentSummaryPolicy::Path { field } => str_field(input, field).to_string(),
        ArgumentSummaryPolicy::Command { field } => str_field(input, field).to_string(),
        ArgumentSummaryPolicy::Question { field } => str_field(input, field).to_string(),
        ArgumentSummaryPolicy::SubagentPrompt { field } => str_field(input, field).to_string(),
        ArgumentSummaryPolicy::PatchPreview { .. } => patch_title(input),
        ArgumentSummaryPolicy::ReadOffsetLimit { path_field } => {
            str_field(input, path_field).to_string()
        }
    }
}

fn tool_detail_content(
    detail: DetailPolicy,
    input: &serde_json::Value,
    exec_output: &str,
) -> Option<String> {
    match detail {
        DetailPolicy::None => None,
        DetailPolicy::Result => Some(exec_output.to_string()),
        DetailPolicy::InputField(field) => str_field(input, field).to_string().into(),
    }
}

fn step_result_detail(
    detail: DetailPolicy,
    input: &serde_json::Value,
    exec_output: &str,
    status: &StepStatus,
) -> Option<String> {
    if matches!(status, StepStatus::Failed) {
        Some(exec_output.to_string())
    } else {
        tool_detail_content(detail, input, exec_output)
    }
}

// ── Native / MCP runners ─────────────────────────────────────────────────

struct ExecResult {
    content: String,
    status: StepStatus,
    image: Option<tact_llm::ImageSource>,
}

async fn run_native_tool(
    tools: &ToolRouter,
    ctx: &crate::tool::ToolContext,
    tool_use_id: &str,
    name: &str,
    input: &serde_json::Value,
    output_policy: OutputPolicy,
) -> ExecResult {
    let call_ctx = ctx.for_invocation(tool_use_id);
    match tools.call_result(&call_ctx, name, input.clone()).await {
        Ok(result) => {
            let tact_path = crate::consts::TactPath::new(&ctx.work_dir);
            match output_policy {
                OutputPolicy::PersistLargeOutput => {
                    match persist_large_output(&tact_path, tool_use_id, &result.content).await {
                        Ok(content) => ExecResult {
                            content,
                            status: StepStatus::Success,
                            image: result.image,
                        },
                        Err(error) => ExecResult {
                            content: format!("Error persisting large output: {error}"),
                            status: StepStatus::Failed,
                            image: None,
                        },
                    }
                }
                OutputPolicy::KeepInline => ExecResult {
                    content: result.content,
                    status: StepStatus::Success,
                    image: result.image,
                },
            }
        }
        Err(e) => ExecResult {
            content: format!("Error invoking tool {}: {}", name, e),
            status: StepStatus::Failed,
            image: None,
        },
    }
}

async fn run_mcp_tool(
    mcp_router: &MCPToolRouter,
    ctx: &crate::tool::ToolContext,
    tool_use_id: &str,
    name: &str,
    input: &serde_json::Value,
) -> ExecResult {
    match mcp_router.call(name, input.clone()).await {
        Ok(output) => {
            let tact_path = crate::consts::TactPath::new(&ctx.work_dir);
            match persist_large_output(&tact_path, tool_use_id, &output).await {
                Ok(content) => ExecResult {
                    content,
                    status: StepStatus::Success,
                    image: None,
                },
                Err(error) => ExecResult {
                    content: format!("Error persisting large MCP output: {error}"),
                    status: StepStatus::Failed,
                    image: None,
                },
            }
        }
        Err(e) => ExecResult {
            content: format!("Error invoking MCP tool {}: {}", name, e),
            status: StepStatus::Failed,
            image: None,
        },
    }
}

// ── Presentation helper ─────────────────────────────────────────────────

fn make_presentation(meta: &crate::tool::ToolMetadata) -> ToolPresentationInfo {
    ToolPresentationInfo {
        visual_kind: meta.presentation.visual_kind,
        display_name: meta.presentation.display_name.to_string(),
        keep_full_live_output: matches!(
            meta.presentation.live_output,
            crate::tool::LiveOutputPolicy::FullTranscript
        ),
        keep_live: matches!(
            meta.presentation.live_output,
            crate::tool::LiveOutputPolicy::Background
        ),
        detail: match meta.presentation.detail {
            DetailPolicy::None => tact_protocol::ToolDetailKind::None,
            DetailPolicy::Result => tact_protocol::ToolDetailKind::Result,
            DetailPolicy::InputField(field) => {
                tact_protocol::ToolDetailKind::InputField(field.to_string())
            }
        },
        popup: match meta.presentation.popup {
            crate::tool::PopupPolicy::None => tact_protocol::ToolPopupKind::None,
            crate::tool::PopupPolicy::SubagentTranscript => {
                tact_protocol::ToolPopupKind::SubagentTranscript
            }
        },
        compact_result_to_meta: meta.presentation.compact_result_to_meta,
    }
}

/// Per-invocation resource resolution for a cleared tool call.
///
/// Most tools use their static metadata [`ResourcePolicy`]. The one
/// input-aware exception: a `spawn_subagent` call with `worktree: true` runs
/// in its own git worktree lane, so its file effects are scoped and it may
/// fan out in the same wave as other tools — mapped to
/// [`ToolResources::independent`] instead of the static `Barrier`. This is
/// the "worktree follow-up" named in the 2026-08-26 async-subagent design
/// review: same-wave fan-out of blocking subagents becomes safe once each
/// subagent has a scoped filesystem.
fn tool_resources_for(
    prep: &PreparedTool,
    work_dir: &std::path::Path,
) -> super::tool_schedule::ToolResources {
    match &prep.resolved {
        ResolvedTool::Native { metadata } => {
            if metadata.name == crate::tool::SPAWN_SUBAGENT_METADATA.name
                && prep.input.get("worktree").and_then(|v| v.as_bool()) == Some(true)
            {
                return super::tool_schedule::ToolResources::independent();
            }
            super::tool_schedule::tool_resources_from_metadata(
                &metadata.resources,
                &prep.input,
                work_dir,
            )
        }
        ResolvedTool::Mcp { server, .. } => super::tool_schedule::mcp_server_resources(server),
        ResolvedTool::Unknown { .. } => super::tool_schedule::ToolResources::barrier(),
    }
}

/// Like [`make_presentation`], but input-aware: a `spawn_subagent` call with
/// `run_in_background: true` keeps its card live (the invocation returns
/// `async_launched { id }` and the card is finalized later by
/// [`AgentUpdate::SubagentFinished`]). Static metadata can't express this
/// because `keep_live` is a per-invocation property, not a per-tool one.
fn make_presentation_for(
    meta: &crate::tool::ToolMetadata,
    input: &serde_json::Value,
) -> ToolPresentationInfo {
    let mut presentation = make_presentation(meta);
    if meta.name == "spawn_subagent"
        && input.get("run_in_background").and_then(|v| v.as_bool()) == Some(true)
    {
        presentation.keep_live = true;
    }
    presentation
}

// ── Main dispatch ────────────────────────────────────────────────────────

/// Phase-1 result: every tool use in the assistant turn, resolved to a native
/// or MCP tool and run through the permission pipeline.
struct Preflight {
    prepared: Vec<PreparedTool>,
    /// The cancel flag fired while walking the tool uses; the rest are stubbed
    /// out as cancelled, so nothing may be executed.
    cancelled: bool,
}

impl Agent {
    /// Executes every `ToolUse` block in `content`.
    ///
    /// Returns the assembled `ToolResult` blocks (in call order) plus a pending
    /// manual-compaction focus when a `compact` call succeeded.
    ///
    /// Three phases: [`Agent::preflight_tool_calls`] resolves and authorizes
    /// each call sequentially, [`Agent::run_tool_waves`] executes the runnable
    /// ones in conflict-free waves, and [`build_tool_results`] reassembles the
    /// outputs back into call order.
    pub async fn execute_tool_call(
        &mut self,
        content: &[ContentBlock],
    ) -> Result<(Vec<ContentBlock>, Option<String>)> {
        let preflight = self.preflight_tool_calls(content).await?;
        if preflight.cancelled {
            return Ok((build_tool_results(preflight.prepared, vec![]), None));
        }
        let (outputs, manual_compact) = self.run_tool_waves(&preflight.prepared).await?;
        Ok((
            build_tool_results(preflight.prepared, outputs),
            manual_compact,
        ))
    }

    /// Phase 1 — sequential pre-flight.
    ///
    /// Each tool use is counted, resolved (native → MCP → unknown), formatted
    /// for display, and run through the `PreToolUse` hook + permission
    /// pipeline. Sequential by design: a permission prompt must be answered
    /// before the next call is resolved, and an "always allow" granted here has
    /// to apply to the calls that follow in the same turn.
    async fn preflight_tool_calls(&mut self, content: &[ContentBlock]) -> Result<Preflight> {
        let mut prepared: Vec<PreparedTool> = Vec::new();
        for block in content {
            let ContentBlock::ToolUse { id, name, input } = block else {
                continue;
            };
            *self
                .runtime
                .stats
                .write_recover()
                .tool_counts
                .entry(name.clone())
                .or_insert(0) += 1;
            if self.cancel_requested() {
                self.emit_update(AgentUpdate::Info("Cancelled by user".into()));
                self.append_cancelled_tool_uses(&mut prepared, content);
                return Ok(Preflight {
                    prepared,
                    cancelled: true,
                });
            }

            let step_idx = self.next_step_idx();

            // Resolve: native first, then MCP, else Unknown
            let resolved = match self.tools.resolve(name) {
                Ok(r) => ResolvedTool::Native {
                    metadata: r.metadata(),
                },
                Err(_) => match self.mcp_router.resolve_tool(name) {
                    Ok(Some(mcp)) => ResolvedTool::Mcp {
                        full_name: mcp.full_name,
                        server: mcp.server,
                        tool: mcp.tool,
                    },
                    Ok(None) | Err(_) => {
                        let msg = format!("unknown tool: {name}");
                        self.emit_update(AgentUpdate::StepAdded(tact_protocol::PlanStep::new(
                            name.clone(),
                            name.clone(),
                            id.clone(),
                            input.as_object().cloned().unwrap_or_default(),
                        )));
                        self.emit_update(AgentUpdate::StepStarted {
                            idx: step_idx,
                            tool_id: id.clone(),
                            tool_name: name.clone(),
                            arg_summary: String::new(),
                            arg_full: String::new(),
                            presentation: ToolPresentationInfo::generic(name.clone()),
                        });
                        self.emit_update(AgentUpdate::StepFailed {
                            idx: step_idx,
                            tool_id: id.clone(),
                            arg_summary: String::new(),
                            error: msg.clone(),
                        });
                        prepared.push(PreparedTool {
                            id: id.clone(),
                            name: name.clone(),
                            input: input.clone(),
                            step_idx,
                            permission_label: None,
                            state: PreparedState::Resolved(msg),
                            resolved: ResolvedTool::Unknown { name: name.clone() },
                            task_before: None,
                        });
                        continue;
                    }
                },
            };

            // Argument formatting. No task record yet — pre-flight only knows
            // the call's input.
            let (arg_full, arg_summary) = arg_pair(&resolved, input, None, None);

            let step_description = if arg_summary.is_empty() {
                name.clone()
            } else {
                format!("{name} ({arg_summary})")
            };
            let presentation = match &resolved {
                ResolvedTool::Native { metadata } => make_presentation_for(metadata, input),
                _ => ToolPresentationInfo::generic(name.clone()),
            };

            self.emit_update(AgentUpdate::StepAdded(tact_protocol::PlanStep::new(
                step_description,
                name.clone(),
                id.clone(),
                input.as_object().cloned().unwrap_or_default(),
            )));
            self.emit_update(AgentUpdate::StepStarted {
                idx: step_idx,
                tool_id: id.clone(),
                tool_name: name.clone(),
                arg_summary: arg_summary.clone(),
                arg_full: arg_full.clone(),
                presentation: presentation.clone(),
            });

            let mut tool_use = ToolUse {
                id: id.clone(),
                name: name.clone(),
                input: input.clone(),
            };
            let mut permission_label: Option<String> = None;
            let stable_name = match &resolved {
                ResolvedTool::Native { metadata } => metadata.name,
                ResolvedTool::Mcp { full_name, .. } => full_name.as_str(),
                ResolvedTool::Unknown { name } => name.as_str(),
            };
            let risk = match &resolved {
                ResolvedTool::Native { metadata } => metadata.permission.resolve(&tool_use.input),
                ResolvedTool::Mcp { server, tool, .. } => normalize_mcp_capability(server, tool),
                ResolvedTool::Unknown { .. } => CapabilityRisk::High,
            };

            let state = match invoke_hooks!(PreToolUse, self, &mut tool_use) {
                Ok(HookControl::Continue) => {
                    let decision =
                        self.runtime
                            .permission_manager
                            .check(stable_name, risk, &tool_use.input);
                    match decision.behavior {
                        PermissionBehavior::Allow => PreparedState::Run,
                        PermissionBehavior::Deny => {
                            let msg = format!("Permission denied: {}", decision.reason);
                            self.emit_update(AgentUpdate::StepFailed {
                                idx: step_idx,
                                tool_id: id.clone(),
                                arg_summary: String::new(),
                                error: msg.clone(),
                            });
                            PreparedState::Resolved(msg)
                        }
                        PermissionBehavior::Ask => {
                            let permit_prompt = match &resolved {
                                ResolvedTool::Native { metadata } => metadata.permission_prompt,
                                _ => crate::tool::PermissionPromptPolicy::Json,
                            };
                            let prompt = format_permission_prompt(
                                stable_name,
                                permit_prompt,
                                &tool_use.input,
                            );

                            // `Notification` hooks fire when the agent surfaces
                            // a user notification; a permission prompt is the
                            // only kind Tact emits today. Observational.
                            let notification = NotificationContext {
                                notification_type: "permission_prompt".to_string(),
                                title: stable_name.to_string(),
                                message: prompt.clone(),
                            };
                            match invoke_hooks!(Notification, self, &notification) {
                                Ok(HookControl::Continue) => {}
                                Ok(HookControl::Block(reason)) => {
                                    self.emit_update(AgentUpdate::Info(format!(
                                        "[Notification hook blocked] {reason}"
                                    )));
                                }
                                Err(error) => {
                                    self.emit_update(AgentUpdate::Info(format!(
                                        "[Notification hook failed] {error}"
                                    )));
                                }
                            }

                            let choice = if let Some(tx) = &self.runtime.ui_tx {
                                let options = vec![
                                    "Allow once".to_string(),
                                    "Deny".to_string(),
                                    "Always allow this tool".to_string(),
                                ];
                                let responder = self.tool_context.ui_responder.clone();
                                // No artificial timeout: the popup is guaranteed
                                // to stay rendered (see
                                // `restore_pending_select_mode`), so the wait
                                // ends on a real user answer or when the UI
                                // closes (both route through the responder).
                                let selection = responder
                                    .request_select(tx, prompt, options, false)
                                    .await
                                    .ok()
                                    .flatten();
                                match selection {
                                    Some(0) => Some("allow_once"),
                                    Some(2) => Some("always_allow"),
                                    _ => Some("deny"),
                                }
                            } else {
                                let approved = self
                                    .runtime
                                    .permission_manager
                                    .ask_user(stable_name, risk)?;
                                if approved {
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
                                    self.runtime.permission_manager.allow_tool_with_input(
                                        stable_name,
                                        permit_prompt,
                                        &tool_use.input,
                                    );
                                    PreparedState::Run
                                }
                                _ => {
                                    let msg =
                                        format!("Permission denied by user for {}", stable_name);
                                    self.emit_update(AgentUpdate::StepFailed {
                                        idx: step_idx,
                                        tool_id: id.clone(),
                                        arg_summary: String::new(),
                                        error: msg.clone(),
                                    });
                                    PreparedState::Resolved(msg)
                                }
                            }
                        }
                    }
                }
                Ok(HookControl::Block(reason)) => {
                    let msg = format!("Tool blocked by PreToolUse hook: {reason}");
                    self.emit_update(AgentUpdate::StepFailed {
                        idx: step_idx,
                        tool_id: id.clone(),
                        arg_summary: String::new(),
                        error: msg.clone(),
                    });
                    PreparedState::Resolved(msg)
                }
                Err(error) => {
                    let msg = format!("PreToolUse hook failed: {error}");
                    self.emit_update(AgentUpdate::StepFailed {
                        idx: step_idx,
                        tool_id: id.clone(),
                        arg_summary: String::new(),
                        error: msg.clone(),
                    });
                    PreparedState::Resolved(msg)
                }
            };

            // Pre-call snapshot of the task a Task-domain call is about to
            // touch, so phase 2 can render a before→after title.
            let task_before = match &resolved {
                ResolvedTool::Native { metadata } => match metadata.domain {
                    ToolDomain::Task(TaskOperation::Update | TaskOperation::Get) => {
                        task_by_id(&self.tool_context.task_manager, input).await
                    }
                    _ => None,
                },
                _ => None,
            };

            prepared.push(PreparedTool {
                id: tool_use.id,
                name: tool_use.name,
                input: tool_use.input,
                step_idx,
                permission_label,
                state,
                resolved,
                task_before,
            });
        }

        // Phase-1 pre-flight is complete. Stamp the permission snapshot now so
        // a `spawn_subagent` spawned in this turn inherits the parent's
        // *current* mode / allow-list / settings — including any "always allow"
        // granted earlier in this same turn. This must happen before the wave
        // loop borrows `self.tool_context` below (the borrow checker rejects a
        // later mutable access).
        self.tool_context.permission_snapshot = Some(self.runtime.permission_manager.snapshot());
        // Stamp the pending-results queue handle so a detached async subagent
        // task can push its summary back for re-injection.
        self.tool_context.subagent_results = Some(self.runtime.pending_subagent_results.clone());

        Ok(Preflight {
            prepared,
            cancelled: false,
        })
    }

    /// Phase 2 — execute the runnable calls in conflict-free waves.
    ///
    /// Returns the per-call outputs (indexed like `prepared`; `None` for calls
    /// that were skipped or never reached) plus the pending manual-compaction
    /// focus.
    ///
    /// Calls are grouped so that tools with overlapping resource footprints
    /// never run concurrently ([`super::tool_schedule::waves_grouped`]). A
    /// cancel request is honoured at wave boundaries only: dropping
    /// `FuturesUnordered` cancels the not-yet-polled calls, while an in-flight
    /// tool is allowed to finish.
    async fn run_tool_waves(
        &mut self,
        prepared: &[PreparedTool],
    ) -> Result<(Vec<Option<ExecResult>>, Option<String>)> {
        let run_indices: Vec<usize> = prepared
            .iter()
            .enumerate()
            .filter(|(_, p)| matches!(p.state, PreparedState::Run))
            .map(|(i, _)| i)
            .collect();

        let resources: Vec<super::tool_schedule::ToolResources> = run_indices
            .iter()
            .map(|&i| tool_resources_for(&prepared[i], &self.tool_context.work_dir))
            .collect();

        if !run_indices.is_empty() {
            let names: Vec<String> = run_indices
                .iter()
                .map(|&i| prepared[i].name.clone())
                .collect();
            self.persist_tool_schedule(&super::tool_schedule::summarize(&names, &resources))
                .await;
        }

        let mut outputs: Vec<Option<ExecResult>> = (0..prepared.len()).map(|_| None).collect();
        let mut manual_compact = None;

        for wave in super::tool_schedule::waves_grouped(&resources) {
            if self.cancel_requested() {
                self.emit_update(AgentUpdate::Info("Cancelled by user".into()));
                return Ok((outputs, manual_compact));
            }
            let mut futures = FuturesUnordered::new();
            for &pos in &wave {
                let pi = run_indices[pos];
                let tools = &self.tools;
                let mcp = &self.mcp_router;
                let ctx = &self.tool_context;
                let prep = &prepared[pi];
                let is_mcp = matches!(prep.resolved, ResolvedTool::Mcp { .. });
                let output_policy = match &prep.resolved {
                    ResolvedTool::Native { metadata } => metadata.output,
                    _ => OutputPolicy::PersistLargeOutput,
                };
                futures.push(async move {
                    let start = std::time::Instant::now();
                    let exec = if is_mcp {
                        run_mcp_tool(mcp, ctx, &prep.id, &prep.name, &prep.input).await
                    } else {
                        run_native_tool(
                            tools,
                            ctx,
                            &prep.id,
                            &prep.name,
                            &prep.input,
                            output_policy,
                        )
                        .await
                    };
                    (pi, exec, start.elapsed().as_micros() as u64)
                });
            }
            let mut pending_durations_us: Vec<u64> = Vec::new();
            let mut pending_recent_files: Vec<String> = Vec::new();
            while let Some((pi, exec, duration_us)) = futures.next().await {
                let prep = &prepared[pi];
                let prep_id = prep.id.clone();
                let prep_name = prep.name.clone();
                let prep_input = prep.input.clone();
                let prep_step_idx = prep.step_idx;
                let prep_permission_label = prep.permission_label.clone();

                let tool_use = ToolUse {
                    id: prep_id.clone(),
                    name: prep_name.clone(),
                    input: prep_input.clone(),
                };
                let exec_content = exec.content.clone();
                let exec_image = exec.image.clone();
                let exec_status = exec.status;
                let mut tool_result = ToolResult {
                    tool_use_id: prep_id.clone(),
                    content: exec_content.clone(),
                };
                let (content_after_hook, final_status) = match invoke_hooks!(
                    PostToolUse,
                    self,
                    &tool_use,
                    &mut tool_result,
                    exec_status
                ) {
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
                let exec_output = content_after_hook.clone();

                // `PostToolUseFailure` fires only when the tool *itself* failed
                // (Claude Code: `PostToolUse` covers the success path,
                // `PostToolUseFailure` covers the error path). A hook `Block` is
                // observational — the tool already failed. The raw execution
                // error (not the PostToolUse-rewritten content) is passed on.
                if matches!(exec_status, StepStatus::Failed) {
                    let error_text = exec_content;
                    match invoke_hooks!(PostToolUseFailure, self, &tool_use, error_text.as_str()) {
                        Ok(HookControl::Continue) => {}
                        Ok(HookControl::Block(reason)) => {
                            self.emit_update(AgentUpdate::Info(format!(
                                "[PostToolUseFailure hook blocked] {reason}"
                            )));
                        }
                        Err(error) => {
                            self.emit_update(AgentUpdate::Info(format!(
                                "[PostToolUseFailure hook failed] {error}"
                            )));
                        }
                    }
                }
                pending_durations_us.push(duration_us);
                let summary = exec_output.chars().take(200).collect::<String>();
                let task_before = prepared[pi].task_before.clone();

                // Post-call task state, so the title can render before→after.
                let task_after = match &prep.resolved {
                    ResolvedTool::Native { metadata } => match metadata.domain {
                        ToolDomain::Task(TaskOperation::Create) => {
                            task_created(&self.tool_context.task_manager, &prep_input).await
                        }
                        ToolDomain::Task(TaskOperation::Update | TaskOperation::Get) => {
                            task_by_id(&self.tool_context.task_manager, &prep_input).await
                        }
                        _ => None,
                    },
                    _ => None,
                };
                let (arg_full, arg_summary) = arg_pair(
                    &prep.resolved,
                    &prep_input,
                    task_before.as_ref(),
                    task_after.as_ref(),
                );

                let detail_policy = match &prep.resolved {
                    ResolvedTool::Native { metadata } => metadata.presentation.detail,
                    _ => DetailPolicy::Result,
                };
                let detail =
                    step_result_detail(detail_policy, &prep_input, &exec_output, &final_status);
                let succeeded = matches!(final_status, StepStatus::Success);

                let presentation = match &prep.resolved {
                    ResolvedTool::Native { metadata } => {
                        make_presentation_for(metadata, &prep_input)
                    }
                    _ => ToolPresentationInfo::generic(prep_name.clone()),
                };

                self.emit_update(AgentUpdate::StepFinished {
                    idx: prep_step_idx,
                    tool_id: prep_id,
                    result: StepResult {
                        tool: prep_name.clone(),
                        arg_summary,
                        arg_full: Some(arg_full),
                        status: final_status,
                        message: summary,
                        detail,
                        duration_us: Some(duration_us),
                        permission_label: prep_permission_label,
                        presentation,
                    },
                });

                if succeeded && let ResolvedTool::Native { metadata } = &prep.resolved {
                    pending_recent_files.extend(metadata.resources.recent_paths(&prep_input));
                }

                // Compact effect
                if matches!(&prep.resolved, ResolvedTool::Native { metadata } if metadata.name == "compact")
                    && succeeded
                {
                    manual_compact = prep_input
                        .get("focus")
                        .and_then(|v| v.as_str())
                        .map(|s| {
                            if s.is_empty() {
                                String::new()
                            } else {
                                s.to_string()
                            }
                        })
                        .or_else(|| Some(String::new()));
                }

                self.record_tool_stats(&prep_name, succeeded, duration_us);
                outputs[pi] = Some(ExecResult {
                    content: exec_output,
                    status: final_status,
                    image: exec_image,
                });
            }
            drop(futures);
            for duration_us in pending_durations_us {
                self.runtime
                    .stats
                    .write_recover()
                    .tool_durations_ms
                    .push(duration_us / 1000);
            }
            for path in pending_recent_files {
                self.remember_recent_file(&path);
            }
        }

        Ok((outputs, manual_compact))
    }

    fn append_cancelled_tool_uses(
        &mut self,
        prepared: &mut Vec<PreparedTool>,
        content: &[ContentBlock],
    ) {
        for block in content.iter().skip(prepared.len()) {
            let ContentBlock::ToolUse { id, name, input } = block else {
                continue;
            };
            let step_idx = self.next_step_idx();
            self.emit_update(AgentUpdate::StepFailed {
                idx: step_idx,
                tool_id: id.clone(),
                arg_summary: String::new(),
                error: TOOL_CANCELLED_MSG.to_string(),
            });
            prepared.push(PreparedTool {
                id: id.clone(),
                name: name.clone(),
                input: input.clone(),
                step_idx,
                permission_label: None,
                state: PreparedState::Resolved(TOOL_CANCELLED_MSG.to_string()),
                resolved: ResolvedTool::Unknown { name: name.clone() },
                task_before: None,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tact_protocol::StepStatus;

    #[test]
    fn tool_detail_content_edit_file_returns_new_text() {
        let input = serde_json::json!({"path": "src/lib.rs", "old_text": "fn old() {}", "new_text": "fn new() {}"});
        let out = tool_detail_content(DetailPolicy::InputField("new_text"), &input, "wrote");
        assert_eq!(out.as_deref(), Some("fn new() {}"));
    }

    #[test]
    fn step_result_detail_on_failure_returns_full_output() {
        let out = step_result_detail(
            DetailPolicy::Result,
            &serde_json::json!({"path": "src/lib.rs"}),
            "Error: Text not found",
            &StepStatus::Failed,
        );
        assert_eq!(out.as_deref(), Some("Error: Text not found"));
    }

    #[test]
    fn step_result_detail_on_success_uses_tool_specific_rules() {
        let out = step_result_detail(
            DetailPolicy::Result,
            &serde_json::json!({"command": "echo hi"}),
            "hi\n",
            &StepStatus::Success,
        );
        assert_eq!(out.as_deref(), Some("hi\n"));
        let out = step_result_detail(
            DetailPolicy::InputField("content"),
            &serde_json::json!({"path": "a.rs", "content": "fn main(){}"}),
            "wrote",
            &StepStatus::Success,
        );
        assert_eq!(out.as_deref(), Some("fn main(){}"));
        let out = step_result_detail(
            DetailPolicy::None,
            &serde_json::json!({}),
            "matches",
            &StepStatus::Success,
        );
        assert!(out.is_none());
    }

    #[test]
    fn path_policy_arg_full() {
        let full = tool_arg_full(
            ArgumentSummaryPolicy::Path { field: "path" },
            &serde_json::json!({"path": "src/lib.rs"}),
        );
        assert_eq!(full, "src/lib.rs");
    }

    #[test]
    fn patch_preview_summary() {
        let full = tool_arg_full(
            ArgumentSummaryPolicy::PatchPreview {
                patch_field: "patch",
            },
            &serde_json::json!({
            "patch": "diff --git a/src/lib.rs" }),
        );
        assert!(full.starts_with("patch: diff --git"));
    }

    #[test]
    fn long_bash_summary_is_truncated() {
        let cmd = "x".repeat(200);
        let full = tool_arg_full(
            ArgumentSummaryPolicy::Command { field: "command" },
            &serde_json::json!({"command": cmd}),
        );
        assert_eq!(truncate_tool_arg_summary(&full).chars().count(), 120);
    }

    #[test]
    fn short_bash_summary_is_preserved() {
        let full = tool_arg_full(
            ArgumentSummaryPolicy::Command { field: "command" },
            &serde_json::json!({"command": "git status"}),
        );
        assert_eq!(full, "git status");
    }

    #[test]
    fn arg_pair_for_returns_full_and_bounded_summary() {
        let (full, summary) = arg_pair_for(
            ArgumentSummaryPolicy::Command { field: "command" },
            &serde_json::json!({"command": "x".repeat(200)}),
        );
        assert_eq!(full.chars().count(), 200);
        assert_eq!(summary.chars().count(), MAX_TOOL_ARG_SUMMARY_CHARS);
        assert!(summary.ends_with("..."));
    }

    #[test]
    fn arg_pair_routes_task_domain_through_task_title() {
        let router = crate::tool::toolset();
        let metadata = router.resolve("task_update").unwrap().metadata();
        let resolved = ResolvedTool::Native { metadata };
        let (full, summary) = arg_pair(
            &resolved,
            &serde_json::json!({"task_id": 7, "status": "completed"}),
            None,
            None,
        );
        assert_eq!(summary, full, "a short task title needs no truncation");
        assert!(!full.is_empty());
    }

    #[test]
    fn arg_pair_uses_json_for_unknown_tools() {
        let resolved = ResolvedTool::Unknown {
            name: "nope".into(),
        };
        let (full, _) = arg_pair(&resolved, &serde_json::json!({"a": 1}), None, None);
        assert_eq!(full, "{\"a\":1}");
    }

    #[test]
    fn task_arg_pair_truncates_long_titles() {
        let (full, summary) = task_arg_pair(
            TaskOperation::Create,
            &serde_json::json!({"subject": "s".repeat(400)}),
            None,
            None,
        );
        assert!(full.chars().count() > TASK_SUMMARY_CHARS);
        assert_eq!(summary.chars().count(), TASK_SUMMARY_CHARS);
        assert!(summary.ends_with("..."));
    }

    fn prepared_spawn_subagent(worktree: bool) -> PreparedTool {
        PreparedTool {
            id: "t1".into(),
            name: "spawn_subagent".into(),
            input: if worktree {
                serde_json::json!({ "prompt": "p", "worktree": true })
            } else {
                serde_json::json!({ "prompt": "p" })
            },
            step_idx: 0,
            permission_label: None,
            state: PreparedState::Run,
            resolved: ResolvedTool::Native {
                metadata: &crate::tool::SPAWN_SUBAGENT_METADATA,
            },
            task_before: None,
        }
    }

    #[test]
    fn worktree_subagent_resources_are_independent() {
        let prep = prepared_spawn_subagent(true);
        let resources = tool_resources_for(&prep, std::path::Path::new("/tmp"));
        assert!(
            !resources.barrier,
            "isolated subagent must not be a barrier"
        );
        assert!(resources.reads.is_empty());
        assert!(resources.writes.is_empty());
    }

    #[test]
    fn non_worktree_subagent_stays_barrier() {
        let prep = prepared_spawn_subagent(false);
        let resources = tool_resources_for(&prep, std::path::Path::new("/tmp"));
        assert!(
            resources.barrier,
            "non-isolated subagent must stay a barrier"
        );
    }
}
