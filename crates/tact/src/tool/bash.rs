use std::{sync::atomic::Ordering, time::Duration};

use crate::tool::{
    ArgumentSummaryPolicy, DetailPolicy, LiveOutputPolicy, OutputPolicy, PermissionPolicy,
    PermissionPromptPolicy, PopupPolicy, ResourcePolicy, ToolDomain, ToolMetadata,
    ToolPresentation,
};
use anyhow::{Context, Result};
use schemars::JsonSchema;
use serde::Deserialize;
use tact_protocol::ToolVisualKind;
use tact_protocol::{ToolOutputBuffer, ToolOutputChunk, ToolOutputStream};
use tokio::{
    process::{Child, Command},
    sync::mpsc,
    time::{Interval, MissedTickBehavior, interval},
};
use tool_refactor_macros::tool;

use crate::{
    pipe_stream::{
        PIPE_CHANNEL_CAPACITY, PROGRESS_INTERVAL, PendingProgress, PipeEvent, Utf8Decoder,
        read_pipe, stream_index,
    },
    shell::validate_shell_command,
    tool::ToolContext,
};

const OUTPUT_LIMIT_CHARS: usize = 50_000;

fn push_decoded(
    stream: ToolOutputStream,
    text: String,
    capture: &mut ToolOutputBuffer,
    pending: &mut PendingProgress,
) {
    if text.is_empty() {
        return;
    }
    let chunk = ToolOutputChunk {
        stream,
        kind: None,
        text,
    };
    capture.push_chunks(std::slice::from_ref(&chunk));
    pending.push(chunk);
}

fn report_pending(
    ctx: &ToolContext,
    pending: &mut PendingProgress,
    progress_tick: &mut Interval,
) -> bool {
    if pending.is_empty() {
        return false;
    }
    ctx.progress_reporter.report(pending.take());
    progress_tick.reset();
    true
}

#[cfg(unix)]
fn configure_process_group(command: &mut Command) {
    command.process_group(0);
}

/// Lower the scheduling priority of the child's entire process group so
/// TUI stays responsive during CPU-heavy commands like `cargo test`.
#[cfg(unix)]
fn set_process_group_priority(process_group_id: u32, nice: i32) {
    if nice > 0 {
        // SAFETY: process_group_id comes from Child::id() which always
        // returns a valid OS PID. PRIO_PGRP with our own PGRP is safe;
        // setpriority is not a memory-safety operation.
        unsafe {
            libc::setpriority(libc::PRIO_PGRP, process_group_id, nice);
        }
    }
}

#[cfg(not(unix))]
fn configure_process_group(_command: &mut Command) {}

#[cfg(not(unix))]
fn set_process_group_priority(_process_group_id: u32, _nice: i32) {}

async fn terminate_child(child: &mut Child, process_group_id: Option<u32>) {
    #[cfg(unix)]
    if let Some(pid) = process_group_id
        && let Ok(pid) = i32::try_from(pid)
    {
        // SAFETY: the spawned shell is placed in a process group whose id is its
        // positive pid; negating it asks kill(2) to signal that group.
        unsafe {
            libc::kill(-pid, libc::SIGKILL);
        }
    }
    #[cfg(not(unix))]
    let _ = process_group_id;
    let _ = child.kill().await;
}

async fn terminate_process(
    child: &mut Child,
    process_group_id: Option<u32>,
    stdout_task: &tokio::task::JoinHandle<()>,
    stderr_task: &tokio::task::JoinHandle<()>,
) {
    terminate_child(child, process_group_id).await;
    #[cfg(not(unix))]
    {
        stdout_task.abort();
        stderr_task.abort();
    }
    #[cfg(unix)]
    let _ = (stdout_task, stderr_task);
}

