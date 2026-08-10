//! Background (asynchronous) task execution.
//!
//! Background tasks run shell commands asynchronously via `tokio::spawn`.
//! Results are persisted to disk and can be polled at any time.
//!
//! - [`BackgroundManager`] owns the in-memory task map and the on-disk
//!   collection store.
//! - [`SharedBackgroundManager`] is the thread-safe wrapper used by tool
//!   implementations.
//! - [`BackgroundTaskRecord`] captures the command, status, start/finish
//!   timestamps, and combined stdout+stderr output.

use std::{
    collections::{HashMap, VecDeque},
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tact_protocol::{AgentUpdate, ToolOutputChunk, ToolOutputStream};
use tokio::{
    io::{AsyncRead, AsyncReadExt},
    process::Command,
    sync::mpsc,
    time::{MissedTickBehavior, interval},
};

use crate::store::{CollectionStore, StoreRoot};

const READ_BUFFER_BYTES: usize = 4096;
const PIPE_CHANNEL_CAPACITY: usize = 32;
const MAX_PROGRESS_BYTES: usize = 4096;
const OMITTED_MARKER: &str = "[intermediate output omitted]\n";
const PROGRESS_INTERVAL: Duration = Duration::from_millis(50);
const MAX_OUTPUT_CHARS: usize = 50_000;
const COMMAND_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackgroundTaskStatus {
    Running,
    Completed,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackgroundTaskRecord {
    pub id: String,
    pub status: BackgroundTaskStatus,
    pub command: String,
    pub started_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub output: String,
}

#[derive(Debug)]
pub struct BackgroundManager {
    records: CollectionStore<BackgroundTaskRecord>,
    tasks: Mutex<HashMap<String, BackgroundTaskRecord>>,
    next_id: AtomicU64,
}

#[derive(Clone, Debug)]
pub struct SharedBackgroundManager {
    inner: Arc<BackgroundManager>,
}

impl SharedBackgroundManager {
    pub fn new(root: &StoreRoot) -> Result<Self> {
        let records = root.collection::<BackgroundTaskRecord>("background/tasks")?;
        let mut tasks = HashMap::new();
        for mut record in records.list()? {
            if record.status == BackgroundTaskStatus::Running {
                record.status = BackgroundTaskStatus::Error;
                record.finished_at = Some(Utc::now());
                record.output = "Process interrupted (agent restarted)".to_string();
                records.write(&record.id, &record)?;
            }
            tasks.insert(record.id.clone(), record);
        }

        Ok(Self {
            inner: Arc::new(BackgroundManager {
                records,
                tasks: Mutex::new(tasks),
                next_id: AtomicU64::new(Utc::now().timestamp_millis().max(0) as u64),
            }),
        })
    }

    pub fn run(
        &self,
        command: String,
        work_dir: &Path,
        progress: Option<BackgroundProgressSink>,
    ) -> Result<String> {
        crate::shell::validate_shell_command(&command)?;

        let id = format!("{:08x}", self.inner.next_id.fetch_add(1, Ordering::Relaxed));
        let record = BackgroundTaskRecord {
            id: id.clone(),
            status: BackgroundTaskStatus::Running,
            command: command.clone(),
            started_at: Utc::now(),
            finished_at: None,
            output: String::new(),
        };
        self.save_record(record.clone())?;

        let manager = self.clone();
        let command_for_task = command.clone();
        let work_dir = work_dir.to_path_buf();
        let task_id = id.clone();
        tokio::spawn(async move {
            let (status, output) =
                run_background_process(&command_for_task, &work_dir, &progress).await;
            let mut record = record;
            record.finished_at = Some(Utc::now());
            record.status = status;
            record.output = output;
            let success = record.status == BackgroundTaskStatus::Completed;
            let message = format!(
                "Background task {task_id} {}",
                if success { "completed" } else { "failed" }
            );
            if let Some(progress) = &progress {
                progress.send_finished(success, &message, &record.output);
            }
            let _ = manager.save_record(record);
        });

        Ok(format!("Background task {id} started: {command}"))
    }

    pub fn check(&self, task_id: Option<&str>) -> Result<String> {
        let mut tasks = self
            .inner
            .tasks
            .lock()
            .map_err(|_| anyhow::anyhow!("background manager lock poisoned"))?;

        if let Some(task_id) = task_id {
            let record = tasks
                .get(task_id)
                .cloned()
                .or_else(|| self.inner.records.read(task_id).ok())
                .with_context(|| format!("Unknown background task {task_id}"))?;
            return serde_json::to_string_pretty(&record).context("failed to serialize task");
        }

        if tasks.is_empty() {
            for record in self.inner.records.list()? {
                tasks.insert(record.id.clone(), record);
            }
        }

        if tasks.is_empty() {
            return Ok("No background tasks.".to_string());
        }
        let mut records = tasks.values().cloned().collect::<Vec<_>>();
        records.sort_by_key(|record| record.started_at);
        Ok(records
            .into_iter()
            .map(|record| format!("{}: {:?} {}", record.id, record.status, record.command))
            .collect::<Vec<_>>()
            .join("\n"))
    }

    fn save_record(&self, record: BackgroundTaskRecord) -> Result<()> {
        self.inner.records.write(&record.id, &record)?;
        let mut tasks = self
            .inner
            .tasks
            .lock()
            .map_err(|_| anyhow::anyhow!("background manager lock poisoned"))?;
        tasks.insert(record.id.clone(), record);
        Ok(())
    }
}

impl std::ops::Deref for SharedBackgroundManager {
    type Target = Arc<BackgroundManager>;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

/// Handle for a `background_run` invocation to stream live output into the
/// TUI tool card while the task runs, then finalize it on completion.
#[derive(Clone, Debug)]
pub struct BackgroundProgressSink {
    tool_id: String,
    ui_tx: Option<tokio::sync::mpsc::UnboundedSender<AgentUpdate>>,
}

impl BackgroundProgressSink {
    pub fn new(
        tool_id: impl Into<String>,
        ui_tx: Option<tokio::sync::mpsc::UnboundedSender<AgentUpdate>>,
    ) -> Self {
        Self {
            tool_id: tool_id.into(),
            ui_tx,
        }
    }

    fn send_progress(&self, chunks: Vec<ToolOutputChunk>) {
        if chunks.is_empty() {
            return;
        }
        if let Some(tx) = &self.ui_tx {
            let _ = tx.send(AgentUpdate::ToolProgress {
                tool_id: self.tool_id.clone(),
                chunks,
            });
        }
    }

    fn send_finished(&self, success: bool, message: &str, output: &str) {
        if let Some(tx) = &self.ui_tx {
            let _ = tx.send(AgentUpdate::BackgroundTaskFinished {
                tool_id: self.tool_id.clone(),
                success,
                message: message.to_string(),
                output: output.to_string(),
            });
        }
    }
}

/// Incremental UTF-8 decoder that survives bytes split across pipe reads.
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
                }
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
                }
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
            }
            Ok(read) => {
                if tx
                    .send(PipeEvent::Bytes(stream, buffer[..read].to_vec()))
                    .await
                    .is_err()
                {
                    return;
                }
            }
            Err(error) => {
                let _ = tx.send(PipeEvent::Failed(stream, error)).await;
                return;
            }
        }
    }
}

