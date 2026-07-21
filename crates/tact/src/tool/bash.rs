use std::{collections::VecDeque, sync::atomic::Ordering, time::Duration};

use anyhow::Result;
use schemars::JsonSchema;
use serde::Deserialize;
use tact_protocol::{ToolOutputBuffer, ToolOutputChunk, ToolOutputStream};
use tokio::{
    io::{AsyncRead, AsyncReadExt},
    process::{Child, Command},
    sync::mpsc,
    time::{Interval, MissedTickBehavior, interval},
};
use tool_refactor_macros::tool;

use crate::{shell::validate_shell_command, tool::ToolContext};

const READ_BUFFER_BYTES: usize = 4096;
const PIPE_CHANNEL_CAPACITY: usize = 32;
const MAX_PROGRESS_BYTES: usize = 4096;
const PROGRESS_INTERVAL: Duration = Duration::from_millis(50);
const OUTPUT_LIMIT_CHARS: usize = 50_000;
const OMITTED_MARKER: &str = "[intermediate output omitted]\n";

#[derive(Default)]
struct Utf8Decoder {
    pending: Vec<u8>,
}

impl Utf8Decoder {
    fn push(&mut self, bytes: &[u8]) -> String {
        self.pending.extend_from_slice(bytes);
        let mut output = String::new();
        loop {
            match std::str::from_utf8(&self.pending) {
                Ok(valid) => {
                    output.push_str(valid);
                    self.pending.clear();
                    break;
                },
                Err(error) => {
                    let valid_up_to = error.valid_up_to();
                    if valid_up_to > 0 {
                        let valid = std::str::from_utf8(&self.pending[..valid_up_to])
                            .expect("valid_up_to identifies valid UTF-8");
                        output.push_str(valid);
                        self.pending.drain(..valid_up_to);
                    }
                    let Some(error_len) = error.error_len() else {
                        break;
                    };
                    output.push('\u{fffd}');
                    self.pending.drain(..error_len);
                },
            }
        }
        output
    }

    fn finish(&mut self) -> String {
        let output = String::from_utf8_lossy(&self.pending).into_owned();
        self.pending.clear();
        output
    }
}

enum PipeEvent {
    Bytes(ToolOutputStream, Vec<u8>),
    Closed(ToolOutputStream),
    Failed(ToolOutputStream, std::io::Error),
}

async fn read_pipe<R>(mut reader: R, stream: ToolOutputStream, tx: mpsc::Sender<PipeEvent>)
where
    R: AsyncRead + Unpin,
{
    let mut buffer = [0_u8; READ_BUFFER_BYTES];
    loop {
        match reader.read(&mut buffer).await {
            Ok(0) => {
                let _ = tx.send(PipeEvent::Closed(stream)).await;
                return;
            },
            Ok(read) => {
                if tx.send(PipeEvent::Bytes(stream, buffer[..read].to_vec())).await.is_err() {
                    return;
                }
            },
            Err(error) => {
                let _ = tx.send(PipeEvent::Failed(stream, error)).await;
                return;
            },
        }
    }
}

#[derive(Default)]
struct PendingProgress {
    chunks: VecDeque<ToolOutputChunk>,
    bytes: usize,
    omitted: bool,
}

impl PendingProgress {
    fn push(&mut self, mut chunk: ToolOutputChunk) {
        if chunk.text.is_empty() {
            return;
        }
        let data_limit = MAX_PROGRESS_BYTES.saturating_sub(OMITTED_MARKER.len());
        if chunk.text.len() > data_limit {
            chunk.text = utf8_tail(&chunk.text, data_limit).to_string();
            self.chunks.clear();
            self.bytes = 0;
            self.omitted = true;
        }
        self.bytes += chunk.text.len();
        self.chunks.push_back(chunk);
        while self.bytes > data_limit {
            let Some(removed) = self.chunks.pop_front() else {
                break;
            };
            self.bytes = self.bytes.saturating_sub(removed.text.len());
            self.omitted = true;
        }
    }

    fn take(&mut self) -> Vec<ToolOutputChunk> {
        let mut chunks = Vec::with_capacity(self.chunks.len() + usize::from(self.omitted));
        if self.omitted {
            chunks.push(ToolOutputChunk::other(OMITTED_MARKER));
        }
        chunks.extend(self.chunks.drain(..));
        self.bytes = 0;
        self.omitted = false;
        chunks
    }

    fn is_empty(&self) -> bool {
        self.chunks.is_empty() && !self.omitted
    }
}

fn utf8_tail(text: &str, max_bytes: usize) -> &str {
    if text.len() <= max_bytes {
        return text;
    }
    let mut start = text.len() - max_bytes;
    while !text.is_char_boundary(start) {
        start += 1;
    }
    &text[start..]
}

