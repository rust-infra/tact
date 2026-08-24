//! Background (asynchronous) task execution.
//!
//! Background tasks run shell commands asynchronously via `tokio::spawn`.
//! Results are persisted to `<workdir>/.tact/tact.db` (the
//! `background_tasks` table) and can be polled at any time.
//!
//! - [`BackgroundManager`] owns the in-memory id source and the SQLite
//!   store.
//! - [`SharedBackgroundManager`] is the thread-safe wrapper used by tool
//!   implementations.
//! - [`BackgroundTaskRecord`] captures the command, status, start/finish
//!   timestamps, combined stdout+stderr output, and the full-output log
//!   file path.
//!
//! Output is stored hybrid: the DB record keeps the metadata plus the
//! first [`MAX_OUTPUT_CHARS`] chars (bounded, cheap to poll), while the
//! **full** stdout+stderr stream is appended to
//! `<workdir>/.tact/background/<id>.log` as it arrives. The `output_path`
//! field lets the agent (or a human) `tail` / `grep` the full log with the
//! `bash` tool instead of pulling a 50k JSON blob into context.

use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tact_protocol::{AgentUpdate, ToolOutputChunk, ToolOutputStream};
use tokio::{
    io::AsyncWriteExt,
    process::Command,
    sync::mpsc,
    time::{MissedTickBehavior, interval},
};

use crate::{
    pipe_stream::{
        PIPE_CHANNEL_CAPACITY, PROGRESS_INTERVAL, PendingProgress, PipeEvent, Utf8Decoder,
        read_pipe, stream_index,
    },
    store::background_store::{BackgroundStore, SqliteBackgroundStore},
};

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
    #[serde(default)]
    pub session_id: String,
    pub started_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub output: String,
    /// Full-output log file (`<workdir>/.tact/background/<id>.log`).
    /// `None` only if the log file could not be created.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_path: Option<String>,
}

pub struct BackgroundManager {
    records: Arc<dyn BackgroundStore>,
    next_id: AtomicU64,
}

impl std::fmt::Debug for BackgroundManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BackgroundManager")
            .field("next_id", &self.next_id)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Debug)]
pub struct SharedBackgroundManager {
    inner: Arc<BackgroundManager>,
}

impl BackgroundManager {
    /// Creates a manager backed by the given SQLite database file.
    ///
    /// Repairs orphans on startup: any record still marked `running`
    /// belongs to a process that no longer exists, so it is rewritten as an
    /// error (`"Process interrupted (agent restarted)"`).
    pub async fn new(db_path: &Path) -> Result<Self> {
        let records: Arc<dyn BackgroundStore> =
            Arc::new(SqliteBackgroundStore::new(db_path).await?);
        for mut record in records.list().await? {
            if record.status == BackgroundTaskStatus::Running {
                record.status = BackgroundTaskStatus::Error;
                record.finished_at = Some(Utc::now());
                record.output = "Process interrupted (agent restarted)".to_string();
                records.upsert(&record).await?;
            }
        }

        Ok(Self {
            records,
            next_id: AtomicU64::new(Utc::now().timestamp_millis().max(0) as u64),
        })
    }

    pub async fn run(
        &self,
        command: String,
        work_dir: &Path,
        session_id: String,
        progress: Option<BackgroundProgressSink>,
    ) -> Result<String> {
        crate::shell::validate_shell_command(&command)?;

        let id = format!("{:08x}", self.next_id.fetch_add(1, Ordering::Relaxed));
        let log_path = work_dir
            .join(".tact")
            .join("background")
            .join(format!("{id}.log"));
        let record = BackgroundTaskRecord {
            id: id.clone(),
            status: BackgroundTaskStatus::Running,
            command: command.clone(),
            session_id,
            started_at: Utc::now(),
            finished_at: None,
            output: String::new(),
            output_path: Some(log_path.to_string_lossy().into_owned()),
        };
        self.save_record(record.clone()).await?;

        let manager = self.records.clone();
        let command_for_task = command.clone();
        let work_dir = work_dir.to_path_buf();
        let task_id = id.clone();
        tokio::spawn(async move {
            let (status, output) =
                run_background_process(&command_for_task, &work_dir, &progress, Some(&log_path))
                    .await;
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
            let _ = manager.upsert(&record).await;
        });

        Ok(format!("Background task {id} started: {command}"))
    }