fn stream_index(stream: ToolOutputStream) -> usize {
    match stream {
        ToolOutputStream::Stdout => 0,
        ToolOutputStream::Stderr => 1,
        ToolOutputStream::Other => 2,
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

/// Capped buffer of decoded output text (keeps the *first* characters, matching
/// the pre-streaming record semantics).
#[derive(Default)]
struct OutputAccumulator {
    text: String,
    chars: usize,
    truncated: bool,
}

impl OutputAccumulator {
    fn push(&mut self, text: &str) {
        if self.truncated || text.is_empty() {
            return;
        }
        let add = text.chars().count();
        if self.chars + add <= MAX_OUTPUT_CHARS {
            self.text.push_str(text);
            self.chars += add;
        } else {
            let remaining = MAX_OUTPUT_CHARS.saturating_sub(self.chars);
            self.text.extend(text.chars().take(remaining));
            self.chars = MAX_OUTPUT_CHARS;
            self.truncated = true;
        }
    }

    fn into_string(self) -> String {
        self.text
    }
}

/// Coalesced progress batch awaiting the next flush tick.
#[derive(Default)]
struct PendingProgress {
    chunks: VecDeque<ToolOutputChunk>,
    bytes: usize,
    omitted: bool,
}

impl PendingProgress {
    fn push(&mut self, chunk: ToolOutputChunk) {
        if chunk.text.is_empty() {
            return;
        }
        let data_limit = MAX_PROGRESS_BYTES.saturating_sub(OMITTED_MARKER.len());
        if chunk.text.len() > data_limit {
            let mut chunk = chunk;
            chunk.text = utf8_tail(&chunk.text, data_limit).to_string();
            self.chunks.clear();
            self.bytes = 0;
            self.omitted = true;
            self.bytes += chunk.text.len();
            self.chunks.push_back(chunk);
            return;
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

/// Run `sh -c command` in the background, streaming stdout/stderr as live
/// `ToolProgress` updates when a sink is present, and returning the final
/// `(status, output)` for the persisted record.
async fn run_background_process(
    command: &str,
    work_dir: &Path,
    progress: &Option<BackgroundProgressSink>,
) -> (BackgroundTaskStatus, String) {
    let mut child = match Command::new("sh")
        .arg("-c")
        .arg(command)
        .current_dir(work_dir)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
    {
        Ok(child) => child,
        Err(error) => {
            return (
                BackgroundTaskStatus::Error,
                format!("Failed to spawn: {error}"),
            );
        }
    };
    let Some(stdout) = child.stdout.take() else {
        return (
            BackgroundTaskStatus::Error,
            "stdout pipe unavailable".to_string(),
        );
    };
    let Some(stderr) = child.stderr.take() else {
        return (
            BackgroundTaskStatus::Error,
            "stderr pipe unavailable".to_string(),
        );
    };
    let (pipe_tx, mut pipe_rx) = mpsc::channel(PIPE_CHANNEL_CAPACITY);
    let stdout_task = tokio::spawn(read_pipe(stdout, ToolOutputStream::Stdout, pipe_tx.clone()));
    let stderr_task = tokio::spawn(read_pipe(stderr, ToolOutputStream::Stderr, pipe_tx));

    let mut decoders = [Utf8Decoder::default(), Utf8Decoder::default()];
    let mut record = OutputAccumulator::default();
    let mut pending = PendingProgress::default();
    let mut progress_tick = interval(PROGRESS_INTERVAL);
    progress_tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
    progress_tick.tick().await;
    let timeout_sleep = tokio::time::sleep(COMMAND_TIMEOUT);
    tokio::pin!(timeout_sleep);
    let mut exit_status: Option<std::process::ExitStatus> = None;
    let mut closed_pipes = 0_usize;
    let mut failure_reason: Option<String> = None;

    while exit_status.is_none() || closed_pipes < 2 {
        tokio::select! {
            event = pipe_rx.recv(), if closed_pipes < 2 => {
                match event {
                    Some(PipeEvent::Bytes(stream, bytes)) => {
                        let text = decoders[stream_index(stream)].push(&bytes);
                        record.push(&text);
                        pending.push(ToolOutputChunk { stream, text });
                    }
                    Some(PipeEvent::Closed(stream)) => {
                        let text = decoders[stream_index(stream)].finish();
                        record.push(&text);
                        pending.push(ToolOutputChunk { stream, text });
                        closed_pipes += 1;
                    }
                    Some(PipeEvent::Failed(stream, error)) => {
                        let text = decoders[stream_index(stream)].finish();
                        record.push(&text);
                        pending.push(ToolOutputChunk { stream, text });
                        closed_pipes += 1;
                        if failure_reason.is_none() {
                            failure_reason = Some(format!("reading {stream:?}: {error}"));
                        }
                    }
                    None => closed_pipes = 2,
                }
            }
            status = child.wait(), if exit_status.is_none() => {
                match status {
                    Ok(status) => exit_status = Some(status),
                    Err(error) => {
                        failure_reason.get_or_insert_with(|| {
                            format!("waiting for command: {error}")
                        });
                        exit_status = Some(std::process::ExitStatus::default());
                    }
                }
            }
            _ = progress_tick.tick() => {
                flush_progress(progress, &mut pending);
            }
            _ = &mut timeout_sleep, if failure_reason.is_none() => {
                failure_reason = Some(format!("Timeout ({COMMAND_TIMEOUT:?})"));
                let _ = child.kill().await;
                stdout_task.abort();
                stderr_task.abort();
                closed_pipes = 2;
            }
        }
    }
    flush_progress(progress, &mut pending);

    if let Some(reason) = failure_reason {
        let partial = record.into_string();
        let output = if partial.trim().is_empty() {
            reason
        } else {
            format!("{reason}\n\nPartial output:\n{}", partial.trim())
        };
        return (BackgroundTaskStatus::Error, output);
    }

    let output = record.into_string();
    match exit_status {
        Some(status) if status.success() => (BackgroundTaskStatus::Completed, output),
        Some(_) => (BackgroundTaskStatus::Error, output),
        None => (
            BackgroundTaskStatus::Error,
            "Process exited without a status".to_string(),
        ),
    }
}

fn flush_progress(progress: &Option<BackgroundProgressSink>, pending: &mut PendingProgress) {
    if pending.is_empty() {
        return;
    }
    if let Some(sink) = progress {
        sink.send_progress(pending.take());
    } else {
        let _ = pending.take();
    }
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;
    use crate::store::StoreRoot;

    #[test]
    fn marks_stale_running_tasks_on_startup() {
        let tmp = TempDir::new().unwrap();
        let root = StoreRoot::new(tmp.path()).unwrap();
        let records = root
            .collection::<BackgroundTaskRecord>("background/tasks")
            .unwrap();
        records
            .write(
                "deadbeef",
                &BackgroundTaskRecord {
                    id: "deadbeef".to_string(),
                    status: BackgroundTaskStatus::Running,
                    command: "sleep 999".to_string(),
                    started_at: Utc::now(),
                    finished_at: None,
                    output: String::new(),
                },
            )
            .unwrap();

        let manager = SharedBackgroundManager::new(&root).unwrap();
        let output = manager.check(Some("deadbeef")).unwrap();

        assert!(output.contains("error"));
        assert!(output.contains("Process interrupted (agent restarted)"));
    }

    #[tokio::test]
    async fn run_streams_progress_and_emits_finished_event() {
        let tmp = TempDir::new().unwrap();
        let root = StoreRoot::new(tmp.path()).unwrap();
        let manager = SharedBackgroundManager::new(&root).unwrap();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let progress = BackgroundProgressSink::new("bg-test", Some(tx));

        let started = manager
            .run("echo hello-world".to_string(), tmp.path(), Some(progress))
            .unwrap();
        assert!(started.contains("Background task"));

        let mut saw_progress = false;
        loop {
            let Ok(update) = tokio::time::timeout(Duration::from_secs(5), rx.recv()).await else {
                panic!("timed out waiting for background task events");
            };
            match update {
                Some(AgentUpdate::ToolProgress { tool_id, chunks }) => {
                    assert_eq!(tool_id, "bg-test");
                    let text: String = chunks.iter().map(|c| c.text.as_str()).collect();
                    assert!(text.contains("hello-world"), "progress text: {text:?}");
                    saw_progress = true;
                }
                Some(AgentUpdate::BackgroundTaskFinished {
                    tool_id,
                    success,
                    message,
                    output,
                }) => {
                    assert_eq!(tool_id, "bg-test");
                    assert!(success, "echo should succeed");
                    assert!(message.contains("completed"), "message: {message}");
                    assert!(output.contains("hello-world"), "output: {output:?}");
                    break;
                }
                other => panic!("unexpected update: {other:?}"),
            }
        }
        assert!(saw_progress, "expected live ToolProgress before finish");

        // The persisted record reflects the completion.
        let listing = manager.check(None).unwrap();
        assert!(listing.contains("Completed"), "listing: {listing}");
    }

    #[tokio::test]
    async fn run_failed_command_emits_error_finished_event() {
        let tmp = TempDir::new().unwrap();
        let root = StoreRoot::new(tmp.path()).unwrap();
        let manager = SharedBackgroundManager::new(&root).unwrap();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let progress = BackgroundProgressSink::new("bg-fail", Some(tx));

        manager
            .run(
                "echo before-fail && false".to_string(),
                tmp.path(),
                Some(progress),
            )
            .unwrap();

        // Live progress may arrive before the completion event; consume it.
        loop {
            let Ok(update) = tokio::time::timeout(Duration::from_secs(5), rx.recv()).await else {
                panic!("timed out waiting for background task events");
            };
            match update {
                Some(AgentUpdate::ToolProgress { tool_id, chunks }) => {
                    assert_eq!(tool_id, "bg-fail");
                    let text: String = chunks.iter().map(|c| c.text.as_str()).collect();
                    assert!(text.contains("before-fail"), "progress text: {text:?}");
                }
                Some(AgentUpdate::BackgroundTaskFinished {
                    tool_id,
                    success,
                    message,
                    output,
                }) => {
                    assert_eq!(tool_id, "bg-fail");
                    assert!(!success, "`false` must finish as failed");
                    assert!(message.contains("failed"), "message: {message}");
                    assert!(output.contains("before-fail"), "output: {output:?}");
                    break;
                }
                other => panic!("unexpected update: {other:?}"),
            }
        }
    }

    #[tokio::test]
    async fn run_without_sink_persists_record() {
        let tmp = TempDir::new().unwrap();
        let root = StoreRoot::new(tmp.path()).unwrap();
        let manager = SharedBackgroundManager::new(&root).unwrap();

        manager
            .run("echo persisted-output".to_string(), tmp.path(), None)
            .unwrap();

        // Poll until the spawned task writes the completed record back.
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        let mut listing = String::new();
        while tokio::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(20)).await;
            listing = manager.check(None).unwrap();
            if listing.contains("Completed") {
                break;
            }
        }
        assert!(listing.contains("Completed"), "listing: {listing}");
    }
}