fn error_with_partial(reason: &str, capture: &ToolOutputBuffer) -> anyhow::Error {
    let partial = capture.full_detail_text();
    if partial.trim().is_empty() {
        anyhow::anyhow!("Error: {reason}")
    } else {
        anyhow::anyhow!("Error: {reason}\n\nPartial output:\n{}", partial.trim())
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct BashInput {
    #[schemars(description = "Shell command to run in the current workspace.")]
    pub command: String,
}

pub const BASH_METADATA: ToolMetadata = ToolMetadata {
    name: "bash",
    description: "Run a shell command in the current workspace.",
    permission: PermissionPolicy::ShellCommand {
        command_field: "command",
    },
    permission_prompt: PermissionPromptPolicy::Command { field: "command" },
    resources: ResourcePolicy::Barrier,
    domain: ToolDomain::Generic,
    presentation: ToolPresentation {
        visual_kind: ToolVisualKind::Command,
        display_name: "$ Bash",
        live_output: LiveOutputPolicy::Standard,
        detail: DetailPolicy::Result,
        popup: PopupPolicy::None,
        compact_result_to_meta: false,
    },
    output: OutputPolicy::PersistLargeOutput,
    argument_summary: ArgumentSummaryPolicy::Command { field: "command" },
};

#[tool]
/// # Errors
///
/// Returns an error if:
/// - The shell command is invalid or potentially dangerous.
/// - The shell process cannot be spawned.
/// - The stdout or stderr pipes cannot be captured.
/// - The command times out (configured via `ctx.bash_timeout_secs`).
/// - The command is cancelled by the user.
/// - The command exits with a failure or the pipe readers encounter an error.
pub async fn bash(ctx: ToolContext, input: BashInput) -> Result<String> {
    let command = input.command;

    validate_shell_command(&command)?;

    let mut process = Command::new("sh");
    process
        .arg("-c")
        .arg(command)
        .current_dir(&ctx.work_dir)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    configure_process_group(&mut process);
    let mut child = process.spawn().context("failed to spawn shell process")?;
    let process_group_id = child.id();
    if let Some(pgid) = process_group_id {
        set_process_group_priority(pgid, ctx.bash_nice);
    }

    let stdout = child.stdout.take().context("stdout pipe unavailable")?;
    let stderr = child.stderr.take().context("stderr pipe unavailable")?;
    let (pipe_tx, mut pipe_rx) = mpsc::channel(PIPE_CHANNEL_CAPACITY);
    let stdout_task = tokio::spawn(read_pipe(stdout, ToolOutputStream::Stdout, pipe_tx.clone()));
    let stderr_task = tokio::spawn(read_pipe(stderr, ToolOutputStream::Stderr, pipe_tx.clone()));
    drop(pipe_tx);

    let mut decoders = [
        Utf8Decoder::default(),
        Utf8Decoder::default(),
        Utf8Decoder::default(),
    ];
    let mut capture = ToolOutputBuffer::new_full(OUTPUT_LIMIT_CHARS);
    let mut pending = PendingProgress::default();
    let mut progress_tick = interval(PROGRESS_INTERVAL);
    progress_tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
    progress_tick.tick().await;
    let timeout_enabled = ctx.bash_timeout_secs != 0;
    let timeout_sleep = tokio::time::sleep(Duration::from_secs(ctx.bash_timeout_secs.max(1)));
    tokio::pin!(timeout_sleep);
    let mut exit_status = None;
    let mut closed_pipes = 0_usize;
    let mut failure_reason = None;
    let mut sent_progress = false;

    while exit_status.is_none() || closed_pipes < 2 {
        tokio::select! {
            event = pipe_rx.recv(), if closed_pipes < 2 => {
                match event {
                    Some(PipeEvent::Bytes(stream, bytes)) => {
                        let text = decoders[stream_index(stream)].push(&bytes);
                        push_decoded(stream, text, &mut capture, &mut pending);
                        if !sent_progress && report_pending(&ctx, &mut pending, &mut progress_tick) {
                            sent_progress = true;
                        }
                    }
                    Some(PipeEvent::Closed(stream)) => {
                        let text = decoders[stream_index(stream)].finish();
                        push_decoded(stream, text, &mut capture, &mut pending);
                        closed_pipes += 1;
                    }
                    Some(PipeEvent::Failed(stream, error)) => {
                        let text = decoders[stream_index(stream)].finish();
                        push_decoded(stream, text, &mut capture, &mut pending);
                        closed_pipes += 1;
                        if failure_reason.is_none() {
                            failure_reason = Some(format!("reading {stream:?}: {error}"));
                            terminate_process(
                                &mut child,
                                process_group_id,
                                &stdout_task,
                                &stderr_task,
                            ).await;
                        }
                    }
                    None => closed_pipes = 2,
                }
            }
            status = child.wait(), if exit_status.is_none() => {
                match status {
                    Ok(status) => exit_status = Some(status),
                    Err(error) => {
                        failure_reason.get_or_insert_with(|| format!("waiting for command: {error}"));
                        exit_status = Some(std::process::ExitStatus::default());
                    }
                }
                // Shell has exited. If pipes are still held open by orphaned
                // background grandchildren, we would hang forever (the loop
                // waits for closed_pipes == 2). Kill the process group now so
                // the pipe readers see EOF and the loop can finish.
                if closed_pipes < 2 {
                    terminate_child(&mut child, process_group_id).await;
                }
            }
            _ = progress_tick.tick() => {
                if report_pending(&ctx, &mut pending, &mut progress_tick) {
                    sent_progress = true;
                }
                if failure_reason.is_none() && ctx.cancel_flag.load(Ordering::Relaxed) {
                    // Relaxed ordering is sufficient here: the cancel flag is a
                    // plain boolean signal set by the parent task and consumed
                    // periodically by this select loop — there is no need for
                    // synchronization with other atomic operations.
                    failure_reason = Some("Cancelled by user".to_string());
                    terminate_process(
                        &mut child,
                        process_group_id,
                        &stdout_task,
                        &stderr_task,
                    ).await;
                }
            }
            _ = &mut timeout_sleep, if timeout_enabled && failure_reason.is_none() => {
                failure_reason = Some(format!("Timeout ({}s)", ctx.bash_timeout_secs));
                terminate_process(
                    &mut child,
                    process_group_id,
                    &stdout_task,
                    &stderr_task,
                ).await;
            }
        }
    }

    // Guard: kill any orphaned descendants that may still hold pipe fds open.
    // Normally handled when child.wait() fires, but this is a safety net in
    // case an unexpected code path exits the loop with a live process group.
    terminate_child(&mut child, process_group_id).await;

    for (stream, decoder) in [ToolOutputStream::Stdout, ToolOutputStream::Stderr]
        .into_iter()
        .zip(decoders.iter_mut())
    {
        let text = decoder.finish();
        push_decoded(stream, text, &mut capture, &mut pending);
    }
    if !pending.is_empty() {
        ctx.progress_reporter.report(pending.take());
    }
    let _ = stdout_task.await;
    let _ = stderr_task.await;

    if let Some(reason) = failure_reason {
        return Err(error_with_partial(&reason, &capture));
    }
    let status = exit_status.unwrap_or_default();
    if !status.success() {
        let reason = match status.code() {
            Some(code) => format!("exit code {code}"),
            None => "terminated by signal".to_string(),
        };
        return Err(error_with_partial(&reason, &capture));
    }
    let output = capture.take_full_detail();
    let trimmed = output.trim();
    if trimmed.is_empty() {
        Ok("(no output)".to_string())
    } else {
        Ok(trimmed.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool::test_support::{run_tool, test_context};

    #[tokio::test]
    async fn bash_returns_placeholder_for_empty_output() {
        let context = test_context("bash_returns_placeholder_for_empty_output");

        let output = run_tool(
            &context,
            BashTool,
            "bash",
            serde_json::json!({ "command": "true" }),
        )
        .await
        .unwrap();

        assert_eq!(output, "(no output)");
    }

    #[tokio::test]
    async fn bash_fails_on_nonzero_exit_and_keeps_output() {
        let context = test_context("bash_nonzero_exit");

        let error = run_tool(
            &context,
            BashTool,
            "bash",
            serde_json::json!({ "command": "printf 'boom\\n' >&2; exit 7" }),
        )
        .await
        .unwrap_err()
        .to_string();

        assert!(
            error.contains("exit code 7"),
            "expected exit code in error: {error}"
        );
        assert!(
            error.contains("boom"),
            "expected partial stderr in error: {error}"
        );
    }

    #[tokio::test]
    async fn bash_zero_exit_still_succeeds() {
        let context = test_context("bash_zero_exit");

        let output = run_tool(
            &context,
            BashTool,
            "bash",
            serde_json::json!({ "command": "printf 'ok\\n'" }),
        )
        .await
        .unwrap();

        assert_eq!(output, "ok");
    }

    #[tokio::test]
    async fn bash_uses_configured_timeout_and_preserves_partial_output() {
        let mut context = test_context("bash_uses_configured_timeout");
        context.bash_timeout_secs = 1;

        let error = run_tool(
            &context,
            BashTool,
            "bash",
            serde_json::json!({ "command": "printf 'started\\n'; sleep 5" }),
        )
        .await
        .unwrap_err()
        .to_string();

        assert!(error.contains("Timeout (1s)"), "unexpected error: {error}");
        assert!(error.contains("started"), "partial output missing: {error}");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn orphaned_background_grandchild_does_not_hang() {
        // Regression: after the shell exits, a background grandchild that
        // inherited stdout/stderr pipe fds kept them open, causing a permanent
        // hang in the select loop (child.wait() returned, but closed_pipes
        // could never reach 2). Now the child.wait() arm kills the process
        // group when pipes are still held, so the loop can finish.
        let context = test_context("bash_orphaned_grandchild");
        let done = tokio::time::timeout(
            Duration::from_secs(3),
            bash(
                context,
                BashInput {
                    command: "sh -c 'sleep 2 &'".to_string(),
                },
            ),
        )
        .await;
        match done {
            Ok(Ok(_output)) => {} // clean completion — no hang
            Ok(Err(e)) if e.to_string().contains("Cancelled") => {
                // Acceptable: the kill sends before cancelled; some
                // output may trigger the cancel-poll to set a reason.
            }
            Ok(Err(e)) => panic!("unexpected error: {e}"),
            Err(_elapsed) => panic!("hung waiting for orphaned grandchild"),
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn cancellation_kills_long_running_process() {
        let context = test_context("bash_cancel_long_running");
        let cancel_flag = context.cancel_flag.clone();
        let mut task = tokio::spawn(bash(
            context,
            BashInput {
                command: "sleep 10".to_string(),
            },
        ));

        tokio::time::sleep(Duration::from_millis(150)).await;
        cancel_flag.store(true, Ordering::Relaxed);
        let result = tokio::time::timeout(Duration::from_millis(500), &mut task).await;
        if result.is_err() {
            task.abort();
        }
        let error = result
            .expect("cancellation should have terminated the sleep")
            .unwrap()
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("Cancelled by user"),
            "unexpected error: {error}"
        );
    }

    #[tokio::test]
    async fn regular_progress_waits_after_the_immediate_first_batch() {
        let context = test_context("bash_progress_interval");
        let mut pending = PendingProgress::default();
        pending.push(ToolOutputChunk::stdout("first\n"));
        let first_tick = tokio::time::Instant::now() + Duration::from_millis(40);
        let mut progress_tick = tokio::time::interval_at(first_tick, PROGRESS_INTERVAL);
        progress_tick.set_missed_tick_behavior(MissedTickBehavior::Skip);

        progress_tick.tick().await;
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert!(report_pending(&context, &mut pending, &mut progress_tick));
        let sent_at = tokio::time::Instant::now();
        progress_tick.tick().await;
        let gap = sent_at.elapsed();

        assert!(
            gap >= Duration::from_millis(40),
            "regular progress became eligible only {gap:?} after the immediate batch"
        );
    }
}
