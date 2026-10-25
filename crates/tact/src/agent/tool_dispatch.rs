//! Tool-call dispatch: pre-flight, parallel execution, and result assembly.
//!
//! After Task 5, all semantic decisions flow through typed metadata instead of
//! matching native tool-name strings.

use anyhow::Result;
use futures_util::{StreamExt, stream::FuturesUnordered};
use tact_kernel::tool::ToolImage;
use tact_llm::{ContentBlock, ImageSource};
use tact_protocol::{AgentUpdate, StepResult, StepStatus, ToolPresentationInfo};

use super::Agent;
use crate::{
    compact::{persist_large_output, persist_large_output_over_tokens},
    hook::{HookControl, NotificationContext, ToolResult, ToolUse},
    invoke_hooks,
    permission::policy_ext::PermissionPolicyExt,
    tool::{
        ArgumentSummaryPolicy, DetailPolicy, GoalYieldReason, OutputPolicy, TaskOperation,
        ToolDomain, ToolEffect,
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
    Unknown,
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
    /// Present for a native tool that went through the kernel pipeline. Its
    /// one-use ticket is what phase 2 redeems, so nothing can call a handler
    /// that was never authorized.
    kernel: Option<tact_kernel::tool::executor::ToolPreflight>,
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

fn to_image_source(image: ToolImage) -> ImageSource {
    ImageSource {
        type_: "base64".to_string(),
        media_type: image.mime_type,
        data: image.data,
    }
}

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
            blocks.push(ContentBlock::Image {
                source: to_image_source(image),
            });
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

#[derive(Debug, Default)]
pub(crate) struct ToolExecutionOutcome {
    /// Focus string for a manual compaction requested by a `compact` tool in
    /// this dispatch, if any.
    pub manual_compact: Option<String>,
    /// Goal yield requested by a tool in this dispatch, if any.
    pub goal_yield: Option<GoalYieldReason>,
    /// Number of tool calls that actually ran and produced a real
    /// [`ExecResult`].
    ///
    /// Deliberately NOT counted: preflight-denied calls, hook-blocked calls
    /// that never executed, and wave-skipped calls that carry a
    /// `Not executed: ...` message (for example the calls skipped after a Goal
    /// yield).
    pub executed_tool_calls: u64,
}

struct ExecResult {
    content: String,
    status: StepStatus,
    image: Option<ToolImage>,
    effects: Vec<ToolEffect>,
}

/// Run a kernel-authorized native call and shape its result for the transcript.
///
/// The output policy, image and effects are applied here rather than in the
/// kernel: persisting a large output is a session-store concern, and the kernel
/// deliberately owns no store.
async fn run_kernel_tool(
    executor: std::sync::Arc<tact_kernel::tool::executor::ToolExecutor>,
    prepared: tact_kernel::tool::executor::ToolPreflight,
    work_dir: std::path::PathBuf,
    tool_use_id: String,
    output_policy: OutputPolicy,
    output_token_limit: Option<usize>,
) -> ExecResult {
    let name = prepared.tool_name().to_string();
    // An MCP failure names the transport, not just the tool: a reader needs to
    // know whether Tact's own handler failed or a server did. The wording is a
    // user-visible contract that predates the pipeline, so it is preserved here
    // rather than flattened into one message.
    let prefix = if name.starts_with("mcp__") {
        "Error invoking MCP tool"
    } else {
        "Error invoking tool"
    };
    match executor.execute(prepared).await {
        Ok(result) => {
            let tact_path = crate::consts::TactPath::new(&work_dir);
            match output_policy {
                OutputPolicy::PersistLargeOutput => {
                    // A source that declares a per-tool budget (an MCP entry's
                    // `tools.<name>.output_token_limit`) spills on its own
                    // threshold; without one the session-wide rule applies.
                    let persisted = match output_token_limit {
                        Some(limit) => {
                            persist_large_output_over_tokens(
                                &tact_path,
                                &tool_use_id,
                                &result.content,
                                limit,
                            )
                            .await
                        }
                        None => {
                            persist_large_output(&tact_path, &tool_use_id, &result.content).await
                        }
                    };
                    match persisted {
                        Ok(content) => ExecResult {
                            content,
                            status: StepStatus::Success,
                            image: result.image,
                            effects: result.effects,
                        },
                        Err(error) => ExecResult {
                            content: format!("Error persisting large output: {error}"),
                            status: StepStatus::Failed,
                            image: None,
                            effects: Vec::new(),
                        },
                    }
                }
                OutputPolicy::KeepInline => ExecResult {
                    content: result.content,
                    status: StepStatus::Success,
                    image: result.image,
                    effects: result.effects,
                },
            }
        }
        Err(error) => ExecResult {
            content: format!("{prefix} {name}: {error}"),
            status: StepStatus::Failed,
            image: None,
            effects: Vec::new(),
        },
    }
}

/// Whether `name` is one of Tact's own MCP resource tools.
///
/// A free function, not a lookup in the catalog: the *resolver* needs it before
/// the catalog is consulted, to decide which family a name belongs to.
fn is_mcp_resource_tool(name: &str) -> bool {
    crate::mcp::McpResourceTool::from_name(name).is_some()
}

/// Whether `name` is one of Tact's own MCP prompt tools.
fn is_mcp_prompt_tool(name: &str) -> bool {
    crate::mcp::McpPromptTool::from_name(name).is_some()
}

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
        ResolvedTool::Unknown => super::tool_schedule::ToolResources::barrier(),
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

impl Agent {
    /// The generation of the descriptor set the model was last shown.
    ///
    /// The kernel compares it before touching a handler, so a tool appearing or
    /// disappearing between the request and the call is a refusal instead of a
    /// dispatch to something the model never saw.
    /// The generation the tool list the model is holding came from.
    ///
    /// Read from the cache stamp, not from a fresh snapshot: a snapshot rebuilt
    /// here would report the *current* generation, so the comparison in
    /// `resolve` could never fail.
    fn advertised_generation(&self) -> u64 {
        self.advertised_generation
    }

    /// The run-scoped context the kernel pipeline hands a tool.
    ///
    /// `metadata` and `input` decide only the redaction level: the same
    /// per-call question the result pass asks, so live progress and the final
    /// result cannot disagree about how much to hide.
    fn kernel_tool_context(
        &self,
        tool_use_id: &str,
        metadata: Option<&crate::tool::ToolMetadata>,
        input: &serde_json::Value,
    ) -> tact_kernel::tool::context::ToolCallContext {
        use tact_kernel::tool::context::{
            CancelToken, EventSink, ToolCallContext, ToolEvent, ToolOrigin, Workspace,
        };

        struct AgentEvents;
        impl EventSink for AgentEvents {
            fn emit(&self, _event: ToolEvent) {}
        }

        // The token wraps the agent's live flag: a cancel that arrives while the
        // tool is running has to be visible to it, which a private copy could
        // not see.
        let cancel = CancelToken::from_shared(self.tool_context.cancel_flag.clone());
        let redaction = {
            let config = self.runtime.permission_manager.security_config().redaction;
            // A source-provided tool has no static metadata, so there is no
            // declared target to classify: its results still get the configured
            // level, they just cannot be escalated per call.
            let (sensitive, target) = match metadata {
                Some(metadata) => (
                    metadata
                        .permission
                        .sensitive(input, &self.runtime.security)
                        .is_some(),
                    metadata.permission.target(input),
                ),
                None => (false, None),
            };
            crate::security::redact::level_for_call(&config, sensitive, target.as_deref())
        };
        ToolCallContext {
            run_id: self.runtime.session_id.clone().unwrap_or_default(),
            tool_use_id: tool_use_id.to_string(),
            session_id: self.runtime.session_id.clone(),
            active_goal_id: self.tool_context.active_goal_id.clone(),
            origin: if self.tool_context.is_subagent {
                ToolOrigin::Subagent
            } else {
                ToolOrigin::Main
            },
            workspace: Workspace::new(self.tool_context.work_dir.clone()),
            events: std::sync::Arc::new(AgentEvents),
            interaction: std::sync::Arc::new(crate::permission::UiInteractionBroker::from_context(
                &self.tool_context,
            )),
            cancel,
            redaction: match redaction {
                crate::security::RedactionLevel::Off => {
                    tact_kernel::tool::context::RedactionPolicy::Off
                }
                crate::security::RedactionLevel::Basic => {
                    tact_kernel::tool::context::RedactionPolicy::Basic
                }
                crate::security::RedactionLevel::Credential => {
                    tact_kernel::tool::context::RedactionPolicy::Credential
                }
            },
        }
    }

    /// The label the UI shows for a kernel-authorized call.
    fn authorization_label(&self, prepared: &tact_kernel::tool::executor::ToolPreflight) -> String {
        use tact_kernel::tool::executor::PreflightAuthorization;
        match self.tool_executor.authorization(prepared) {
            PreflightAuthorization::HookAllowed => "Allowed by PreToolUse hook".to_string(),
            PreflightAuthorization::PermissionRequestHookAllowed => {
                "Allowed by PermissionRequest hook".to_string()
            }
            PreflightAuthorization::Prompted => "Allow once".to_string(),
            PreflightAuthorization::PolicyAllowed => String::new(),
        }
    }

    /// Ask the human about a kernel permission prompt.
    ///
    /// Returns the picked option (label + typed kind) or `None` when there is no
    /// UI to ask or the answer was cancelled / denied.
    async fn ask_permission(
        &self,
        prompt: &tact_kernel::tool::executor::PermissionPrompt,
        _tool_name: &str,
    ) -> Option<KernelPermissionAnswer> {
        let tx = self.runtime.events.ui()?;
        // Rendered the same way the kernel's own `ask` renders an option, so a
        // `detail` added later cannot make the popup and the recorded answer
        // describe different choices.
        let labels: Vec<String> = prompt
            .options
            .iter()
            .map(|option| match &option.detail {
                Some(detail) => format!("{} ({detail})", option.label),
                None => option.label.clone(),
            })
            .collect();
        let responder = self.tool_context.ui_responder.clone();
        let from_subagent = self.tool_context.is_subagent;
        let selection = responder
            .request_select(tx, prompt.message.clone(), labels, false, from_subagent)
            .await
            .ok()
            .flatten();
        let option = prompt.options.get(selection?)?;
        // The label names the gesture, not the option's generic text: it is what
        // the toast says afterwards.
        let label = match option.kind {
            tact_kernel::tool::executor::PermissionOptionKind::AllowOnce => "Allow once",
            tact_kernel::tool::executor::PermissionOptionKind::AlwaysAllow => {
                "Always allow this tool"
            }
            tact_kernel::tool::executor::PermissionOptionKind::AlwaysAllowWide => {
                "Always allow this pattern"
            }
            tact_kernel::tool::executor::PermissionOptionKind::Deny => return None,
        };
        Some(KernelPermissionAnswer {
            label,
            answer: tact_kernel::tool::executor::PermissionAnswer {
                option_id: option.id.clone(),
                kind: option.kind,
            },
        })
    }
}

/// The hook port the kernel pipeline would call.
///
/// The Agent keeps hooks on its own side because a hook callback takes
/// `&Agent`; the pipeline is driven through `resolve`/`authorize_policy`/
/// `finish_authorization` instead of `preflight`, so these methods are never
/// reached. Implemented rather than optional so `ToolExecutor` stays a plain
/// struct with no `Option` in the hot path.
pub(super) struct NoKernelHooks;

#[async_trait::async_trait]
impl tact_kernel::tool::executor::HookService for NoKernelHooks {
    async fn pre_tool_use(
        &self,
        _event: &mut tact_kernel::tool::executor::PreToolUseEvent,
    ) -> Result<tact_kernel::tool::executor::HookControl> {
        Ok(tact_kernel::tool::executor::HookControl::Continue)
    }

    async fn post_tool_use(
        &self,
        _event: &mut tact_kernel::tool::executor::PostToolUseEvent,
    ) -> Result<tact_kernel::tool::executor::HookControl> {
        Ok(tact_kernel::tool::executor::HookControl::Continue)
    }
}

/// A permission answer plus the label the UI should show for it.
struct KernelPermissionAnswer {
    label: &'static str,
    answer: tact_kernel::tool::executor::PermissionAnswer,
}

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
    /// Returns the assembled `ToolResult` blocks (in call order) plus typed
    /// dispatch outcomes such as manual compaction and Goal yield.
    ///
    /// Three phases: [`Agent::preflight_tool_calls`] resolves and authorizes
    /// each call sequentially, [`Agent::run_tool_waves`] executes the runnable
    /// ones in conflict-free waves, and [`build_tool_results`] reassembles the
    /// outputs back into call order.
    pub(crate) async fn execute_tool_call(
        &mut self,
        content: &[ContentBlock],
    ) -> Result<(Vec<ContentBlock>, ToolExecutionOutcome)> {
        self.runtime.goal_yield_reason = None;
        let preflight = self.preflight_tool_calls(content).await?;
        if preflight.cancelled {
            // Nothing ran. A call approved before the flag flipped still needs
            // a result — and `build_tool_results` indexes `outputs` by
            // position, so an empty vec would panic on it. Every entry is
            // answered as cancelled, which is what the wave-boundary cancel
            // already writes for a call it approved but never started.
            // Nothing will run, so release every ticket this turn minted.
            for prepared_tool in &preflight.prepared {
                if let Some(ticket) = prepared_tool.kernel.as_ref() {
                    self.tool_executor.release(ticket.clone_for_host());
                }
            }
            let outputs = (0..preflight.prepared.len()).map(|_| None).collect();
            return Ok((
                build_tool_results(preflight.prepared, outputs),
                ToolExecutionOutcome::default(),
            ));
        }
        let (outputs, outcome) = self.run_tool_waves(&preflight.prepared).await?;
        Ok((build_tool_results(preflight.prepared, outputs), outcome))
    }

    /// Phase 1 — sequential pre-flight.
    ///
    /// Each tool use is counted, resolved (native → MCP → unknown), formatted
    /// for display, and run through the `PreToolUse` hook + permission
    /// pipeline. Sequential by design: a permission prompt must be answered
    /// before the next call is resolved, and an "always allow" granted here has
    /// to apply to the calls that follow in the same turn.
    async fn preflight_tool_calls(&mut self, content: &[ContentBlock]) -> Result<Preflight> {
        // Stamp the permission snapshot and the subagent-results queue *before*
        // any tool runs: a `spawn_subagent` in this turn must inherit the
        // parent's current mode / allow-list / settings — including an "always
        // allow" granted earlier in this same turn — and a background child must
        // be able to push its summary back for re-injection.
        self.tool_context.permission_snapshot = Some(self.runtime.permission_manager.snapshot());
        self.tool_context.subagent_results = Some(self.runtime.pending_subagent_results.clone());
        // Hand the live context to every contributed tool. The factory is built
        // at construction, but `with_ui_channel` and the two stamps above all
        // happen after it — a frozen base would give a kernel-dispatched
        // `spawn_subagent` the construction-time values.
        self.tool_contexts.refresh(&self.tool_context);

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
                            resolved: ResolvedTool::Unknown,
                            task_before: None,
                            kernel: None,
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
            // ── Authorization ──────────────────────────────────────────────
            //
            // The kernel pipeline owns this order: sensitive-path guard →
            // PreToolUse hook → guard again on the rewritten input →
            // PermissionRequest hook → permission ladder → one-use ticket. The
            // Agent only keeps what needs host state: the hook callbacks
            // themselves (they take `&Agent`), the display label, and the step
            // events.
            //
            // MCP resources/prompts still use the legacy ladder below: they are
            // Tact's own tools rather than a plugin's, and their risk comes from
            // `[mcp]` settings that the descriptor does not carry yet.
            let mut permission_label: Option<String> = None;
            let mut preflight: Option<tact_kernel::tool::executor::ToolPreflight> = None;
            // Every family is a catalog entry now, so which resolver produced this
            // call only decides the *label*; all of them go through the pipeline.
            // `McpResource`/`McpPrompt` are kernel tools too (McpControlSource).
            let catalog_name: Option<&str> = match &resolved {
                ResolvedTool::Native { metadata } => Some(metadata.name),
                ResolvedTool::Mcp { full_name, .. } => Some(full_name.as_str()),
                ResolvedTool::McpResource { tool } => Some(tool.name()),
                ResolvedTool::McpPrompt { tool } => Some(tool.name()),
                // An unknown name is refused above; reaching here would mean the
                // catalog disagrees with the resolver, which the parity test
                // forbids.
                ResolvedTool::Unknown => None,
            };
            let static_metadata = match &resolved {
                ResolvedTool::Native { metadata } => Some(*metadata),
                _ => None,
            };
            let state = if let Some(name) = catalog_name {
                let metadata = static_metadata;
                // The name the loop needs for labels and hooks. A source-provided
                // tool has no static metadata, so it comes from the resolver.
                let display_name = name;
                let generation = self.advertised_generation();
                let request = tact_kernel::tool::executor::ToolRequest {
                    call_id: tool_use.id.clone(),
                    tool_name: name.to_string(),
                    input: tool_use.input.clone(),
                    snapshot_generation: generation,
                };
                let call_ctx = self.kernel_tool_context(&tool_use.id, metadata, &tool_use.input);
                match self.tool_executor.resolve(&request).await {
                    Err(error) => {
                        let msg = format!("{error}");
                        // A guard refusal names what it refused; every other
                        // resolve failure has no label to show.
                        permission_label = error
                            .downcast_ref::<tact_kernel::tool::executor::LabeledRefusal>()
                            .map(|refusal| refusal.label.clone());
                        self.emit_update(AgentUpdate::StepFailed {
                            idx: step_idx,
                            tool_id: id.clone(),
                            arg_summary: String::new(),
                            error: msg.clone(),
                        });
                        PreparedState::Resolved(msg)
                    }
                    Ok(resolved_call) => {
                        // The PreToolUse hooks run here, between resolve and
                        // authorize, because they take `&Agent`.
                        let control = match invoke_hooks!(PreToolUse, self, &mut tool_use) {
                            Ok(control) => control,
                            Err(error) => {
                                let msg = format!("PreToolUse hook failed: {error}");
                                self.emit_update(AgentUpdate::StepFailed {
                                    idx: step_idx,
                                    tool_id: id.clone(),
                                    arg_summary: String::new(),
                                    error: msg.clone(),
                                });
                                prepared.push(PreparedTool {
                                    id: tool_use.id,
                                    name: tool_use.name,
                                    input: tool_use.input,
                                    step_idx,
                                    permission_label: None,
                                    state: PreparedState::Resolved(msg),
                                    resolved,
                                    task_before: None,
                                    kernel: None,
                                });
                                continue;
                            }
                        };
                        if let HookControl::Block(reason) = &control {
                            let msg = format!("Tool blocked by PreToolUse hook: {reason}");
                            self.emit_update(AgentUpdate::StepFailed {
                                idx: step_idx,
                                tool_id: id.clone(),
                                arg_summary: String::new(),
                                error: msg.clone(),
                            });
                            prepared.push(PreparedTool {
                                id: tool_use.id,
                                name: tool_use.name,
                                input: tool_use.input,
                                step_idx,
                                permission_label: None,
                                state: PreparedState::Resolved(msg),
                                resolved,
                                task_before: None,
                                kernel: None,
                            });
                            continue;
                        }

                        let rewritten = resolved_call.with_input(tool_use.input.clone());
                        let authorized = self
                            .tool_executor
                            .authorize_policy(
                                rewritten,
                                call_ctx.clone(),
                                control == HookControl::Allow,
                            )
                            .await;
                        match authorized {
                            Err(error) => {
                                let msg = format!("{error}");
                                self.emit_update(AgentUpdate::StepFailed {
                                    idx: step_idx,
                                    tool_id: id.clone(),
                                    arg_summary: String::new(),
                                    error: msg.clone(),
                                });
                                PreparedState::Resolved(msg)
                            }
                            Ok(tact_kernel::tool::executor::AuthorizedCall::Denied(message)) => {
                                self.emit_update(AgentUpdate::StepFailed {
                                    idx: step_idx,
                                    tool_id: id.clone(),
                                    arg_summary: String::new(),
                                    error: message.clone(),
                                });
                                PreparedState::Resolved(message)
                            }
                            Ok(tact_kernel::tool::executor::AuthorizedCall::Allowed(
                                prepared_call,
                            )) => {
                                let label = self.authorization_label(&prepared_call);
                                permission_label = Some(label);
                                preflight = Some(prepared_call);
                                PreparedState::Run
                            }
                            Ok(tact_kernel::tool::executor::AuthorizedCall::NeedsPrompt {
                                call,
                                prompt,
                                request,
                            }) => {
                                // Codex's `PermissionRequest`: a policy hook may
                                // answer the prompt. `allow` skips it, `block`
                                // denies outright, anything else asks the user.
                                let hook_control =
                                    match invoke_hooks!(PermissionRequest, self, &mut tool_use) {
                                        Ok(control) => control,
                                        Err(error) => {
                                            // Fail open, like every other hook
                                            // failure: a broken policy hook must not
                                            // block work.
                                            self.emit_update(AgentUpdate::Info(format!(
                                                "[PermissionRequest hook failed] {error}"
                                            )));
                                            HookControl::Continue
                                        }
                                    };
                                match hook_control {
                                    HookControl::Allow => {
                                        match self
                                            .tool_executor
                                            .finish_authorization(
                                                call, request, None, call_ctx, true,
                                            )
                                            .await
                                        {
                                            Ok(prepared_call) => {
                                                permission_label = Some(
                                                    "Allowed by PermissionRequest hook".to_string(),
                                                );
                                                preflight = Some(prepared_call);
                                                PreparedState::Run
                                            }
                                            Err(error) => {
                                                PreparedState::Resolved(format!("{error}"))
                                            }
                                        }
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
                                        // Observational: a plugin plugs in here
                                        // to be told a prompt is about to appear,
                                        // it cannot decide anything. Fired before
                                        // every prompt, as before the cutover.
                                        let notification = NotificationContext {
                                            notification_type: "permission_prompt".to_string(),
                                            title: display_name.to_string(),
                                            message: prompt.message.clone(),
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
                                        let answer =
                                            self.ask_permission(&prompt, display_name).await;
                                        match answer {
                                            None => {
                                                let msg = format!(
                                                    "Permission denied by user for {}",
                                                    display_name
                                                );
                                                self.emit_update(AgentUpdate::StepFailed {
                                                    idx: step_idx,
                                                    tool_id: id.clone(),
                                                    arg_summary: String::new(),
                                                    error: msg.clone(),
                                                });
                                                PreparedState::Resolved(msg)
                                            }
                                            Some(answer) => {
                                                permission_label = Some(answer.label.to_string());
                                                match self
                                                    .tool_executor
                                                    .finish_authorization(
                                                        call,
                                                        request,
                                                        Some(answer.answer),
                                                        call_ctx,
                                                        false,
                                                    )
                                                    .await
                                                {
                                                    Ok(prepared_call) => {
                                                        preflight = Some(prepared_call);
                                                        PreparedState::Run
                                                    }
                                                    Err(error) => {
                                                        PreparedState::Resolved(format!("{error}"))
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            } else {
                // Every dispatchable tool is a catalog entry now, so an
                // unresolvable name is a refusal rather than a second pipeline.
                let msg = format!("unknown tool: {name}");
                self.emit_update(AgentUpdate::StepFailed {
                    idx: step_idx,
                    tool_id: id.clone(),
                    arg_summary: String::new(),
                    error: msg.clone(),
                });
                PreparedState::Resolved(msg)
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
                kernel: preflight,
            });
        }

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
    ) -> Result<(Vec<Option<ExecResult>>, ToolExecutionOutcome)> {
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
        let mut outcome = ToolExecutionOutcome::default();

        for wave in super::tool_schedule::waves_grouped(&resources) {
            if self.cancel_requested() {
                self.emit_update(AgentUpdate::Info("Cancelled by user".into()));
                return Ok((outputs, outcome));
            }
            let mut futures = FuturesUnordered::new();
            for &pos in &wave {
                let pi = run_indices[pos];
                let prep = &prepared[pi];
                // Native calls redeem their one-use ticket here, so a handler
                // can only run through a call the pipeline authorized.
                let kernel = prep.kernel.as_ref().map(|prepared_call| {
                    tact_kernel::tool::executor::ToolPreflight::clone_for_host(prepared_call)
                });
                let executor = self.tool_executor.clone();
                let work_dir = self.tool_context.work_dir.clone();
                let tool_use_id = prep.id.clone();
                let output_policy = match &prep.resolved {
                    ResolvedTool::Native { metadata } => metadata.output,
                    _ => OutputPolicy::PersistLargeOutput,
                };
                // Read from the descriptor the pipeline resolved: a source's
                // per-tool budget lives on its policy, not on the static
                // metadata a built-in has.
                let output_token_limit = prep
                    .kernel
                    .as_ref()
                    .and_then(|call| call.output_token_limit());
                futures.push(async move {
                    let start = std::time::Instant::now();
                    // Every family — native, MCP, MCP control — is a catalog entry
                    // with a ticket, so this branch is the only execution path.
                    let exec = if let Some(prepared_call) = kernel {
                        run_kernel_tool(
                            executor,
                            prepared_call,
                            work_dir.clone(),
                            tool_use_id.clone(),
                            output_policy,
                            output_token_limit,
                        )
                        .await
                    } else {
                        // A native tool that reached `Run` without a ticket would
                        // be a bug in preflight, not a call to make: dispatching
                        // it here is exactly what the ticket exists to prevent.
                        ExecResult {
                            content: format!(
                                "Error invoking tool {}: it was not authorized",
                                prep.name
                            ),
                            status: StepStatus::Failed,
                            image: None,
                            effects: Vec::new(),
                        }
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
                let exec_effects = exec.effects;
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
                let effects = if succeeded { exec_effects } else { Vec::new() };

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
                    outcome.manual_compact = prep_input
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
                if let Some(reason) = effects.iter().find_map(|effect| match effect {
                    ToolEffect::YieldTurn { reason } => Some(*reason),
                    _ => None,
                }) {
                    outcome.goal_yield = Some(reason);
                }
                // This call reached the executor and produced a real result:
                // it counts toward `executed_tool_calls`. The wave-skipped
                // ("Not executed: ...") entries written below never reach here.
                outcome.executed_tool_calls += 1;
                outputs[pi] = Some(ExecResult {
                    content: exec_output,
                    status: final_status,
                    image: exec_image,
                    effects,
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

            if outcome.goal_yield == Some(GoalYieldReason::NeedsUser) {
                // Goal request tools are Barriers, so a yielding wave cannot
                // contain another call. Later waves are deliberately not run.
                let wave_end = wave.iter().copied().max().unwrap_or(usize::MAX);
                for &pi in run_indices.iter().skip(wave_end.saturating_add(1)) {
                    if outputs[pi].is_none() {
                        outputs[pi] = Some(ExecResult {
                            content: "Not executed: Goal requested user input".to_string(),
                            status: StepStatus::Failed,
                            image: None,
                            effects: Vec::new(),
                        });
                    }
                }
                break;
            }
        }

        Ok((outputs, outcome))
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
                resolved: ResolvedTool::Unknown,
                task_before: None,
                kernel: None,
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
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::mcp::{MCPToolRouter, McpClient, McpResourceTool, MockMcpService};
    use crate::tool::McpControl;
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

    #[test]
    fn goal_yield_stops_later_tool_waves() {
        let (blocks, outcome, marker) = block_on(async {
            crate::config::test_support::install_default();
            let mut context = crate::tool::test_support::test_context("dispatch_goal_yield");
            let goal = context
                .goal_manager
                .create(crate::goal::GoalCreate {
                    title: "dispatch goal yield".into(),
                    objective: "pause for user input".into(),
                    criteria: vec![crate::goal::GoalCriterion::required_with_id(
                        "criterion-1",
                        "input received",
                    )],
                })
                .await
                .unwrap();
            context.active_goal_id = Some(goal.id);
            let marker = context.work_dir.join("goal-yield-marker");
            let mut agent = Agent::new(
                tact_llm::LlmProvider::Mock(tact_llm::MockClient::new(vec![])),
                context,
                crate::tool::toolset(),
                crate::mcp::MCPToolRouter::new(),
                crate::permission::PermissionManager::try_new(
                    crate::permission::PermissionMode::Auto,
                )
                .unwrap(),
                crate::agent::AgentSystemPrompt::Static("You are a test agent.".to_string()),
            );
            let content = vec![
                ContentBlock::ToolUse {
                    id: "goal-request".to_string(),
                    name: "goal_request_input".to_string(),
                    input: serde_json::json!({"question": "which path?"}),
                },
                ContentBlock::ToolUse {
                    id: "later-bash".to_string(),
                    name: "bash".to_string(),
                    input: serde_json::json!({
                        "command": format!("printf marker > {}", marker.display())
                    }),
                },
            ];
            let (blocks, outcome) = agent.execute_tool_call(&content).await.unwrap();
            (blocks, outcome, marker)
        });

        assert_eq!(outcome.manual_compact, None);
        assert_eq!(outcome.goal_yield, Some(GoalYieldReason::NeedsUser));
        assert_eq!(
            tool_result_text(&blocks),
            "User input requested; yield to Runner.\nNot executed: Goal requested user input"
        );
        assert!(!marker.exists(), "later bash call must not run");
    }

    #[test]
    fn dispatch_clears_stale_goal_yield_reason() {
        let mut agent = agent_with(
            "dispatch_clears_goal_yield_reason",
            crate::permission::PermissionMode::Auto,
        );
        agent.runtime.goal_yield_reason = Some(GoalYieldReason::NeedsUser);

        let (blocks, outcome) = block_on(agent.execute_tool_call(&tool_use(
            "bash",
            serde_json::json!({ "command": "printf ok" }),
        )))
        .unwrap();

        assert_eq!(tool_result_text(&blocks), "ok");
        assert_eq!(outcome.goal_yield, None);
        assert_eq!(agent.runtime.goal_yield_reason, None);
    }

    /// A `PostToolUse` block turns an otherwise successful call into a failed
    /// one, and a failed call must not carry the Goal yield its execution
    /// produced — the yield is what makes the Runner stop, so a hook that
    /// rejects the result has to be able to cancel it.
    #[test]
    fn post_tool_use_failure_drops_goal_yield() {
        let (blocks, outcome) = block_on(async {
            crate::config::test_support::install_default();
            let mut context =
                crate::tool::test_support::test_context("dispatch_goal_yield_post_tool_blocked");
            let goal = context
                .goal_manager
                .create(crate::goal::GoalCreate {
                    title: "post tool blocked".into(),
                    objective: "pause for user input".into(),
                    criteria: vec![crate::goal::GoalCriterion::required_with_id(
                        "criterion-1",
                        "input received",
                    )],
                })
                .await
                .unwrap();
            context.active_goal_id = Some(goal.id);
            let mut agent = Agent::new(
                tact_llm::LlmProvider::Mock(tact_llm::MockClient::new(vec![])),
                context,
                crate::tool::toolset(),
                crate::mcp::MCPToolRouter::new(),
                crate::permission::PermissionManager::try_new(
                    crate::permission::PermissionMode::Auto,
                )
                .unwrap(),
                crate::agent::AgentSystemPrompt::Static("You are a test agent.".to_string()),
            )
            .with_post_tool_hook(|_state, _tool_use, _result, _status| {
                Box::pin(async { Ok(HookControl::Block("nope".to_string())) })
            });

            agent
                .execute_tool_call(&tool_use(
                    "goal_request_input",
                    serde_json::json!({"question": "which path?"}),
                ))
                .await
                .unwrap()
        });

        assert_eq!(
            tool_result_text(&blocks),
            "Tool blocked by PostToolUse hook: nope"
        );
        assert_eq!(
            outcome.goal_yield, None,
            "a PostToolUse-failed call must drop the Goal yield"
        );
    }

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

    /// A kernel-dispatched tool must see the context the Agent has *now*, not
    /// the one it had at construction.
    ///
    /// The two fields asserted here are stamped per turn, after `Agent::new`:
    /// a frozen snapshot would give a spawned child `PermissionMode::Default`
    /// instead of the parent's mode, and a background child no queue to report
    /// into. Both were silently broken by the cutover and no test caught it.
    #[test]
    fn preflight_hands_every_tool_the_current_context() {
        let mut agent = agent_with("fresh_context", crate::permission::PermissionMode::Plan);
        // Cancel before the walk so pre-flight returns without executing: this
        // asserts what the factory holds, not what a tool did.
        block_on(agent.preflight_tool_calls(&tool_use(
            "read_file",
            serde_json::json!({"path": "src/main.rs"}),
        )))
        .unwrap();

        let snapshot = agent
            .tool_contexts
            .with_base(|base| base.permission_snapshot.clone())
            .expect("pre-flight stamps the permission snapshot");
        assert_eq!(snapshot.mode, crate::permission::PermissionMode::Plan);
        assert!(
            agent
                .tool_contexts
                .with_base(|base| base.subagent_results.is_some()),
            "a background child needs the parent's results queue"
        );
    }

    /// A frontend attached *after* construction must still get a popup.
    ///
    /// `with_ui_channel` is the documented attach point and is called after
    /// `Agent::new` everywhere, including `session_bootstrap`. A `ui_available`
    /// captured at construction therefore reported "nobody can answer" and sent
    /// every `Ask` through the headless ladder — a write was silently allowed
    /// once and a high-risk call silently refused, with no prompt either way.
    #[test]
    fn a_late_ui_channel_still_produces_a_permission_prompt() {
        let mut agent = agent_with("late_ui", crate::permission::PermissionMode::Default);
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        agent = agent.with_ui_channel(tx);

        // A write in Default mode is an `Ask`; answer it as "deny" so pre-flight
        // returns instead of waiting for a popup nobody will click.
        let responder = agent.tool_context.ui_responder.clone();
        let answer = std::thread::spawn(move || {
            // Bounded: without a deadline a regression makes this thread wait
            // forever for a popup that never appears, turning a red test into a
            // hung one.
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
            let pending = loop {
                if let Some(pending) = responder.snapshot().into_iter().next() {
                    break Some(pending.request_id);
                }
                if std::time::Instant::now() >= deadline {
                    break None;
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            };
            if let Some(request_id) = pending {
                responder.respond(tact_protocol::UiResponse::Select {
                    request_id,
                    choice: Some(1),
                });
            }
        });
        let outcome = block_on(agent.preflight_tool_calls(&tool_use(
            "write_file",
            serde_json::json!({"path": "x.txt", "content": "y"}),
        )))
        .unwrap();
        answer.join().unwrap();

        let message = outcome
            .prepared
            .into_iter()
            .map(|p| match p.state {
                PreparedState::Run => "run".to_string(),
                PreparedState::Resolved(msg) => msg,
            })
            .next()
            .unwrap();
        assert!(
            message.contains("Permission denied by user"),
            "a late UI must be asked, got: {message}"
        );
        // The popup really was emitted, not just answered from a queue.
        let mut prompts = 0;
        while let Ok(update) = rx.try_recv() {
            if matches!(update, AgentUpdate::RequestSelect { .. }) {
                prompts += 1;
            }
        }
        assert_eq!(prompts, 1, "exactly one popup");
    }

    /// A call against a stale tool list is refused, not dispatched.
    ///
    /// The generation has to come from what the model was shown. Reading a fresh
    /// snapshot made the guard vacuous — it compared the current generation with
    /// itself, so a tool that appeared after the request would still dispatch.
    #[test]
    fn a_stale_generation_is_refused() {
        let mut agent = agent_with("stale_generation", crate::permission::PermissionMode::Auto);
        // Pretend the model is holding a list from a different generation.
        agent.advertised_generation = agent.tool_executor.catalog().generation() + 1;
        let outcome = outcome(
            &mut agent,
            "read_file",
            serde_json::json!({"path": "src/main.rs"}),
        );
        assert!(
            outcome.contains("stale tool descriptor snapshot"),
            "expected a stale-generation refusal, got: {outcome}"
        );
    }

    /// A credential refusal still names what it refused.
    ///
    /// The cell's label line is the only place a reader learns the *kind* of
    /// material; the cutover dropped it because `resolve` returned a bare error
    /// string. `LabeledRefusal` carries it back.
    #[test]
    fn a_credential_refusal_keeps_its_label() {
        let mut agent = agent_with("refusal_label", crate::permission::PermissionMode::Auto);
        let preflight = block_on(agent.preflight_tool_calls(&tool_use(
            "read_file",
            serde_json::json!({"path": ".ssh/id_ed25519"}),
        )))
        .unwrap();
        let prepared = preflight.prepared.into_iter().next().unwrap();
        assert!(matches!(prepared.state, PreparedState::Resolved(_)));
        assert_eq!(
            prepared.permission_label.as_deref(),
            Some("Refused: private-key")
        );
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
        let exec = run_control_tool(
            &router_with_a_resource(),
            McpControl::Resource(McpResourceTool::List),
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
        let exec = run_control_tool(
            &router_with_a_resource(),
            McpControl::Resource(McpResourceTool::Read),
            &serde_json::json!({"uri": "memory://guide"}),
        )
        .await;
        assert!(matches!(exec.status, StepStatus::Failed));
        assert!(
            exec.content.contains("`server` is required"),
            "{}",
            exec.content
        );

        let exec = run_control_tool(
            &router_with_a_resource(),
            McpControl::Resource(McpResourceTool::Read),
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
        let exec = run_control_tool(
            &router_with_a_resource(),
            McpControl::Resource(McpResourceTool::Read),
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

        let exec = run_control_tool(
            &router,
            McpControl::Resource(McpResourceTool::Templates),
            &serde_json::json!({}),
        )
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
    /// Run one MCP control tool the way the pipeline does, returning the shape
    /// the tests assert on (`StepStatus` + content).
    async fn run_control_tool(
        router: &MCPToolRouter,
        control: crate::tool::McpControl,
        input: &serde_json::Value,
    ) -> ExecResult {
        let backend = std::sync::Arc::new(crate::tool::SnapshotMcpControlBackend::new(router));
        let tool = crate::tool::McpControlTool::new(control, backend);
        // A per-call directory: `test_context` wipes and recreates its target,
        // so a shared name makes these tests destroy each other's workspace when
        // they run in parallel.
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let name = format!(
            "mcp_control_runner_{}",
            NEXT.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
        );
        let context = crate::tool::test_support::test_context(&name);
        let call_ctx = kernel_ctx(&context);
        match tact_kernel::capability::tool::Tool::call(&tool, call_ctx, input.clone()).await {
            Ok(result) => ExecResult {
                content: result.content,
                status: StepStatus::Success,
                image: None,
                effects: Vec::new(),
            },
            Err(error) => ExecResult {
                content: format!("{error}"),
                status: StepStatus::Failed,
                image: None,
                effects: Vec::new(),
            },
        }
    }

    fn kernel_ctx(
        context: &crate::tool::ToolContext,
    ) -> tact_kernel::tool::context::ToolCallContext {
        struct Events;
        impl tact_kernel::tool::context::EventSink for Events {
            fn emit(&self, _event: tact_kernel::tool::context::ToolEvent) {}
        }
        tact_kernel::tool::context::ToolCallContext {
            run_id: "run".into(),
            tool_use_id: "call".into(),
            session_id: None,
            active_goal_id: None,
            origin: tact_kernel::tool::context::ToolOrigin::Main,
            workspace: tact_kernel::tool::context::Workspace::new(&context.work_dir),
            events: std::sync::Arc::new(Events),
            interaction: std::sync::Arc::new(crate::permission::UiInteractionBroker::from_context(
                context,
            )),
            cancel: tact_kernel::tool::context::CancelToken::new(),
            redaction: tact_kernel::tool::context::RedactionPolicy::Off,
        }
    }

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
        let exec = run_control_tool(
            &router_with_a_prompt(),
            McpControl::Prompt(crate::mcp::McpPromptTool::List),
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
        let exec = run_control_tool(
            &router_with_a_prompt(),
            McpControl::Prompt(crate::mcp::McpPromptTool::Get),
            &serde_json::json!({"name": "getting_started"}),
        )
        .await;
        assert!(matches!(exec.status, StepStatus::Failed));
        assert!(
            exec.content.contains("`server` is required"),
            "{}",
            exec.content
        );

        let exec = run_control_tool(
            &router_with_a_prompt(),
            McpControl::Prompt(crate::mcp::McpPromptTool::Get),
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
        let exec = run_control_tool(
            &router_with_a_prompt(),
            McpControl::Prompt(crate::mcp::McpPromptTool::Get),
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
        let ok = crate::tool::prompt_arguments(&serde_json::json!({
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
        assert_eq!(
            crate::tool::prompt_arguments(&serde_json::json!({})).unwrap(),
            None
        );
        assert_eq!(
            crate::tool::prompt_arguments(&serde_json::json!({"arguments": {}})).unwrap(),
            None
        );

        let error =
            crate::tool::prompt_arguments(&serde_json::json!({"arguments": {"topic": ["a", "b"]}}))
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
        let resolved = ResolvedTool::Unknown;
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
            kernel: None,
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