    pub async fn check(&self, task_id: Option<&str>) -> Result<String> {
        if let Some(task_id) = task_id {
            let record = self
                .records
                .get(task_id)
                .await?
                .with_context(|| format!("Unknown background task {task_id}"))?;
            return serde_json::to_string_pretty(&record).context("failed to serialize task");
        }

        let mut records = self.records.list().await?;
        if records.is_empty() {
            return Ok("No background tasks.".to_string());
        }
        records.sort_by_key(|record| record.started_at);
        Ok(records
            .into_iter()
            .map(|record| {
                let mut line = format!("{}: {:?} {}", record.id, record.status, record.command);
                if let Some(path) = &record.output_path {
                    line.push_str(&format!(" (log: {path})"));
                }
                line
            })
            .collect::<Vec<_>>()
            .join("\n"))
    }

    async fn save_record(&self, record: BackgroundTaskRecord) -> Result<()> {
        self.records.upsert(&record).await
    }
}

impl SharedBackgroundManager {
    pub fn new(manager: BackgroundManager) -> Self {
        Self {
            inner: Arc::new(manager),
        }
    }

    pub async fn run(
        &self,
        command: String,
        work_dir: &Path,
        session_id: String,
        progress: Option<BackgroundProgressSink>,
    ) -> Result<String> {
        self.inner
            .run(command, work_dir, session_id, progress)
            .await
    }

