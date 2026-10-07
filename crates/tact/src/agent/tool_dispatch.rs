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
    compact::{persist_large_output, persist_large_output_over_tokens},
    hook::{HookControl, NotificationContext, ToolResult, ToolUse},
    invoke_hooks,
    mcp::MCPToolRouter,
    permission::{CapabilityRisk, PermissionBehavior, PermissionManager, format_permission_prompt},
    tool::{
        ArgumentSummaryPolicy, DetailPolicy, OutputPolicy, TaskOperation, ToolDomain, ToolRouter,
    },
    utils::RwLockExt,
};

/// What the interactive permission popup's selection means.
///
/// A typed replacement for the `"allow_once"` strings this used to carry: the
/// `match` on it is exhaustive, so a new option cannot silently fall through to
/// whichever arm happened to be last.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PermissionChoice {
    AllowOnce,
    AllowForSession,
    AlwaysAllow,
    AlwaysAllowProgram,
    Deny,
}

/// The permission popup's options, in the order the popup shows them.
///
/// Indices are load-bearing — [`permission_choice_for`] is the only reader — so
/// the list is stated once. `Deny` keeps index 1 and "always allow this tool"
/// keeps index 2, because those positions are muscle memory; every later option
/// is appended rather than inserted.
const PERMISSION_OPTIONS: [&str; 5] = [
    "Allow once",
    "Deny",
    "Always allow this tool",
    "Allow for this session",
    "Always allow this pattern",
];

/// The options to show, dropping the program-level one when no rule for it
/// could be built.
///
/// Dropping the *last* entry is what keeps every other index stable. A disabled
/// entry would be worse than an absent one: it advertises a gesture that
/// records nothing, which is the shape of bug the "no rule could be narrowed"
/// notice exists to prevent.
fn permission_options(with_program_choice: bool) -> Vec<String> {
    let count = if with_program_choice {
        PERMISSION_OPTIONS.len()
    } else {
        PERMISSION_OPTIONS.len() - 1
    };
    PERMISSION_OPTIONS[..count]
        .iter()
        .map(|option| (*option).to_string())
        .collect()
}

