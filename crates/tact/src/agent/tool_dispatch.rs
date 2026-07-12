//! Tool-call dispatch: pre-flight, parallel execution, and result assembly.

use super::Agent;

use anyhow::Result;
use futures_util::{StreamExt, stream::FuturesUnordered};
use tact_llm::ContentBlock;

use crate::compact::persist_large_output;
use crate::hook::{HookControl, ToolResult, ToolUse};
use crate::invoke_hooks;
use crate::mcp::MCPToolRouter;
use crate::permission::PermissionBehavior;
use crate::tool::{ToolContext, ToolRouter};
use tact_protocol::{AgentUpdate, StepResult, StepStatus};

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

const TOOL_CANCELLED_MSG: &str = "Cancelled by user";

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

/// Run a single MCP tool. The router is shared immutably so different servers
/// can execute concurrently within the same wave.
async fn run_mcp_tool(
    mcp_router: &MCPToolRouter,
    name: &str,
    input: &serde_json::Value,
) -> ExecResult {
    match mcp_router.call(name, input.clone()).await {
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

fn tool_detail_content(name: &str, input: &serde_json::Value, exec_output: &str) -> Option<String> {
    match name {
        "read_file" | "run_command" | "bash" | "shell" => Some(exec_output.to_string()),
        "write_file" => input
            .get("content")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        "edit_file" => input
            .get("new_text")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        _ => None,
    }
}

fn step_result_detail(
    name: &str,
    input: &serde_json::Value,
    exec_output: &str,
    status: &StepStatus,
) -> Option<String> {
    if matches!(status, StepStatus::Failed) {
        Some(exec_output.to_string())
    } else {
        tool_detail_content(name, input, exec_output)
    }
}

impl Agent {
    /// Dispatch the tool calls in one assistant turn.
    ///
    /// Runs in three stages so that independent tools overlap while conflicting
    /// ones stay ordered:
    /// 1. **Pre-flight** (sequential): stats, step events, PreToolUse hooks, and
    ///    permission checks — the latter may prompt the user, so order matters.
    /// 2. **Execution** (parallel by wave): tools touching disjoint resources
    ///    run concurrently; a read/write or write/write on the same file (and
    ///    any unscoped "barrier" tool such as `bash`/MCP) is serialised. See
    ///    [`super::tool_schedule`].
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
                self.append_cancelled_tool_uses(&mut prepared, content);
                return Ok((build_tool_results(prepared, vec![]), None));
            }

            let step_idx = self.next_step_idx();
            let arg_full = tool_arg_full(name, input);
            let arg_summary = truncate_tool_arg_summary(&arg_full);
            let step_description = if arg_summary.is_empty() {
                name.clone()
            } else {
                format!("{name} ({arg_summary})")
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
                arg_summary,
                arg_full,
            });

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
                            self.emit_update(AgentUpdate::StepFailed {
                                idx: step_idx,
                                tool_id: id.clone(),
                                error: msg.clone(),
                            });
                            PreparedState::Resolved(msg)
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
        let resources: Vec<super::tool_schedule::ToolResources> = run_indices
            .iter()
            .map(|&i| {
                super::tool_schedule::tool_resources(
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
            self.persist_tool_schedule(&super::tool_schedule::summarize(&names, &resources))
                .await;
        }

        // Final tool outputs keyed by index into `prepared`. We still collect
        // them for deterministic tool_result ordering, but StepFinished is now
        // emitted immediately when each tool completes (instead of after a
        // whole wave joins), so parallel progress is visible in the UI.
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

            // A barrier wave always holds a single tool. Every other wave runs
            // concurrently over shared borrows (native and MCP).
            let mut futures = FuturesUnordered::new();
            for &pos in &wave {
                let pi = run_indices[pos];
                let tools = &self.tools;
                let mcp = &self.mcp_router;
                let ctx = &self.tool_context;
                let prep = &prepared[pi];
                let is_mcp = MCPToolRouter::is_mcp_tool(&prep.name);
                futures.push(async move {
                    let start = std::time::Instant::now();
                    let exec = if is_mcp {
                        run_mcp_tool(mcp, &prep.name, &prep.input).await
                    } else {
                        run_native_tool(tools, ctx, &prep.id, &prep.name, &prep.input).await
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
                let detail =
                    step_result_detail(&prep_name, &prep_input, &exec_output, &final_status);
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
                    },
                });
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
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{step_result_detail, tool_arg_summary};
    use tact_protocol::StepStatus;

    #[test]
    fn tool_detail_content_edit_file_returns_new_text() {
        let input = serde_json::json!({
            "path": "src/lib.rs",
            "old_text": "fn old() {}",
            "new_text": "fn new() {}",
        });
        let out = super::tool_detail_content("edit_file", &input, "wrote");
        assert_eq!(out.as_deref(), Some("fn new() {}"));
    }

    #[test]
    fn step_result_detail_on_failure_returns_full_output() {
        let input = serde_json::json!({"edits": []});
        let out = step_result_detail(
            "batch_edit",
            &input,
            "BatchEdit aborted — 1 validation error(s):\nEdit 0: bad",
            &StepStatus::Failed,
        );
        assert_eq!(
            out.as_deref(),
            Some("BatchEdit aborted — 1 validation error(s):\nEdit 0: bad")
        );
    }

    #[test]
    fn step_result_detail_on_success_uses_tool_specific_rules() {
        let input = serde_json::json!({"command": "echo hi"});
        let out = step_result_detail("bash", &input, "hi\n", &StepStatus::Success);
        assert_eq!(out.as_deref(), Some("hi\n"));

        let write = serde_json::json!({"path": "a.rs", "content": "fn main(){}"});
        let out = step_result_detail("write_file", &write, "wrote", &StepStatus::Success);
        assert_eq!(out.as_deref(), Some("fn main(){}"));

        let out = step_result_detail(
            "grep",
            &serde_json::json!({}),
            "matches",
            &StepStatus::Success,
        );
        assert!(out.is_none());
    }

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