    pub async fn check(&self, task_id: Option<&str>) -> Result<String> {
        self.inner.check(task_id).await
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

/// Best-effort creation of the full-output log file. Failures are logged and
/// degrade to "no log file" — the DB record and live TUI stream still work.
async fn open_log_file(log_path: &Path) -> Option<tokio::fs::File> {
    if let Some(dir) = log_path.parent()
        && let Err(error) = tokio::fs::create_dir_all(dir).await
    {
        tracing::warn!(
            "background: failed to create log dir {}: {error}",
            dir.display()
        );
        return None;
    }
    match tokio::fs::File::create(log_path).await {
        Ok(file) => Some(file),
        Err(error) => {
            tracing::warn!(
                "background: failed to create log file {}: {error}",
                log_path.display()
            );
            None
        }
    }
}

/// Appends decoded text to the log file. Drops the file handle on the first
/// write error so a broken file is not retried for the whole task lifetime.
async fn log_write(file: &mut Option<tokio::fs::File>, text: &str) {
    let Some(f) = file.as_mut() else {
        return;
    };
    if f.write_all(text.as_bytes()).await.is_ok() {
        return;
    }
    tracing::warn!("background: log file write failed; disabling log file");
    *file = None;
}

/// Run `sh -c command` in the background, streaming stdout/stderr as live
/// `ToolProgress` updates when a sink is present, appending the **full**
/// output to `log_path` when given, and returning the final
/// `(status, output)` for the persisted record (`output` is capped at
/// [`MAX_OUTPUT_CHARS`]; the log file holds everything).
async fn run_background_process(
    command: &str,
    work_dir: &Path,
    progress: &Option<BackgroundProgressSink>,
    log_path: Option<&Path>,
) -> (BackgroundTaskStatus, String) {
    let mut log_file = match log_path {
        Some(path) => open_log_file(path).await,
        None => None,
    };
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
                        log_write(&mut log_file, &text).await;
                        pending.push(ToolOutputChunk {
                            stream,
                            kind: None,
                            text,
                        });
                    }
                    Some(PipeEvent::Closed(stream)) => {
                        let text = decoders[stream_index(stream)].finish();
                        record.push(&text);
                        log_write(&mut log_file, &text).await;
                        pending.push(ToolOutputChunk {
                            stream,
                            kind: None,
                            text,
                        });
                        closed_pipes += 1;
                    }
                    Some(PipeEvent::Failed(stream, error)) => {
                        let text = decoders[stream_index(stream)].finish();
                        record.push(&text);
                        log_write(&mut log_file, &text).await;
                        pending.push(ToolOutputChunk {
                            stream,
                            kind: None,
                            text,
                        });
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
        log_write(&mut log_file, &format!("\n[{reason}]\n")).await;
        let output = if partial.trim().is_empty() {
            reason
        } else {
            format!("{reason}\n\nPartial output:\n{}", partial.trim())
        };
        return (BackgroundTaskStatus::Error, output);
    }

    let output = record.into_string();
    if let Some(f) = log_file.as_mut() {
        let _ = f.flush().await;
    }
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
    use crate::store::background_store::SqliteBackgroundStore;

    fn temp_manager(_name: &str) -> (SharedBackgroundManager, TempDir) {
        let tmp = TempDir::new().unwrap();
        let db = tmp.path().join("tact.db");
        let manager = SharedBackgroundManager::new(block_on(BackgroundManager::new(&db)).unwrap());
        (manager, tmp)
    }

    fn block_on<F>(future: F) -> F::Output
    where
        F: std::future::Future + Send,
        F::Output: Send,
    {
        std::thread::scope(|scope| {
            scope
                .spawn(|| {
                    tokio::runtime::Runtime::new()
                        .expect("failed to create tokio runtime")
                        .block_on(future)
                })
                .join()
                .expect("block_on thread panicked")
        })
    }

    #[tokio::test]
    async fn marks_stale_running_tasks_on_startup() {
        let tmp = TempDir::new().unwrap();
        let db = tmp.path().join("tact.db");
        let store = SqliteBackgroundStore::new(&db).await.unwrap();
        store
            .upsert(&BackgroundTaskRecord {
                id: "deadbeef".to_string(),
                status: BackgroundTaskStatus::Running,
                command: "sleep 999".to_string(),
                session_id: String::new(),
                started_at: Utc::now(),
                finished_at: None,
                output: String::new(),
                output_path: None,
            })
            .await
            .unwrap();

        let manager = SharedBackgroundManager::new(BackgroundManager::new(&db).await.unwrap());
        let output = manager.check(Some("deadbeef")).await.unwrap();

        assert!(output.contains("error"));
        assert!(output.contains("Process interrupted (agent restarted)"));
    }

    #[tokio::test]
    async fn run_streams_progress_and_emits_finished_event() {
        let (manager, tmp) = temp_manager("run_streams_progress");
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let progress = BackgroundProgressSink::new("bg-test", Some(tx));

        let started = manager
            .run(
                "echo hello-world".to_string(),
                tmp.path(),
                "sess-1".to_string(),
                Some(progress),
            )
            .await
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

        // The persisted record reflects the completion (the DB write happens
        // after the finished event, so poll briefly).
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        let mut listing = String::new();
        while tokio::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(20)).await;
            listing = manager.check(None).await.unwrap();
            if listing.contains("Completed") {
                break;
            }
        }
        assert!(listing.contains("Completed"), "listing: {listing}");
    }

    #[tokio::test]
    async fn run_failed_command_emits_error_finished_event() {
        let (manager, tmp) = temp_manager("run_failed_command");
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let progress = BackgroundProgressSink::new("bg-fail", Some(tx));

        manager
            .run(
                "echo before-fail && false".to_string(),
                tmp.path(),
                String::new(),
                Some(progress),
            )
            .await
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
        let (manager, tmp) = temp_manager("run_without_sink");

        manager
            .run(
                "echo persisted-output".to_string(),
                tmp.path(),
                String::new(),
                None,
            )
            .await
            .unwrap();

        // Poll until the spawned task writes the completed record back.
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        let mut listing = String::new();
        while tokio::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(20)).await;
            listing = manager.check(None).await.unwrap();
            if listing.contains("Completed") {
                break;
            }
        }
        assert!(listing.contains("Completed"), "listing: {listing}");
    }

    #[tokio::test]
    async fn run_writes_full_output_to_log_file_and_truncates_db_record() {
        let (manager, tmp) = temp_manager("run_writes_log_file");
        // ~66k chars: more than MAX_OUTPUT_CHARS, so the DB record keeps the
        // first 50k while the log file must hold everything.
        manager
            .run(
                "awk 'BEGIN { for (i = 0; i < 6000; i++) print \"0123456789\" }'".to_string(),
                tmp.path(),
                String::new(),
                None,
            )
            .await
            .unwrap();

        // Poll until the spawned task writes the completed record back.
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        let mut record = None;
        while tokio::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(20)).await;
            let mut records = manager.inner.records.list().await.unwrap();
            if let Some(r) = records.pop()
                && r.status == BackgroundTaskStatus::Completed
            {
                record = Some(r);
                break;
            }
        }
        let record = record.expect("background task never completed");

        assert_eq!(record.output.chars().count(), MAX_OUTPUT_CHARS);
        let log_path = record.output_path.expect("output_path must be set");
        assert!(log_path.ends_with(".log"), "log path: {log_path}");
        let full = tokio::fs::read_to_string(&log_path).await.unwrap();
        assert_eq!(full.chars().count(), 6000 * 11); // "0123456789\n" per line
        assert!(full.starts_with(&record.output));
    }

    #[tokio::test]
    async fn run_persists_session_id() {
        let (manager, tmp) = temp_manager("run_persists_session_id");

        manager
            .run(
                "echo persisted-session".to_string(),
                tmp.path(),
                "sess-42".to_string(),
                None,
            )
            .await
            .unwrap();

        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        let mut output = String::new();
        while tokio::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(20)).await;
            output = manager.check(None).await.unwrap();
            if output.contains("Completed") {
                break;
            }
        }
        assert!(output.contains("Completed"), "listing: {output}");
        // The persisted record carries the session id.
        let raw = manager.inner.records.list().await.unwrap();
        let record = raw
            .iter()
            .find(|r| r.command.contains("persisted-session"))
            .unwrap();
        assert_eq!(record.session_id, "sess-42");
    }
}