/// One label for both prefix kinds: the preview is what names the rule, and a
/// second wording ("this program" vs "this folder") would be a second thing to
/// keep in step with the generator for no gain.
///
/// Which decision a popup selection means.
///
/// Anything unexpected denies, which is the only safe default: a cancelled
/// popup arrives as `None`, and an out-of-range index can only come from a UI
/// that disagrees with [`PERMISSION_OPTIONS`].
fn permission_choice_for(selection: Option<usize>) -> PermissionChoice {
    match selection {
        Some(0) => PermissionChoice::AllowOnce,
        Some(2) => PermissionChoice::AlwaysAllow,
        Some(3) => PermissionChoice::AllowForSession,
        Some(4) => PermissionChoice::AlwaysAllowProgram,
        _ => PermissionChoice::Deny,
    }
}

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
    /// `list_mcp_resources` / `read_mcp_resource` — Codex's native resource
    /// tools, served by the MCP router rather than by a `Tool` of their own.
    McpResource {
        tool: crate::mcp::McpResourceTool,
    },
    /// `list_mcp_prompts` / `get_mcp_prompt` — the prompt tools, served the same
    /// way for the same reason.
    McpPrompt {
        tool: crate::mcp::McpPromptTool,
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

/// A `ToolResult` written for a `ToolUse` that was never executed. Each reason
/// is spelled out, because the result is persisted: a later reader (human or
/// model) has to be able to tell a user cancel from a turn that died on a
/// refusal / an unrecognized stop reason / a truncated response.
pub(super) const TOOL_CANCELLED_MSG: &str = "Cancelled by user";
pub(super) const TOOL_REFUSED_MSG: &str = "Not executed: the model refused this request";
pub(super) const TOOL_UNKNOWN_STOP_MSG: &str = "Not executed: unrecognized stop reason";
pub(super) const TOOL_TRUNCATED_MSG: &str =
    "Not executed: output was truncated before this tool ran";
pub(super) const TOOL_TURN_ENDED_MSG: &str = "Not executed: the turn ended before this tool ran";
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
        ArgumentSummaryPolicy::Id { field } => str_field(input, field).to_string(),
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
    stream_redaction: crate::security::RedactionLevel,
) -> ExecResult {
    let call_ctx = ctx.for_invocation_with_redaction(tool_use_id, stream_redaction);
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
            // A `tools.<name>.output_token_limit` on the server's entry wins
            // over the session-wide character threshold; without one the
            // global rule applies unchanged.
            let persisted = match mcp_router.output_token_limit(name) {
                Some(limit) => {
                    persist_large_output_over_tokens(&tact_path, tool_use_id, &output, limit).await
                }
                None => persist_large_output(&tact_path, tool_use_id, &output).await,
            };
            match persisted {
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

/// Whether `name` is one of Codex's two MCP resource tools.
fn is_mcp_resource_tool(name: &str) -> bool {
    crate::mcp::McpResourceTool::from_name(name).is_some()
}

/// Whether `name` is one of Tact's two MCP prompt tools.
fn is_mcp_prompt_tool(name: &str) -> bool {
    crate::mcp::McpPromptTool::from_name(name).is_some()
}

/// Runs the two prompt tools against the live router.
///
/// Outside `run_mcp_tool` for the resource tools' reason: those take an
/// `mcp__<server>__<tool>` name parsed into a server/tool pair, and a prompt has
/// no tool name at all.
async fn run_mcp_prompt_tool(
    mcp_router: &MCPToolRouter,
    tool: crate::mcp::McpPromptTool,
    input: &serde_json::Value,
) -> ExecResult {
    let server = input.get("server").and_then(|value| value.as_str());
    let result = match tool {
        crate::mcp::McpPromptTool::List => mcp_router.list_prompts(server).await,
        crate::mcp::McpPromptTool::Get => {
            let Some(server) = server else {
                return ExecResult {
                    content: "Error invoking get_mcp_prompt: `server` is required".to_string(),
                    status: StepStatus::Failed,
                    image: None,
                };
            };
            let Some(name) = input.get("name").and_then(|value| value.as_str()) else {
                return ExecResult {
                    content: "Error invoking get_mcp_prompt: `name` is required".to_string(),
                    status: StepStatus::Failed,
                    image: None,
                };
            };
            match prompt_arguments(input) {
                Ok(arguments) => mcp_router.get_prompt(server, name, arguments).await,
                Err(message) => Err(anyhow::anyhow!(message)),
            }
        }
    };

    match result {
        Ok(content) => ExecResult {
            content,
            status: StepStatus::Success,
            image: None,
        },
        Err(error) => ExecResult {
            content: format!("Error invoking {}: {error}", tool.name()),
            status: StepStatus::Failed,
            image: None,
        },
    }
}

/// Collects `get_mcp_prompt`'s `arguments` object.
///
/// The protocol's prompt arguments are a *string* map, so a number or a bool is
/// stringified rather than dropped — a model that writes `{"count": 3}` means
/// `"3"`. Anything else (an object, an array, null) is refused by name: silently
/// omitting it would hand the server a template with an unfilled placeholder and
/// report success.
fn prompt_arguments(
    input: &serde_json::Value,
) -> Result<Option<serde_json::Map<String, serde_json::Value>>, String> {
    let Some(map) = input.get("arguments").and_then(|value| value.as_object()) else {
        return Ok(None);
    };
    let mut arguments = serde_json::Map::new();
    for (key, value) in map {
        let text = match value {
            serde_json::Value::String(text) => text.clone(),
            serde_json::Value::Number(_) | serde_json::Value::Bool(_) => value.to_string(),
            _ => {
                return Err(format!("argument `{key}` must be a string (got {value})"));
            }
        };
        arguments.insert(key.clone(), serde_json::Value::String(text));
    }
    Ok((!arguments.is_empty()).then_some(arguments))
}

/// Runs the three resource tools against the live router.
///
/// Deliberately outside `run_mcp_tool`: those tools take a `mcp__<server>__<tool>`
/// name parsed into a server/tool pair, and a resource has no tool name at all.
async fn run_mcp_resource_tool(
    mcp_router: &MCPToolRouter,
    tool: crate::mcp::McpResourceTool,
    input: &serde_json::Value,
) -> ExecResult {
    let server = input.get("server").and_then(|value| value.as_str());
    let result = match tool {
        crate::mcp::McpResourceTool::List => mcp_router.list_resources(server).await,
        crate::mcp::McpResourceTool::Templates => mcp_router.list_resource_templates(server).await,
        crate::mcp::McpResourceTool::Read => {
            let Some(server) = server else {
                return ExecResult {
                    content: "Error invoking read_mcp_resource: `server` is required".to_string(),
                    status: StepStatus::Failed,
                    image: None,
                };
            };
            match input.get("uri").and_then(|value| value.as_str()) {
                Some(uri) => mcp_router.read_resource(server, uri).await,
                None => Err(anyhow::anyhow!("`uri` is required")),
            }
        }
    };

    match result {
        Ok(content) => ExecResult {
            content,
            status: StepStatus::Success,
            image: None,
        },
        Err(error) => ExecResult {
            content: format!("Error invoking {}: {error}", tool.name()),
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
        // A listing may touch every server, so only a *read* can be scoped to
        // one; a listing is a barrier over all of them.
        ResolvedTool::McpResource {
            tool: crate::mcp::McpResourceTool::Read,
        } => match prep.input.get("server").and_then(|value| value.as_str()) {
            Some(server) => super::tool_schedule::mcp_server_resources(server),
            None => super::tool_schedule::ToolResources::barrier(),
        },
        ResolvedTool::McpResource { .. } => super::tool_schedule::ToolResources::barrier(),
        // Same split for prompts: a get names one server, a listing spans all.
        ResolvedTool::McpPrompt {
            tool: crate::mcp::McpPromptTool::Get,
        } => match prep.input.get("server").and_then(|value| value.as_str()) {
            Some(server) => super::tool_schedule::mcp_server_resources(server),
            None => super::tool_schedule::ToolResources::barrier(),
        },
        ResolvedTool::McpPrompt { .. } => super::tool_schedule::ToolResources::barrier(),
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
            // Nothing ran. A call approved before the flag flipped still needs
            // a result — and `build_tool_results` indexes `outputs` by
            // position, so an empty vec would panic on it. Every entry is
            // answered as cancelled, which is what the wave-boundary cancel
            // already writes for a call it approved but never started.
            let outputs = (0..preflight.prepared.len()).map(|_| None).collect();
            return Ok((build_tool_results(preflight.prepared, outputs), None));
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
                self.append_unexecuted_tool_uses(&mut prepared, content, TOOL_CANCELLED_MSG);
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
                    Ok(None) | Err(_) if is_mcp_resource_tool(name) => ResolvedTool::McpResource {
                        tool: crate::mcp::McpResourceTool::from_name(name)
                            .expect("guarded by is_mcp_resource_tool"),
                    },
                    Ok(None) | Err(_) if is_mcp_prompt_tool(name) => ResolvedTool::McpPrompt {
                        tool: crate::mcp::McpPromptTool::from_name(name)
                            .expect("guarded by is_mcp_prompt_tool"),
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
                ResolvedTool::McpResource { tool } => tool.name(),
                ResolvedTool::McpPrompt { tool } => tool.name(),
                ResolvedTool::Unknown { name } => name.as_str(),
            };
            let risk = match &resolved {
                ResolvedTool::Native { metadata } => metadata.permission.resolve(&tool_use.input),
                // The entry declares a tier through `tools.<name>.risk` or
                // `default_tool_risk`; a silent entry keeps High.
                ResolvedTool::Mcp { server, tool, .. } => self.mcp_router.risk_for(server, tool),
                // Tact's own resource tools: no server entry can declare their
                // risk, so `[mcp]`'s two keys do, and both default to High.
                ResolvedTool::McpResource { tool } => crate::mcp::resource_tool_risk(*tool),
                // Tact's own prompt tools: same rule, `[mcp]`'s prompt keys.
                ResolvedTool::McpPrompt { tool } => crate::mcp::prompt_tool_risk(*tool),
                ResolvedTool::Unknown { .. } => CapabilityRisk::High,
            };

            // ── Sensitive-target guard ──────────────────────────────────────
            //
            // Deliberately *here*, before the PreToolUse hook and before the
            // permission ladder: a `Credential` hit has to be unreachable by
            // `Auto` mode, by an explicit settings `allow` rule, by an
            // in-session "always allow", and by a `PermissionRequest` hook.
            // Anything evaluated later can be talked out of it; this cannot.
            //
            // The `Secret` tier needs no special case — it escalates to `High`
            // below and then follows the ordinary decision path, so plan mode
            // denies it, Default asks, and the user's rules compose as usual.
            let sensitive_hit = match &resolved {
                ResolvedTool::Native { metadata } => metadata
                    .permission
                    .sensitive(&tool_use.input, &self.runtime.security),
                // MCP tools are third-party and High by default; no path field
                // of theirs is known here. Their *results* are still redacted.
                _ => None,
            };
            if let Some(hit) = &sensitive_hit
                && hit.tier == crate::security::sensitive::Tier::Credential
            {
                let msg = crate::security::sensitive::refusal_text(hit);
                self.emit_update(AgentUpdate::StepFailed {
                    idx: step_idx,
                    tool_id: id.clone(),
                    arg_summary: String::new(),
                    error: msg.clone(),
                });
                prepared.push(PreparedTool {
                    id: id.clone(),
                    name: name.clone(),
                    input: tool_use.input.clone(),
                    step_idx,
                    permission_label: Some(format!("Refused: {}", hit.kind.label())),
                    state: PreparedState::Resolved(msg),
                    resolved,
                    task_before: None,
                });
                continue;
            }
            // A `Secret` hit escalates the declared risk; the manager then
            // decides it like any other High call.
            let risk = if sensitive_hit.is_some() {
                CapabilityRisk::High
            } else {
                risk
            };
            // An MCP entry's `approval_mode: "auto"` skips the default prompt
            // without lowering the risk: it must not unlock plan mode, and an
            // explicit local deny/ask rule still wins.
            let auto_approved = match &resolved {
                ResolvedTool::Mcp { server, tool, .. } => {
                    self.mcp_router.is_auto_approved(server, tool)
                }
                _ => false,
            };

            let state = match invoke_hooks!(PreToolUse, self, &mut tool_use) {
                Ok(HookControl::Continue) => {
                    let decision = self.runtime.permission_manager.check_with_auto(
                        stable_name,
                        risk,
                        &tool_use.input,
                        auto_approved,
                    );
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
                            // Codex's `PermissionRequest`: runs only when a
                            // prompt would actually appear, so a policy hook
                            // answers it without paying for every call.
                            // `allow` skips the prompt, `block` denies outright,
                            // anything else leaves the decision to the user.
                            let hook_control =
                                match invoke_hooks!(PermissionRequest, self, &mut tool_use) {
                                    Ok(control) => control,
                                    Err(error) => {
                                        // Fail-open, like every other hook failure:
                                        // a broken policy hook must not block work.
                                        self.emit_update(AgentUpdate::Info(format!(
                                            "[PermissionRequest hook failed] {error}"
                                        )));
                                        HookControl::Continue
                                    }
                                };
                            match hook_control {
                                HookControl::Allow => {
                                    permission_label =
                                        Some("Allowed by PermissionRequest hook".to_string());
                                    PreparedState::Run
                                }
                                HookControl::Block(reason) => {
                                    let msg = format!(
                                        "Permission denied by PermissionRequest hook: {reason}"
                                    );
                                    self.emit_update(AgentUpdate::StepFailed {
                                        idx: step_idx,
                                        tool_id: id.clone(),
                                        arg_summary: String::new(),
                                        error: msg.clone(),
                                    });
                                    PreparedState::Resolved(msg)
                                }
                                HookControl::Continue => {
                                    let permit_prompt = match &resolved {
                                        ResolvedTool::Native { metadata } => {
                                            metadata.permission_prompt
                                        }
                                        _ => crate::tool::PermissionPromptPolicy::Json,
                                    };
                                    let mut prompt = format_permission_prompt(
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
                                        Ok(HookControl::Continue | HookControl::Allow) => {}
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

                                    // The program-level choice is offered
                                    // only when a rule for it can be built, and
                                    // the popup then previews that exact rule:
                                    // the gesture is worth nothing if the user
                                    // cannot see how wide it is.
                                    let prefix_rule = PermissionManager::prefix_rule_for(
                                        stable_name,
                                        permit_prompt,
                                        &tool_use.input,
                                    );

                                    let choice = if let Some(tx) = &self.runtime.ui_tx {
                                        if let Some(rule) = &prefix_rule {
                                            prompt.push_str(&format!(
                                                "\n\n\"Always allow this pattern\" would \
                                                 record: {}",
                                                rule.to_rule_string()
                                            ));
                                        }
                                        let options = permission_options(prefix_rule.is_some());
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
                                        permission_choice_for(selection)
                                    } else {
                                        let approved = self
                                            .runtime
                                            .permission_manager
                                            .ask_user(stable_name, risk)?;
                                        if approved {
                                            PermissionChoice::AllowOnce
                                        } else {
                                            PermissionChoice::Deny
                                        }
                                    };
                                    match choice {
                                        PermissionChoice::AllowOnce => {
                                            permission_label = Some("Allow once".to_string());
                                            PreparedState::Run
                                        }
                                        PermissionChoice::AllowForSession => {
                                            permission_label =
                                                Some("Allow for this session".to_string());
                                            let outcome = self
                                                .runtime
                                                .permission_manager
                                                .allow_tool_for_session(
                                                    stable_name,
                                                    permit_prompt,
                                                    &tool_use.input,
                                                );
                                            if !outcome.is_recorded() {
                                                self.report_unrecorded_approval(stable_name);
                                            }
                                            PreparedState::Run
                                        }
                                        PermissionChoice::AlwaysAllowProgram => {
                                            permission_label =
                                                Some("Always allow this pattern".to_string());
                                            let outcome = self
                                                .runtime
                                                .permission_manager
                                                .allow_tool_as_prefix(
                                                    stable_name,
                                                    permit_prompt,
                                                    &tool_use.input,
                                                );
                                            if !outcome.is_recorded() {
                                                self.report_unrecorded_approval(stable_name);
                                            }
                                            PreparedState::Run
                                        }
                                        PermissionChoice::AlwaysAllow => {
                                            permission_label =
                                                Some("Always allow this tool".to_string());
                                            let outcome = self
                                                .runtime
                                                .permission_manager
                                                .allow_tool_with_input(
                                                    stable_name,
                                                    permit_prompt,
                                                    &tool_use.input,
                                                );
                                            if !outcome.is_recorded() {
                                                self.report_unrecorded_approval(stable_name);
                                            }
                                            PreparedState::Run
                                        }
                                        PermissionChoice::Deny => {
                                            let msg = format!(
                                                "Permission denied by user for {}",
                                                stable_name
                                            );
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
                    }
                }
                // A hook that allows skips the permission check entirely: the
                // decision was already made, and asking anyway would defeat the
                // point of the hook.
                Ok(HookControl::Allow) => {
                    permission_label = Some("Allowed by PreToolUse hook".to_string());
                    PreparedState::Run
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
                let resource_tool = match &prep.resolved {
                    ResolvedTool::McpResource { tool } => Some(*tool),
                    _ => None,
                };
                let prompt_tool = match &prep.resolved {
                    ResolvedTool::McpPrompt { tool } => Some(*tool),
                    _ => None,
                };
                let output_policy = match &prep.resolved {
                    ResolvedTool::Native { metadata } => metadata.output,
                    _ => OutputPolicy::PersistLargeOutput,
                };
                // Live-output redaction is decided per call, the same way the
                // final result's level is: a sensitive target gets the full
                // treatment, everything else keeps the high-confidence rules.
                let stream_redaction = {
                    let redaction = self.runtime.permission_manager.security_config().redaction;
                    let (sensitive, target) = match &prep.resolved {
                        ResolvedTool::Native { metadata } => (
                            metadata
                                .permission
                                .sensitive(&prep.input, &self.runtime.security)
                                .is_some(),
                            metadata.permission.target(&prep.input),
                        ),
                        _ => (false, None),
                    };
                    crate::security::redact::level_for_call(
                        &redaction,
                        sensitive,
                        target.as_deref(),
                    )
                };
                futures.push(async move {
                    let start = std::time::Instant::now();
                    let exec = if let Some(tool) = resource_tool {
                        run_mcp_resource_tool(mcp, tool, &prep.input).await
                    } else if let Some(tool) = prompt_tool {
                        run_mcp_prompt_tool(mcp, tool, &prep.input).await
                    } else if is_mcp {
                        run_mcp_tool(mcp, ctx, &prep.id, &prep.name, &prep.input).await
                    } else {
                        run_native_tool(
                            tools,
                            ctx,
                            &prep.id,
                            &prep.name,
                            &prep.input,
                            output_policy,
                            stream_redaction,
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

                // ── Redaction ───────────────────────────────────────────────
                //
                // The single choke point for the *result* text. Redacting here,
                // before anything reads it, means the PostToolUse hook, the TUI
                // step detail, the transcript and the session store all see the
                // same redacted string — there is no second path to keep in
                // sync. Deliberately not applied to `prep.input` / `arg_full`:
                // the model's own `tool_use` block has to round-trip
                // byte-identical or the next request is malformed.
                let mut exec = exec;
                let redaction = self.runtime.permission_manager.security_config().redaction;
                let (call_is_sensitive, target) = match &prep.resolved {
                    ResolvedTool::Native { metadata } => (
                        metadata
                            .permission
                            .sensitive(&prep.input, &self.runtime.security)
                            .is_some(),
                        metadata.permission.target(&prep.input),
                    ),
                    _ => (false, None),
                };
                let level = crate::security::redact::level_for_call(
                    &redaction,
                    call_is_sensitive,
                    target.as_deref(),
                );
                if level != crate::security::RedactionLevel::Off {
                    exec.content = crate::security::redact::redact(
                        &exec.content,
                        level,
                        &redaction.extra_patterns,
                    )
                    .into_owned();
                }

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
                    Ok(HookControl::Continue | HookControl::Allow) => {
                        (tool_result.content, exec_status)
                    }
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
                        Ok(HookControl::Continue | HookControl::Allow) => {}
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

    /// Stub out every `ToolUse` in `content` that `prepared` has not answered
    /// yet, each carrying `reason`.
    ///
    /// `prepared` holds one entry per *tool use* already handled, never per
    /// content block, so what is left is the tail of the content's **tool
    /// uses** — not the tail of `content`, which normally opens with the
    /// assistant's text (or thinking) before its first call. Skipping by block
    /// index would re-stub an already-handled call and answer its id twice.
    fn append_unexecuted_tool_uses(
        &mut self,
        prepared: &mut Vec<PreparedTool>,
        content: &[ContentBlock],
        reason: &str,
    ) {
        for (id, name, input) in content
            .iter()
            .filter_map(|block| match block {
                ContentBlock::ToolUse { id, name, input } => Some((id, name, input)),
                _ => None,
            })
            .skip(prepared.len())
        {
            let step_idx = self.next_step_idx();
            self.emit_update(AgentUpdate::StepFailed {
                idx: step_idx,
                tool_id: id.clone(),
                arg_summary: String::new(),
                error: reason.to_string(),
            });
            prepared.push(PreparedTool {
                id: id.clone(),
                name: name.clone(),
                input: input.clone(),
                step_idx,
                permission_label: None,
                state: PreparedState::Resolved(reason.to_string()),
                resolved: ResolvedTool::Unknown { name: name.clone() },
                task_before: None,
            });
        }
    }

    /// Results for every `ToolUse` in `content`, all carrying `reason`, plus the
    /// matching `StepFailed` UI events.
    ///
    /// Used when a turn ends *after* its assistant message has been persisted but
    /// *without* executing the tools it asked for. Every `ToolUse` needs a
    /// matching `ToolResult` or the stored history is invalid for providers that
    /// enforce pairing (Anthropic 400s on `tool_use` without `tool_result`; a
    /// Responses baseline keeps a `function_call` with no output), and the shape
    /// survives a resume. A cancel during execution already produces these stubs.
    pub(super) fn unexecuted_tool_results(
        &mut self,
        content: &[ContentBlock],
        reason: &str,
    ) -> Vec<ContentBlock> {
        let mut prepared: Vec<PreparedTool> = Vec::new();
        self.append_unexecuted_tool_uses(&mut prepared, content, reason);
        build_tool_results(prepared, Vec::new())
    }

    /// Tell the user that an "always allow"-family choice approved only the
    /// call it was made on, because no rule narrower than the whole tool could
    /// be built for it.
    ///
    /// The gesture promised to be remembered; saying so when it cannot be is
    /// the difference between a limitation and a bug.
    fn report_unrecorded_approval(&self, tool_name: &str) {
        self.emit_update(AgentUpdate::Info(format!(
            "Approved {tool_name} for this call only — no rule could be narrowed for it, and a \
             tool-wide rule would allow calls you were not asked about."
        )));
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::mcp::{McpClient, McpResourceTool, MockMcpService};
    use tact_protocol::StepStatus;

    /// A router with one server that publishes one readable resource.
    fn router_with_a_resource() -> MCPToolRouter {
        let service = MockMcpService::new(Vec::new(), |_| {
            Ok(rmcp::model::CallToolResult::success(Vec::new()))
        })
        .with_text_resource("memory://guide", "Guide", "Read me first.");
        let mut router = MCPToolRouter::new();
        router.register_client(McpClient::with_service("bm", Vec::new(), Arc::new(service)));
        router
    }

    // ── Sensitive-target guard ──────────────────────────────────────────

    /// An agent with the given permission mode and **no** UI channel, which
    /// makes an `Ask` resolve through the non-interactive path (`ask_user`
    /// denies `High`, allows `Write`) — exactly what a headless run does.
    /// `name` must be unique per test: `test_context` keys its scratch
    /// directory on it, and tests in this module run in parallel.
    fn agent_with(name: &str, mode: crate::permission::PermissionMode) -> Agent {
        agent_with_settings(name, mode, None)
    }

    /// As [`agent_with`], optionally with a settings file so a persisted `allow`
    /// rule can be put in the guard's way.
    fn agent_with_settings(
        name: &str,
        mode: crate::permission::PermissionMode,
        settings_json: Option<&str>,
    ) -> Agent {
        crate::config::test_support::install_default();
        let context = crate::tool::test_support::test_context(name);
        let manager = match settings_json {
            Some(json) => {
                let dir = std::env::temp_dir().join(format!("tact-sensitive-guard-{name}"));
                let _ = std::fs::create_dir_all(dir.join(".tact"));
                let path = dir.join(".tact/settings.json");
                std::fs::write(&path, json).unwrap();
                let settings =
                    crate::permission::settings::PermissionSettings::load_from(&path, None);
                crate::permission::PermissionManager::try_new_with_settings(mode, settings).unwrap()
            }
            None => crate::permission::PermissionManager::try_new(mode).unwrap(),
        };
        Agent::new(
            tact_llm::LlmProvider::Mock(tact_llm::MockClient::new(vec![])),
            context,
            crate::tool::toolset(),
            crate::mcp::MCPToolRouter::new(),
            manager,
            crate::agent::AgentSystemPrompt::Static("You are a test agent.".to_string()),
        )
    }

    /// Drive one async call from a sync test.
    ///
    /// `test_context` builds its own runtime on a scoped thread, so a test must
    /// not already be inside one; this mirrors that arrangement rather than
    /// making every test `#[tokio::test]`, which would panic in `test_context`.
    fn block_on<F: std::future::Future>(future: F) -> F::Output {
        tokio::runtime::Runtime::new()
            .expect("failed to create a tokio runtime")
            .block_on(future)
    }

    fn tool_use(name: &str, input: serde_json::Value) -> Vec<ContentBlock> {
        vec![ContentBlock::ToolUse {
            id: "t1".to_string(),
            name: name.to_string(),
            input,
        }]
    }

    /// What pre-flight decided for one call: `run`, or the message it resolved
    /// to instead.
    fn outcome(agent: &mut Agent, name: &str, input: serde_json::Value) -> String {
        let preflight = block_on(agent.preflight_tool_calls(&tool_use(name, input))).unwrap();
        preflight
            .prepared
            .into_iter()
            .map(|p| match p.state {
                PreparedState::Run => "run".to_string(),
                PreparedState::Resolved(msg) => msg,
            })
            .next()
            .unwrap_or_else(|| "none".to_string())
    }

    // ── Permission popup choices ────────────────────────────────────────

    /// The popup's indices are load-bearing — `request_select` answers with a
    /// position — so the mapping is pinned rather than inferred from the list.
    #[test]
    fn permission_popup_indices_map_to_their_choices() {
        assert_eq!(permission_choice_for(Some(0)), PermissionChoice::AllowOnce);
        assert_eq!(permission_choice_for(Some(1)), PermissionChoice::Deny);
        assert_eq!(
            permission_choice_for(Some(2)),
            PermissionChoice::AlwaysAllow
        );
        assert_eq!(
            permission_choice_for(Some(3)),
            PermissionChoice::AllowForSession
        );
        assert_eq!(
            permission_choice_for(Some(4)),
            PermissionChoice::AlwaysAllowProgram
        );

        // A cancelled popup arrives as `None`, and an index past the end can
        // only come from a UI that disagrees with the list. Both deny.
        assert_eq!(permission_choice_for(None), PermissionChoice::Deny);
        assert_eq!(permission_choice_for(Some(99)), PermissionChoice::Deny);
    }

    /// The program-level choice is appended, never inserted, so gating it out
    /// cannot move any index a user has learned.
    #[test]
    fn the_pattern_choice_is_appended_and_can_be_withheld() {
        let offered = permission_options(true);
        let withheld = permission_options(false);

        assert_eq!(offered.len(), PERMISSION_OPTIONS.len());
        assert_eq!(withheld.len(), PERMISSION_OPTIONS.len() - 1);
        assert_eq!(withheld, offered[..withheld.len()].to_vec());
        assert_eq!(
            offered.last().map(String::as_str),
            Some("Always allow this pattern")
        );

        // With the option withheld, no index the popup can produce reaches it —
        // which is what makes offering it conditionally safe.
        for index in 0..withheld.len() {
            assert_ne!(
                permission_choice_for(Some(index)),
                PermissionChoice::AlwaysAllowProgram,
                "index {index} is reachable without the program option"
            );
        }
        assert_eq!(
            permission_choice_for(Some(withheld.len())),
            PermissionChoice::AlwaysAllowProgram,
            "the withheld index is the one the option occupies when offered"
        );
    }

    /// The list and the mapping must stay in step: every option index maps back
    /// to the choice the label advertises, and exactly one of them denies.
    #[test]
    fn every_permission_option_maps_back_to_its_choice() {
        let expected = [
            PermissionChoice::AllowOnce,
            PermissionChoice::Deny,
            PermissionChoice::AlwaysAllow,
            PermissionChoice::AllowForSession,
            PermissionChoice::AlwaysAllowProgram,
        ];
        assert_eq!(PERMISSION_OPTIONS.len(), expected.len());
        for (index, choice) in expected.iter().enumerate() {
            assert_eq!(
                permission_choice_for(Some(index)),
                *choice,
                "option {index} ({}) does not round-trip",
                PERMISSION_OPTIONS[index]
            );
        }
    }

    /// The refusal text, when pre-flight refused rather than denied.
    fn refusal(agent: &mut Agent, name: &str, input: serde_json::Value) -> Option<String> {
        let text = outcome(agent, name, input);
        text.starts_with("Refused:").then_some(text)
    }

    /// The point of the `Credential` tier: it is decided before the mode, so
    /// `Auto` — which skips every prompt — still cannot reach a private key.
    #[test]
    fn a_private_key_is_refused_even_in_auto_mode() {
        let mut agent = agent_with("refused_in_auto", crate::permission::PermissionMode::Auto);
        let text = refusal(
            &mut agent,
            "bash",
            serde_json::json!({ "command": "cat ~/.ssh/id_ed25519" }),
        )
        .expect("a private-key read must be refused");
        assert!(text.contains("credential material"), "{text}");
        assert!(text.contains("private-key"), "{text}");
        assert!(text.contains("sensitive_paths.allow"), "{text}");
    }

    #[test]
    fn netrc_is_refused_too() {
        let mut agent = agent_with("netrc_refused", crate::permission::PermissionMode::Auto);
        let text = refusal(
            &mut agent,
            "bash",
            serde_json::json!({ "command": "cat ~/.netrc" }),
        )
        .expect("a .netrc read must be refused");
        assert!(text.contains("~/.netrc"), "{text}");
        assert!(text.contains("credential-store"), "{text}");
    }

    /// A settings `allow` rule is consulted *after* the guard, so it cannot
    /// lift a refusal — which is what makes the refusal a refusal.
    #[test]
    fn a_persisted_allow_rule_cannot_lift_a_credential_refusal() {
        let mut agent = agent_with_settings(
            "allow_rule_cannot_lift",
            crate::permission::PermissionMode::Auto,
            Some(r#"{"permissions": {"allow": ["bash"]}}"#),
        );
        let text = refusal(
            &mut agent,
            "bash",
            serde_json::json!({ "command": "cat ~/.ssh/id_ed25519" }),
        )
        .expect("a bare `bash` allow rule must not reach a private key");
        assert!(text.contains("id_ed25519"), "{text}");
    }

    /// The documented escape hatch: `sensitive_paths.allow`, which takes a file
    /// edit rather than a button.
    #[test]
    fn a_sensitive_paths_allow_entry_is_the_escape() {
        let mut agent = agent_with_settings(
            "sensitive_allow_escape",
            crate::permission::PermissionMode::Auto,
            Some(r#"{"permissions": {"sensitive_paths": {"allow": ["~/.ssh/config"]}}}"#),
        );
        assert_eq!(
            outcome(
                &mut agent,
                "bash",
                serde_json::json!({ "command": "cat ~/.ssh/config" })
            ),
            "run",
            "an explicitly allowed path runs, and Auto mode does not prompt"
        );
    }

    /// A `Secret`-tier path is not refused — it escalates to `High`, so the
    /// ordinary ladder applies and headless denies it.
    #[test]
    fn an_env_file_is_denied_headlessly_but_is_not_credential() {
        let mut agent = agent_with("env_headless", crate::permission::PermissionMode::Default);
        let text = outcome(
            &mut agent,
            "read_file",
            serde_json::json!({ "path": ".env" }),
        );
        assert!(
            text.contains("Permission denied"),
            "expected the high-risk denial, got: {text}"
        );
        assert!(
            !text.contains("credential material"),
            ".env is Secret, not Credential: {text}"
        );
    }

    #[test]
    fn plan_mode_denies_reading_an_env_file() {
        let mut agent = agent_with("env_plan", crate::permission::PermissionMode::Plan);
        let text = outcome(
            &mut agent,
            "read_file",
            serde_json::json!({ "path": ".env" }),
        );
        assert!(text.contains("Plan mode"), "{text}");
    }

    #[test]
    fn auto_mode_still_allows_an_env_file() {
        let mut agent = agent_with("env_auto", crate::permission::PermissionMode::Auto);
        assert_eq!(
            outcome(
                &mut agent,
                "read_file",
                serde_json::json!({ "path": ".env" })
            ),
            "run",
            "Auto mode is unchanged for the Secret tier"
        );
    }

    #[test]
    fn ordinary_source_still_runs_without_a_prompt() {
        let mut agent = agent_with(
            "ordinary_source",
            crate::permission::PermissionMode::Default,
        );
        assert_eq!(
            outcome(
                &mut agent,
                "read_file",
                serde_json::json!({ "path": "src/main.rs" })
            ),
            "run"
        );
    }

    /// The file tools cannot reach a `~`-rooted or absolute path anyway
    /// (`safe_path` refuses them), so those must not cost a prompt: the tool's
    /// own error is the right answer, and a popup before it is noise.
    #[test]
    fn a_home_path_through_a_file_tool_is_left_to_safe_path() {
        let mut agent = agent_with(
            "home_path_via_file_tool",
            crate::permission::PermissionMode::Default,
        );
        assert_eq!(
            outcome(
                &mut agent,
                "read_file",
                serde_json::json!({ "path": "~/.ssh/id_ed25519" })
            ),
            "run",
            "no prompt before an inevitable safe_path error"
        );
    }

    /// The guard narrows the read-only shell safelist; it does not remove it.
    /// Plan mode still runs inspection commands.
    #[test]
    fn the_guard_does_not_disable_read_only_shell_classification() {
        let mut agent = agent_with(
            "readonly_shell_intact",
            crate::permission::PermissionMode::Plan,
        );
        assert_eq!(
            outcome(
                &mut agent,
                "bash",
                serde_json::json!({ "command": "ls -la" })
            ),
            "run"
        );
    }

    /// The guard is a heuristic; this is the backstop. A token printed by a
    /// command — the shape that leaked in the first place — must not reach the
    /// tool result the conversation and the session store are built from.
    #[test]
    fn a_token_in_command_output_never_reaches_the_tool_result() {
        let mut agent = agent_with(
            "redact_bash_output",
            crate::permission::PermissionMode::Default,
        );
        let (blocks, _) = block_on(agent.execute_tool_call(&tool_use(
            "bash",
            serde_json::json!({ "command": "echo sk-7325c3231cef402d8481c32a49c4898a" }),
        )))
        .unwrap();

        let text = tool_result_text(&blocks);
        assert!(text.contains("[redacted:api-key]"), "{text}");
        assert!(!text.contains("sk-7325c3231cef"), "{text}");
    }

    /// An in-workspace `.env` is `Secret`-tier, so the call is classified
    /// sensitive and the structural rules run: the value goes, the key name and
    /// the non-secret lines stay.
    #[test]
    fn an_env_read_keeps_its_shape_but_not_its_values() {
        let mut agent = agent_with("redact_env_read", crate::permission::PermissionMode::Auto);
        let env_path = agent.tool_context.work_dir.join(".env");
        std::fs::write(&env_path, "API_KEY=abc123secret\nPORT=8080\n").unwrap();

        let (blocks, _) = block_on(agent.execute_tool_call(&tool_use(
            "read_file",
            serde_json::json!({ "path": ".env" }),
        )))
        .unwrap();

        let text = tool_result_text(&blocks);
        assert!(text.contains("API_KEY=[redacted:value]"), "{text}");
        assert!(!text.contains("abc123secret"), "{text}");
        assert!(
            text.contains("PORT=8080"),
            "non-secret lines survive: {text}"
        );
    }

    /// Ordinary source is untouched: the structural rules must not rewrite the
    /// user's own code, which is why they are gated on a sensitive target.
    #[test]
    fn a_source_read_is_left_alone() {
        let mut agent = agent_with(
            "redact_source_read",
            crate::permission::PermissionMode::Default,
        );
        let src = agent.tool_context.work_dir.join("main.rs");
        std::fs::write(&src, "let token = compute(x);\n").unwrap();

        let (blocks, _) = block_on(agent.execute_tool_call(&tool_use(
            "read_file",
            serde_json::json!({ "path": "main.rs" }),
        )))
        .unwrap();

        assert_eq!(tool_result_text(&blocks), "let token = compute(x);");
    }

    /// Concatenated tool-result text, for asserting on what the conversation
    /// (and therefore the session store) received.
    fn tool_result_text(blocks: &[ContentBlock]) -> String {
        blocks
            .iter()
            .filter_map(|b| match b {
                ContentBlock::ToolResult { content, .. } => Some(content.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[tokio::test]
    async fn a_resource_listing_is_a_successful_tool_result() {
        let exec = run_mcp_resource_tool(
            &router_with_a_resource(),
            McpResourceTool::List,
            &serde_json::json!({}),
        )
        .await;

        assert!(matches!(exec.status, StepStatus::Success));
        assert!(exec.content.contains("memory://guide"), "{}", exec.content);
    }

    #[tokio::test]
    async fn a_resource_read_requires_both_server_and_uri() {
        // The schema marks both required, but a model can still send the wrong
        // shape; failing with a named argument beats an "unknown server" from a
        // lookup with an empty name.
        let exec = run_mcp_resource_tool(
            &router_with_a_resource(),
            McpResourceTool::Read,
            &serde_json::json!({"uri": "memory://guide"}),
        )
        .await;
        assert!(matches!(exec.status, StepStatus::Failed));
        assert!(
            exec.content.contains("`server` is required"),
            "{}",
            exec.content
        );

        let exec = run_mcp_resource_tool(
            &router_with_a_resource(),
            McpResourceTool::Read,
            &serde_json::json!({"server": "bm"}),
        )
        .await;
        assert!(matches!(exec.status, StepStatus::Failed));
        assert!(
            exec.content.contains("`uri` is required"),
            "{}",
            exec.content
        );
    }

    #[tokio::test]
    async fn a_resource_read_returns_the_text() {
        let exec = run_mcp_resource_tool(
            &router_with_a_resource(),
            McpResourceTool::Read,
            &serde_json::json!({"server": "bm", "uri": "memory://guide"}),
        )
        .await;

        assert!(matches!(exec.status, StepStatus::Success));
        assert!(exec.content.contains("Read me first."), "{}", exec.content);
    }

    #[tokio::test]
    async fn a_template_listing_reaches_the_model() {
        // A template-addressed server enumerates nothing through
        // `resources/list`, so this is the only call that reveals it has
        // anything at all.
        let service = MockMcpService::new(Vec::new(), |_| {
            Ok(rmcp::model::CallToolResult::success(Vec::new()))
        })
        .with_resource_template("memory://{topic}", "Note by topic");
        let mut router = MCPToolRouter::new();
        router.register_client(McpClient::with_service("bm", Vec::new(), Arc::new(service)));

        let exec =
            run_mcp_resource_tool(&router, McpResourceTool::Templates, &serde_json::json!({}))
                .await;

        assert!(matches!(exec.status, StepStatus::Success));
        assert!(
            exec.content.contains("memory://{topic}"),
            "{}",
            exec.content
        );
    }

    #[test]
    fn only_the_resource_names_resolve_to_the_resource_path() {
        assert!(is_mcp_resource_tool("list_mcp_resources"));
        assert!(is_mcp_resource_tool("list_mcp_resource_templates"));
        assert!(is_mcp_resource_tool("read_mcp_resource"));
        assert!(!is_mcp_resource_tool("mcp__bm__read_note"));
        assert!(!is_mcp_resource_tool("read_file"));
    }

    /// A router whose one server publishes `getting_started`.
    fn router_with_a_prompt() -> MCPToolRouter {
        let service = MockMcpService::new(Vec::new(), |_| {
            Ok(rmcp::model::CallToolResult::success(Vec::new()))
        })
        .with_prompt(
            "getting_started",
            "Introduce the server",
            &[("topic", true)],
        )
        .with_prompt_messages(
            "getting_started",
            vec![rmcp::model::PromptMessage::new_text(
                rmcp::model::PromptMessageRole::User,
                "Start with the notes.",
            )],
        );
        let mut router = MCPToolRouter::new();
        router.register_client(McpClient::with_service("bm", Vec::new(), Arc::new(service)));
        router
    }

    #[tokio::test]
    async fn a_prompt_listing_is_a_successful_tool_result() {
        let exec = run_mcp_prompt_tool(
            &router_with_a_prompt(),
            crate::mcp::McpPromptTool::List,
            &serde_json::json!({}),
        )
        .await;

        assert!(matches!(exec.status, StepStatus::Success));
        assert!(exec.content.contains("getting_started"), "{}", exec.content);
        assert!(
            exec.content.contains("topic (required)"),
            "{}",
            exec.content
        );
    }

    #[tokio::test]
    async fn a_prompt_get_requires_both_server_and_name() {
        // The schema marks both required, but a model can still send the wrong
        // shape; failing with a named argument beats an "unknown server" from a
        // lookup with an empty name.
        let exec = run_mcp_prompt_tool(
            &router_with_a_prompt(),
            crate::mcp::McpPromptTool::Get,
            &serde_json::json!({"name": "getting_started"}),
        )
        .await;
        assert!(matches!(exec.status, StepStatus::Failed));
        assert!(
            exec.content.contains("`server` is required"),
            "{}",
            exec.content
        );

        let exec = run_mcp_prompt_tool(
            &router_with_a_prompt(),
            crate::mcp::McpPromptTool::Get,
            &serde_json::json!({"server": "bm"}),
        )
        .await;
        assert!(matches!(exec.status, StepStatus::Failed));
        assert!(
            exec.content.contains("`name` is required"),
            "{}",
            exec.content
        );
    }

    #[tokio::test]
    async fn a_prompt_get_returns_the_messages() {
        let exec = run_mcp_prompt_tool(
            &router_with_a_prompt(),
            crate::mcp::McpPromptTool::Get,
            &serde_json::json!({"server": "bm", "name": "getting_started"}),
        )
        .await;

        assert!(matches!(exec.status, StepStatus::Success));
        assert!(
            exec.content.contains("Start with the notes."),
            "{}",
            exec.content
        );
    }

    #[test]
    fn prompt_arguments_are_strings_and_a_bad_one_is_named() {
        // The protocol's prompt arguments are a string map. A number or a bool
        // is stringified — a model that writes `{"count": 3}` means `"3"` — but
        // anything else is refused by name rather than dropped, because a
        // silently omitted argument reaches the server as an unfilled
        // placeholder and still reports success.
        let ok = prompt_arguments(&serde_json::json!({
            "arguments": {
                "topic": "notes",
                "count": 3,
                "deep": true
            }
        }))
        .unwrap()
        .expect("a non-empty object is arguments");
        assert_eq!(ok.get("topic").unwrap(), &serde_json::json!("notes"));
        assert_eq!(ok.get("count").unwrap(), &serde_json::json!("3"));
        assert_eq!(ok.get("deep").unwrap(), &serde_json::json!("true"));

        // Absent or empty is `None`, not an empty map: the two are the same
        // request, and the caller passes `None` through to the server.
        assert_eq!(prompt_arguments(&serde_json::json!({})).unwrap(), None);
        assert_eq!(
            prompt_arguments(&serde_json::json!({"arguments": {}})).unwrap(),
            None
        );

        let error = prompt_arguments(&serde_json::json!({"arguments": {"topic": ["a", "b"]}}))
            .expect_err("an array is not a string");
        assert!(error.contains("topic"), "{error}");
    }

    #[test]
    fn only_the_prompt_names_resolve_to_the_prompt_path() {
        assert!(is_mcp_prompt_tool("list_mcp_prompts"));
        assert!(is_mcp_prompt_tool("get_mcp_prompt"));
        assert!(!is_mcp_prompt_tool("mcp__bm__read_note"));
        assert!(!is_mcp_prompt_tool("read_mcp_resource"));
    }

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
    fn id_policy_reads_the_field_and_tolerates_absence() {
        // `wait_background` / `check_background`: the id is the whole readable
        // parameter, and both arguments are optional, so an omitted id must come
        // back as `""` (the renderer then draws the bare label) rather than as
        // the serialized `{}` dump.
        let policy = ArgumentSummaryPolicy::Id { field: "task_id" };
        assert_eq!(
            tool_arg_full(policy, &serde_json::json!({"task_id": "abc123"})),
            "abc123"
        );
        assert_eq!(
            tool_arg_full(
                policy,
                &serde_json::json!({"task_id": "abc123", "timeout_ms": 300_000})
            ),
            "abc123"
        );
        assert_eq!(tool_arg_full(policy, &serde_json::json!({})), "");
        assert_eq!(
            tool_arg_full(policy, &serde_json::json!({"timeout_ms": 300_000})),
            ""
        );
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
    fn background_tools_title_shows_the_id_not_the_input_dump() {
        // Both take an optional `task_id`, so their parameter must be that id —
        // never the serialized input, which the header row would print verbatim.
        let router = crate::tool::toolset();
        for tool in ["check_background", "wait_background"] {
            let metadata = router.resolve(tool).unwrap().metadata();
            let resolved = ResolvedTool::Native { metadata };
            let named = arg_pair(
                &resolved,
                &serde_json::json!({"task_id": "abc123"}),
                None,
                None,
            );
            assert_eq!(named.1, "abc123", "{tool}");
            let listing = arg_pair(&resolved, &serde_json::json!({}), None, None);
            assert_eq!(listing.1, "", "{tool} must keep its bare label");
        }
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
