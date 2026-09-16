use crate::{
    background::{
        BackgroundProgressSink, BackgroundTaskRecord, BackgroundTaskStatus, WaitOutcome,
        record_in_session,
    },
    tool::{
        ArgumentSummaryPolicy, DetailPolicy, LiveOutputPolicy, OutputPolicy, PermissionPolicy,
        PermissionPromptPolicy, PopupPolicy, ResourcePolicy, ToolDomain, ToolMetadata,
        ToolPresentation,
    },
};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Deserialize;
use std::time::Duration;
use tact_protocol::ToolVisualKind;
use tool_refactor_macros::tool;

use crate::tool::ToolContext;

/// Default and maximum blocking time for the wait-shaped calls: five minutes,
/// matching the `sleep` tool's cap so neither can outlast a user's patience.
const DEFAULT_WAIT_MS: u64 = 300_000;
const MAX_WAIT_MS: u64 = 300_000;

/// How much of a finished task's captured output is inlined into the result.
/// The full stream stays in the task's log file, whose path is reported.
const WAIT_OUTPUT_TAIL_CHARS: usize = 4_000;

fn capped_wait_ms(ms: u64) -> u64 {
    ms.min(MAX_WAIT_MS)
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct BackgroundRunInput {
    #[schemars(description = "Shell command to run in the background.")]
    pub command: String,
    #[schemars(
        description = "Optional: block up to this many milliseconds for the command to finish and \
                       return its output instead of just the task id (max 300000). Omit (or 0) to \
                       return immediately."
    )]
    #[serde(default)]
    pub wait_ms: Option<u64>,
}

/// Metadata for the `background_run` tool.
pub const BACKGROUND_RUN_METADATA: ToolMetadata = ToolMetadata {
    name: "background_run",
    description: "Run a shell command in the background. Set wait_ms to block until it finishes \
                  and get its output back in this same call.",
    permission: PermissionPolicy::ShellCommand {
        command_field: "command",
    },
    permission_prompt: PermissionPromptPolicy::Command { field: "command" },
    resources: ResourcePolicy::Barrier,
    domain: ToolDomain::Generic,
    presentation: ToolPresentation {
        visual_kind: ToolVisualKind::Command,
        display_name: "⚙️ Background Run",
        live_output: LiveOutputPolicy::Background,
        detail: DetailPolicy::Result,
        popup: PopupPolicy::None,
        compact_result_to_meta: false,
    },
    output: OutputPolicy::KeepInline,
    argument_summary: ArgumentSummaryPolicy::Command { field: "command" },
};

#[tool]
/// # Errors
///
/// Returns an error if the background manager fails to run the command
/// (e.g., invalid command or internal error).
pub async fn background_run(ctx: ToolContext, input: BackgroundRunInput) -> Result<String> {
    // Stream live output into the invocation's tool card while the task runs.
    let progress = BackgroundProgressSink::new(ctx.progress_reporter.tool_id(), ctx.ui_tx.clone());
    let command = input.command;
    let id = ctx
        .background_manager
        .start(
            command.clone(),
            &ctx.work_dir,
            ctx.session_id.clone().unwrap_or_default(),
            Some(progress),
        )
        .await?;
    let started = format!("Background task {id} started: {command}");

    let Some(wait_ms) = input.wait_ms.map(capped_wait_ms).filter(|ms| *ms > 0) else {
        return Ok(started);
    };

    let outcome = ctx
        .background_manager
        .wait(
            Some(&id),
            "",
            Duration::from_millis(wait_ms),
            &ctx.cancel_flag,
        )
        .await?;

    if outcome == WaitOutcome::Finished {
        // The task is done: report the result directly instead of the id.
        return report_waited(&ctx, &id, WaitOutcome::Finished, None).await;
    }
    Ok(format!(
        "{started}\n{}",
        report_waited(&ctx, &id, outcome, Some(wait_ms)).await?
    ))
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CheckBackgroundInput {
    #[schemars(description = "Optional background task id.")]
    pub task_id: Option<String>,
}

/// Metadata for the `check_background` tool.
pub const CHECK_BACKGROUND_METADATA: ToolMetadata = ToolMetadata {
    name: "check_background",
    description: "Check background task status without waiting. To block until a task finishes, \
                  call wait_background instead of polling this in a loop.",
    permission: PermissionPolicy::Read,
    permission_prompt: PermissionPromptPolicy::Json,
    resources: ResourcePolicy::Independent,
    domain: ToolDomain::Generic,
    presentation: ToolPresentation {
        visual_kind: ToolVisualKind::Generic,
        display_name: "⚙️ Background Check",
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
/// Returns an error if the provided task ID does not exist or the background
/// manager encounters an internal error.
pub async fn check_background(ctx: ToolContext, input: CheckBackgroundInput) -> Result<String> {
    ctx.background_manager
        .check(input.task_id.as_deref(), ctx.session_id.as_deref())
        .await
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct WaitBackgroundInput {
    #[schemars(
        description = "Background task id to wait for. Omit to wait for every background task \
                       started in this session."
    )]
    #[serde(default)]
    pub task_id: Option<String>,
    #[schemars(
        description = "How long to wait at most, in milliseconds (default 300000 = 5 minutes, max \
                       300000). The call returns as soon as the task finishes."
    )]
    #[serde(default)]
    pub timeout_ms: Option<u64>,
}

