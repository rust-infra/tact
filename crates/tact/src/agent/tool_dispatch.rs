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
    hook::{HookControl, ToolResult, ToolUse},
    invoke_hooks,
    mcp::MCPToolRouter,
    permission::{
        CapabilityRisk, PermissionBehavior, format_permission_prompt, normalize_mcp_capability,
    },
    tool::{
        ArgumentSummaryPolicy, DetailPolicy, OutputPolicy, TaskOperation, ToolDomain, ToolRouter,
    },
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
    mut outputs: Vec<Option<String>>,
) -> Vec<ContentBlock> {
    prepared
        .into_iter()
        .enumerate()
        .map(|(idx, prep)| {
            let content = match prep.state {
                PreparedState::Resolved(msg) => msg,
                PreparedState::Run => outputs[idx]
                    .take()
                    .unwrap_or_else(|| TOOL_CANCELLED_MSG.to_string()),
            };
            ContentBlock::ToolResult {
                tool_use_id: prep.id,
                content,
            }
        })
        .collect()
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

// ── Argument formatting from metadata ────────────────────────────────────

fn tool_arg_full(policy: ArgumentSummaryPolicy, input: &serde_json::Value) -> String {
    fn str_field<'a>(input: &'a serde_json::Value, key: &str) -> &'a str {
        input.get(key).and_then(|v| v.as_str()).unwrap_or("")
    }
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
    fn str_field<'a>(input: &'a serde_json::Value, key: &str) -> &'a str {
        input.get(key).and_then(|v| v.as_str()).unwrap_or("")
    }
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
    match tools.call(&call_ctx, name, input.clone()).await {
        Ok(output) => {
            let tact_path = crate::consts::TactPath::new(&ctx.work_dir);
            match output_policy {
                OutputPolicy::PersistLargeOutput => {
                    match persist_large_output(&tact_path, tool_use_id, &output).await {
                        Ok(content) => ExecResult {
                            content,
                            status: StepStatus::Success,
                        },
                        Err(error) => ExecResult {
                            content: format!("Error persisting large output: {error}"),
                            status: StepStatus::Failed,
                        },
                    }
                }
                OutputPolicy::KeepInline => ExecResult {
                    content: output,
                    status: StepStatus::Success,
                },
            }
        }
        Err(e) => ExecResult {
            content: format!("Error invoking tool {}: {}", name, e),
            status: StepStatus::Failed,
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
                },
                Err(error) => ExecResult {
                    content: format!("Error persisting large MCP output: {error}"),
                    status: StepStatus::Failed,
                },
            }
        }
        Err(e) => ExecResult {
            content: format!("Error invoking MCP tool {}: {}", name, e),
            status: StepStatus::Failed,
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

// ── Main dispatch ────────────────────────────────────────────────────────

impl Agent {
    pub async fn execute_tool_call(
        &mut self,
        content: &[ContentBlock],
    ) -> Result<(Vec<ContentBlock>, Option<String>)> {
        // Phase 1: sequential pre-flight
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
                self.append_cancelled_tool_uses(&mut prepared, content);
                return Ok((build_tool_results(prepared, vec![]), None));
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

            // Argument formatting
            let (arg_full, arg_summary) = match &resolved {
                ResolvedTool::Native { metadata }
                    if matches!(metadata.domain, ToolDomain::Task(_)) =>
                {
                    let op = match metadata.domain {
                        ToolDomain::Task(op) => op,
                        _ => unreachable!(),
                    };
                    let full = crate::task::format_task_tool_title(op, input, None, None);
                    const TASK_SUMMARY_CHARS: usize = 240;
                    if full.chars().count() <= TASK_SUMMARY_CHARS {
                        (full.clone(), full)
                    } else {
                        let s = format!(
                            "{}...",
                            full.chars()
                                .take(TASK_SUMMARY_CHARS.saturating_sub(3))
                                .collect::<String>()
                        );
                        (full, s)
                    }
                }
                _ => {
                    let policy = match &resolved {
                        ResolvedTool::Native { metadata } => metadata.argument_summary,
                        _ => ArgumentSummaryPolicy::Json,
                    };
                    let full = tool_arg_full(policy, input);
                    let summary = truncate_tool_arg_summary(&full);
                    (full, summary)
                }
            };

            let step_description = if arg_summary.is_empty() {
                name.clone()
            } else {
                format!("{name} ({arg_summary})")
            };
            let presentation = match &resolved {
                ResolvedTool::Native { metadata } => make_presentation(metadata),
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
                                error: msg.clone(),
                            });
                            PreparedState::Resolved(msg)
                        }
                        PermissionBehavior::Ask => {
                            let permit_prompt = match &resolved {
                                ResolvedTool::Native { metadata } => metadata.permission_prompt,
                                _ => crate::tool::PermissionPromptPolicy::Json,
                            };
                            let choice = if let Some(tx) = &self.runtime.ui_tx {
                                let (respond_tx, respond_rx) = tokio::sync::oneshot::channel();
                                let prompt = format_permission_prompt(
                                    stable_name,
                                    permit_prompt,
                                    &tool_use.input,
                                );
                                let options = vec![
                                    "Allow once".to_string(),
                                    "Deny".to_string(),
                                    "Always allow this tool".to_string(),
                                ];
                                let _ = tx.send(AgentUpdate::RequestSelect {
                                    prompt,
                                    options,
                                    respond: respond_tx,
                                    log_confirm: false,
                                });
                                match respond_rx.await {
                                    Ok(Some(0)) => Some("allow_once"),
                                    Ok(Some(2)) => Some("always_allow"),
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
                        error: msg.clone(),
                    });
                    PreparedState::Resolved(msg)
                }
                Err(error) => {
                    let msg = format!("PreToolUse hook failed: {error}");
                    self.emit_update(AgentUpdate::StepFailed {
                        idx: step_idx,
                        tool_id: id.clone(),
                        error: msg.clone(),
                    });
                    PreparedState::Resolved(msg)
                }
            };

            let task_before = match &resolved {
                ResolvedTool::Native { metadata } => match metadata.domain {
                    ToolDomain::Task(TaskOperation::Update | TaskOperation::Get) => input
                        .get("task_id")
                        .and_then(|v| v.as_u64())
                        .and_then(|id| self.tool_context.task_manager.get(id).ok()),
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

        // Phase 2: execute in conflict-free waves
        let run_indices: Vec<usize> = prepared
            .iter()
            .enumerate()
            .filter(|(_, p)| matches!(p.state, PreparedState::Run))
            .map(|(i, _)| i)
            .collect();

        let resources: Vec<super::tool_schedule::ToolResources> = run_indices
            .iter()
            .map(|&i| match &prepared[i].resolved {
                ResolvedTool::Native { metadata } => {
                    super::tool_schedule::tool_resources_from_metadata(
                        &metadata.resources,
                        &prepared[i].input,
                        &self.tool_context.work_dir,
                    )
                }
                ResolvedTool::Mcp { server, .. } => {
                    super::tool_schedule::mcp_server_resources(server)
                }
                ResolvedTool::Unknown { .. } => super::tool_schedule::ToolResources::barrier(),
            })
            .collect();

        if !run_indices.is_empty() {
            let names: Vec<String> = run_indices
                .iter()
                .map(|&i| prepared[i].name.clone())
                .collect();
            self.persist_tool_schedule(&super::tool_schedule::summarize(&names, &resources))
                .await;
        }

        let mut outputs: Vec<Option<String>> = (0..prepared.len()).map(|_| None).collect();
        let mut manual_compact = None;

        for wave in super::tool_schedule::waves_grouped(&resources) {
            if self
                .runtime
                .cancel_flag
                .load(std::sync::atomic::Ordering::Relaxed)
            {
                self.emit_update(AgentUpdate::Info("Cancelled by user".into()));
                return Ok((build_tool_results(prepared, outputs), manual_compact));
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
                let task_before = prepared[pi].task_before.clone();

                let (arg_full, arg_summary) = match &prep.resolved {
                    ResolvedTool::Native { metadata }
                        if matches!(metadata.domain, ToolDomain::Task(_)) =>
                    {
                        let op = match metadata.domain {
                            ToolDomain::Task(op) => op,
                            _ => unreachable!(),
                        };
                        let after = match op {
                            TaskOperation::Create => {
                                let subject = prep_input
                                    .get("subject")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("");
                                self.tool_context.task_manager.list().ok().and_then(|list| {
                                    list.into_iter()
                                        .filter(|t| t.subject == subject)
                                        .max_by_key(|t| t.id)
                                })
                            }
                            TaskOperation::Update | TaskOperation::Get => prep_input
                                .get("task_id")
                                .and_then(|v| v.as_u64())
                                .and_then(|id| self.tool_context.task_manager.get(id).ok()),
                            _ => None,
                        };
                        let full = crate::task::format_task_tool_title(
                            op,
                            &prep_input,
                            task_before.as_ref(),
                            after.as_ref(),
                        );
                        const TASK_SUMMARY_CHARS: usize = 240;
                        let s = if full.chars().count() <= TASK_SUMMARY_CHARS {
                            full.clone()
                        } else {
                            format!(
                                "{}...",
                                full.chars()
                                    .take(TASK_SUMMARY_CHARS.saturating_sub(3))
                                    .collect::<String>()
                            )
                        };
                        (full, s)
                    }
                    _ => {
                        let policy = match &prep.resolved {
                            ResolvedTool::Native { metadata } => metadata.argument_summary,
                            _ => ArgumentSummaryPolicy::Json,
                        };
                        let full = tool_arg_full(policy, &prep_input);
                        (full.clone(), truncate_tool_arg_summary(&full))
                    }
                };

                let detail_policy = match &prep.resolved {
                    ResolvedTool::Native { metadata } => metadata.presentation.detail,
                    _ => DetailPolicy::Result,
                };
                let detail =
                    step_result_detail(detail_policy, &prep_input, &exec_output, &final_status);
                let succeeded = matches!(final_status, StepStatus::Success);

                let presentation = match &prep.resolved {
                    ResolvedTool::Native { metadata } => make_presentation(metadata),
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

                if succeeded {
                    *self
                        .runtime
                        .stats
                        .tool_success_counts
                        .entry(prep_name.clone())
                        .or_insert(0) += 1;
                } else {
                    *self
                        .runtime
                        .stats
                        .tool_failure_counts
                        .entry(prep_name.clone())
                        .or_insert(0) += 1;
                }
                *self
                    .runtime
                    .stats
                    .tool_total_durations_ms
                    .entry(prep_name.clone())
                    .or_insert(0) += duration_us / 1000;
                *self
                    .runtime
                    .stats
                    .tool_timing_counts
                    .entry(prep_name.clone())
                    .or_insert(0) += 1;
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

        Ok((build_tool_results(prepared, outputs), manual_compact))
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
}