fn stream_index(stream: ToolOutputStream) -> usize {
    match stream {
        ToolOutputStream::Stdout => 0,
        ToolOutputStream::Stderr => 1,
        ToolOutputStream::Other => 2,
    }
}

fn push_decoded(stream: ToolOutputStream, text: String, capture: &mut ToolOutputBuffer, pending: &mut PendingProgress) {
    if text.is_empty() {
        return;
    }
    let chunk = ToolOutputChunk { stream, text };
    capture.push_chunks(std::slice::from_ref(&chunk));
    pending.push(chunk);
}

fn report_pending(ctx: &ToolContext, pending: &mut PendingProgress, progress_tick: &mut Interval) -> bool {
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

#[cfg(not(unix))]
fn configure_process_group(_command: &mut Command) {}

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
    let partial = capture.detail_text();
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

#[tool(name = "bash", description = "Run a shell command in the current workspace.")]
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
    let mut child = match process.spawn() {
        Ok(c) => c,
        Err(e) => return Err(anyhow::anyhow!("Error: {}", e)),
    };
    let process_group_id = child.id();

    let stdout = child.stdout.take().ok_or_else(|| anyhow::anyhow!("Error: stdout pipe unavailable"))?;
    let stderr = child.stderr.take().ok_or_else(|| anyhow::anyhow!("Error: stderr pipe unavailable"))?;
    let (pipe_tx, mut pipe_rx) = mpsc::channel(PIPE_CHANNEL_CAPACITY);
    let stdout_task = tokio::spawn(read_pipe(stdout, ToolOutputStream::Stdout, pipe_tx.clone()));
    let stderr_task = tokio::spawn(read_pipe(stderr, ToolOutputStream::Stderr, pipe_tx.clone()));
    drop(pipe_tx);

    let mut decoders = [Utf8Decoder::default(), Utf8Decoder::default(), Utf8Decoder::default()];
    let mut capture = ToolOutputBuffer::new(OUTPUT_LIMIT_CHARS);
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

    for (stream, decoder) in [ToolOutputStream::Stdout, ToolOutputStream::Stderr].into_iter().zip(decoders.iter_mut()) {
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
    let output = capture.detail_text();
    let trimmed = output.trim();
    if trimmed.is_empty() { Ok("(no output)".to_string()) } else { Ok(trimmed.to_string()) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool::test_support::{run_tool, test_context};

    #[tokio::test]
    async fn bash_returns_placeholder_for_empty_output() {
        let context = test_context("bash_returns_placeholder_for_empty_output");

        let output = run_tool(&context, BashTool, "bash", serde_json::json!({ "command": "true" })).await.unwrap();

        assert_eq!(output, "(no output)");
    }

    #[test]
    fn utf8_decoder_preserves_characters_split_across_reads() {
        let bytes = "前进".as_bytes();
        for split in 1..bytes.len() {
            let mut decoder = Utf8Decoder::default();
            let mut decoded = decoder.push(&bytes[..split]);
            decoded.push_str(&decoder.push(&bytes[split..]));
            decoded.push_str(&decoder.finish());
            assert_eq!(decoded, "前进", "split at byte {split}");
        }
    }

    #[tokio::test]
    async fn bash_uses_configured_timeout_and_preserves_partial_output() {
        let mut context = test_context("bash_uses_configured_timeout");
        context.bash_timeout_secs = 1;

        let error =
            run_tool(&context, BashTool, "bash", serde_json::json!({ "command": "printf 'started\\n'; sleep 5" }))
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
            bash(context, BashInput { command: "sh -c 'sleep 2 &'".to_string() }),
        )
        .await;
        match done {
            Ok(Ok(_output)) => {}, // clean completion — no hang
            Ok(Err(e)) if e.to_string().contains("Cancelled") => {
                // Acceptable: the kill sends before cancelled; some
                // output may trigger the cancel-poll to set a reason.
            },
            Ok(Err(e)) => panic!("unexpected error: {e}"),
            Err(_elapsed) => panic!("hung waiting for orphaned grandchild"),
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn cancellation_kills_long_running_process() {
        let context = test_context("bash_cancel_long_running");
        let cancel_flag = context.cancel_flag.clone();
        let mut task = tokio::spawn(bash(context, BashInput { command: "sleep 10".to_string() }));

        tokio::time::sleep(Duration::from_millis(150)).await;
        cancel_flag.store(true, Ordering::Relaxed);
        let result = tokio::time::timeout(Duration::from_millis(500), &mut task).await;
        if result.is_err() {
            task.abort();
        }
        let error = result.expect("cancellation should have terminated the sleep").unwrap().unwrap_err().to_string();
        assert!(error.contains("Cancelled by user"), "unexpected error: {error}");
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