/// Metadata for the `wait_background` tool.
pub const WAIT_BACKGROUND_METADATA: ToolMetadata = ToolMetadata {
    name: "wait_background",
    description: "Wait for background tasks to finish, returning the moment they do. Use this \
                  instead of polling check_background in a loop or sleeping to burn time.",
    permission: PermissionPolicy::Read,
    permission_prompt: PermissionPromptPolicy::Json,
    resources: ResourcePolicy::Independent,
    domain: ToolDomain::Generic,
    presentation: ToolPresentation {
        visual_kind: ToolVisualKind::Sleep,
        display_name: "⏳ Wait Background",
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
/// Returns an error if the given task id is unknown or the background manager
/// cannot be read.
pub async fn wait_background(ctx: ToolContext, input: WaitBackgroundInput) -> Result<String> {
    let timeout =
        Duration::from_millis(capped_wait_ms(input.timeout_ms.unwrap_or(DEFAULT_WAIT_MS)));
    let outcome = ctx
        .background_manager
        .wait(
            input.task_id.as_deref(),
            &ctx.session_id.clone().unwrap_or_default(),
            timeout,
            &ctx.cancel_flag,
        )
        .await?;
    report_waited(
        &ctx,
        input.task_id.as_deref().unwrap_or(""),
        outcome,
        Some(timeout.as_millis() as u64),
    )
    .await
}

/// Render the outcome of a wait: the finished task (with the tail of its
/// output), or why the wait ended while it was still running.
async fn report_waited(
    ctx: &ToolContext,
    task_id: &str,
    outcome: WaitOutcome,
    waited_ms: Option<u64>,
) -> Result<String> {
    if task_id.is_empty() {
        return session_report(ctx, outcome, waited_ms).await;
    }

    let record = ctx.background_manager.record(task_id).await?;
    let status = record
        .as_ref()
        .map(|record| status_word(record.status))
        .unwrap_or("unknown");

    Ok(match outcome {
        WaitOutcome::Finished => {
            let Some(record) = record else {
                return Ok(format!(
                    "Background task {task_id}: no longer in the store."
                ));
            };
            let mut text = format!(
                "Background task {task_id}: {status} after {}",
                elapsed(&record)
            );
            text.push('\n');
            text.push_str(&output_tail(&record.output));
            if let Some(path) = &record.output_path {
                text.push_str(&format!("\nfull log: {path}"));
            }
            text
        }
        WaitOutcome::TimedOut => format!(
            "Background task {task_id} is still running after {}ms (status: {status}). It keeps \
             running; call wait_background again — it returns as soon as the task finishes.",
            waited_ms.unwrap_or(DEFAULT_WAIT_MS)
        ),
        WaitOutcome::Cancelled => format!(
            "Wait cancelled by the user; background task {task_id} is still running (status: \
             {status})."
        ),
    })
}

/// Report for a wait that covered every task of the session.
async fn session_report(
    ctx: &ToolContext,
    outcome: WaitOutcome,
    waited_ms: Option<u64>,
) -> Result<String> {
    let session_id = ctx.session_id.clone().unwrap_or_default();
    let mut records: Vec<BackgroundTaskRecord> = ctx
        .background_manager
        .records()
        .await?
        .into_iter()
        .filter(|record| record_in_session(record, Some(&session_id)))
        .collect();
    records.sort_by_key(|record| record.started_at);

    let running: Vec<&BackgroundTaskRecord> = records
        .iter()
        .filter(|record| record.status == BackgroundTaskStatus::Running)
        .collect();

    let header = match outcome {
        WaitOutcome::Finished => {
            if records.is_empty() {
                "No background tasks in this session.".to_string()
            } else {
                format!(
                    "All {} background task(s) of this session finished.",
                    records.len()
                )
            }
        }
        WaitOutcome::TimedOut => format!(
            "{} of {} background task(s) still running after {}ms.",
            running.len(),
            records.len(),
            waited_ms.unwrap_or(DEFAULT_WAIT_MS)
        ),
        WaitOutcome::Cancelled => format!(
            "Wait cancelled by the user; {} of {} background task(s) still running.",
            running.len(),
            records.len()
        ),
    };

    let listing = records
        .iter()
        .map(|record| {
            format!(
                "  {} {} ({}) {}",
                record.id,
                status_word(record.status),
                elapsed(record),
                record.command
            )
        })
        .collect::<Vec<_>>()
        .join("\n");

    Ok(if listing.is_empty() {
        header
    } else {
        format!("{header}\n{listing}")
    })
}

fn status_word(status: BackgroundTaskStatus) -> &'static str {
    match status {
        BackgroundTaskStatus::Running => "running",
        BackgroundTaskStatus::Completed => "completed",
        BackgroundTaskStatus::Error => "error",
    }
}

/// How long the task ran (or has been running, for a live task).
fn elapsed(record: &BackgroundTaskRecord) -> String {
    let end = record.finished_at.unwrap_or_else(chrono::Utc::now);
    let millis = (end - record.started_at).num_milliseconds().max(0);
    format!("{:.1}s", millis as f64 / 1000.0)
}

/// The tail of a task's captured output, bounded for the model's context; the
/// full stream stays in the task's log file.
fn output_tail(output: &str) -> String {
    let trimmed = output.trim_end();
    if trimmed.is_empty() {
        return "(no output)".to_string();
    }
    let chars: Vec<char> = trimmed.chars().collect();
    if chars.len() <= WAIT_OUTPUT_TAIL_CHARS {
        return trimmed.to_string();
    }
    let tail: String = chars[chars.len() - WAIT_OUTPUT_TAIL_CHARS..]
        .iter()
        .collect();
    format!("… (truncated to the last {WAIT_OUTPUT_TAIL_CHARS} chars)\n{tail}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool::test_support::{run_tool, test_context};

    #[tokio::test]
    async fn check_background_lists_empty_when_no_tasks() {
        let context = test_context("check_background_lists_empty_when_no_tasks");

        let output = run_tool(
            &context,
            CheckBackgroundTool,
            "check_background",
            serde_json::json!({}),
        )
        .await
        .unwrap();

        assert_eq!(output, "No background tasks.");
    }

    #[tokio::test]
    async fn check_background_lists_only_this_session() {
        let mut context = test_context("check_background_lists_only_this_session");
        context.session_id = Some("sess-mine".to_string());

        run_tool(
            &context,
            BackgroundRunTool,
            "background_run",
            serde_json::json!({ "command": "sleep 0.2 && echo my-own-task" }),
        )
        .await
        .unwrap();
        // A task started by a different session in the same store.
        context
            .background_manager
            .start(
                "sleep 30".to_string(),
                &context.work_dir,
                "sess-other".to_string(),
                None,
            )
            .await
            .unwrap();

        let listing = run_tool(
            &context,
            CheckBackgroundTool,
            "check_background",
            serde_json::json!({}),
        )
        .await
        .unwrap();

        assert!(listing.contains("my-own-task"), "listing: {listing}");
        assert!(!listing.contains("sleep 30"), "listing: {listing}");
    }

    #[tokio::test]
    async fn check_background_errors_for_unknown_task_id() {
        let context = test_context("check_background_errors_for_unknown_task_id");

        let error = run_tool(
            &context,
            CheckBackgroundTool,
            "check_background",
            serde_json::json!({ "task_id": "deadbeef" }),
        )
        .await
        .unwrap_err();

        assert!(error.to_string().contains("Unknown background task"));
    }

    /// Starts a background command through the manager and returns its task id.
    async fn start_task(context: &ToolContext, command: &str) -> String {
        context
            .background_manager
            .start(command.to_string(), &context.work_dir, String::new(), None)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn background_run_with_wait_ms_returns_the_output_inline() {
        let context = test_context("background_run_with_wait_ms");

        let output = tokio::time::timeout(
            Duration::from_secs(20),
            run_tool(
                &context,
                BackgroundRunTool,
                "background_run",
                serde_json::json!({ "command": "echo waited-marker", "wait_ms": 10_000 }),
            ),
        )
        .await
        .expect("a waited background_run must return")
        .unwrap();

        assert!(output.contains("completed"), "output: {output}");
        assert!(output.contains("waited-marker"), "output: {output}");
        assert!(
            !output.contains("started:"),
            "a finished wait reports the result, not the start line: {output}"
        );
    }

    #[tokio::test]
    async fn background_run_without_wait_ms_still_returns_the_started_line() {
        let context = test_context("background_run_without_wait_ms");

        let output = run_tool(
            &context,
            BackgroundRunTool,
            "background_run",
            serde_json::json!({ "command": "sleep 1" }),
        )
        .await
        .unwrap();

        assert!(output.starts_with("Background task "), "output: {output}");
        assert!(output.contains("started:"), "output: {output}");
    }

    #[tokio::test]
    async fn wait_background_with_a_task_id_returns_the_output() {
        let context = test_context("wait_background_with_task_id");
        let id = start_task(&context, "sleep 0.2 && echo waited-done").await;

        let output = tokio::time::timeout(
            Duration::from_secs(20),
            run_tool(
                &context,
                WaitBackgroundTool,
                "wait_background",
                serde_json::json!({ "task_id": id }),
            ),
        )
        .await
        .expect("wait_background must return once the task finishes")
        .unwrap();

        assert!(output.contains("completed"), "output: {output}");
        assert!(output.contains("waited-done"), "output: {output}");
    }

    #[tokio::test]
    async fn wait_background_without_a_task_id_reports_the_session() {
        let context = test_context("wait_background_without_task_id");
        start_task(&context, "sleep 0.2 && echo session-done").await;

        let output = tokio::time::timeout(
            Duration::from_secs(20),
            run_tool(
                &context,
                WaitBackgroundTool,
                "wait_background",
                serde_json::json!({}),
            ),
        )
        .await
        .expect("wait_background must return once the session is idle")
        .unwrap();

        assert!(
            output.contains("All 1 background task(s)"),
            "output: {output}"
        );
        assert!(output.contains("completed"), "output: {output}");
    }

    #[tokio::test]
    async fn wait_background_times_out_and_says_the_task_keeps_running() {
        let context = test_context("wait_background_times_out");
        let id = start_task(&context, "sleep 30").await;

        let output = tokio::time::timeout(
            Duration::from_secs(20),
            run_tool(
                &context,
                WaitBackgroundTool,
                "wait_background",
                serde_json::json!({ "task_id": id, "timeout_ms": 300 }),
            ),
        )
        .await
        .expect("wait_background must honour its timeout")
        .unwrap();

        assert!(output.contains("still running"), "output: {output}");
    }

    #[tokio::test]
    async fn wait_background_rejects_an_unknown_task_id() {
        let context = test_context("wait_background_unknown_id");

        let error = run_tool(
            &context,
            WaitBackgroundTool,
            "wait_background",
            serde_json::json!({ "task_id": "deadbeef" }),
        )
        .await
        .unwrap_err();

        assert!(error.to_string().contains("Unknown background task"));
    }

    #[test]
    fn capped_wait_ms_matches_the_sleep_cap() {
        assert_eq!(capped_wait_ms(0), 0);
        assert_eq!(capped_wait_ms(1_000), 1_000);
        assert_eq!(capped_wait_ms(999_999), MAX_WAIT_MS);
    }

    #[test]
    fn output_tail_keeps_the_end_and_marks_truncation() {
        assert_eq!(output_tail("  "), "(no output)");

        let long = "x".repeat(WAIT_OUTPUT_TAIL_CHARS * 5) + "TAIL";
        let tail = output_tail(&long);
        assert!(tail.contains("truncated"));
        assert!(tail.ends_with("TAIL"));
        assert!(tail.chars().count() < long.chars().count());
    }
}
