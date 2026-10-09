use std::sync::{Arc, Mutex};
use tact_protocol::RuntimeEvent;

use tact_protocol::{ToolOutputChunk, ToolOutputStream};
use tact_view::AgentUpdate;

use crate::security::{RedactionLevel, redact::StreamRedactor};

/// Sends an already-coalesced progress batch for one tool invocation.
///
/// Holds the streaming half of the redaction pass. The final result is redacted
/// again (and more thoroughly) where it is assembled, so this is the path that
/// keeps a secret from appearing **on screen while the command is still
/// running** — the earlier of the two leaks, and the one a user actually
/// watches happen.
///
/// The redactor is behind a mutex because `report` takes `&self`: the tools hold
/// `&ToolContext`, and a per-invocation buffer cannot be `&mut` through it. The
/// reporter is cloned per invocation, and the `Arc` keeps those clones on one
/// buffer — which is the behaviour we want, since they are one stream.
#[derive(Clone, Debug, Default)]
pub struct ToolProgressReporter {
    tool_id: String,
    #[cfg(any(test, feature = "test-support"))]
    ui_tx: Option<tokio::sync::mpsc::UnboundedSender<AgentUpdate>>,
    view_updates: Option<super::ViewUpdateEmitter>,
    stream: Option<Arc<Mutex<StreamState>>>,
}

/// The per-invocation streaming state: what is held back, and where the last
/// chunk came from — so a flushed tail keeps its origin instead of being
/// relabelled stdout.
#[derive(Debug)]
struct StreamState {
    redactor: StreamRedactor,
    last_stream: ToolOutputStream,
}

impl StreamState {
    fn new() -> Self {
        Self {
            redactor: StreamRedactor::new(RedactionLevel::Basic),
            last_stream: ToolOutputStream::Stdout,
        }
    }
}

impl ToolProgressReporter {
    #[cfg(any(test, feature = "test-support"))]
    pub fn new(
        tool_id: impl Into<String>,
        ui_tx: Option<tokio::sync::mpsc::UnboundedSender<AgentUpdate>>,
    ) -> Self {
        Self {
            tool_id: tool_id.into(),
            ui_tx,
            view_updates: None,
            stream: None,
        }
    }

    pub fn with_view_updates(
        tool_id: impl Into<String>,
        view_updates: super::ViewUpdateEmitter,
    ) -> Self {
        Self {
            tool_id: tool_id.into(),
            #[cfg(any(test, feature = "test-support"))]
            ui_tx: None,
            view_updates: Some(view_updates),
            stream: None,
        }
    }

    fn emit_runtime_event(&self, event: RuntimeEvent) {
        if let Some(view_updates) = &self.view_updates {
            let _ = view_updates.emit_runtime_event(event);
        } else {
            #[cfg(any(test, feature = "test-support"))]
            if let Some(tx) = &self.ui_tx {
                // A harness that has not migrated still reads the legacy view
                // model, so project back for it.
                for update in tact_view::runtime_event_to_agent_updates(event) {
                    let _ = tx.send(update);
                }
            }
        }
    }

    /// Redact live output at `level`, on top of whatever the final result pass
    /// will do.
    ///
    /// [`RedactionLevel::Off`] disables it, matching the config.
    /// [`RedactionLevel::Credential`] is treated as
    /// [`RedactionLevel::Basic`] here: the `Credential` rules are line-anchored
    /// and need a whole line of context a stream may not have delivered yet, and
    /// the final pass over the completed result is what guarantees them.
    #[must_use]
    pub fn with_stream_redaction(mut self, level: RedactionLevel) -> Self {
        if level != RedactionLevel::Off {
            self.stream = Some(Arc::new(Mutex::new(StreamState::new())));
        }
        self
    }

    pub fn report(&self, chunks: Vec<ToolOutputChunk>) {
        if chunks.is_empty() {
            return;
        }
        let chunks = self.redact(chunks);
        if chunks.is_empty() {
            // Everything was held back as a possible partial secret. Emitting an
            // empty batch would just churn the renderer.
            return;
        }
        self.emit_runtime_event(RuntimeEvent::ToolProgress {
            run_id: None,
            tool_id: self.tool_id.clone(),
            chunks,
        });
    }

    /// Emit whatever the redactor is still holding back.
    ///
    /// Must be called when a tool finishes producing output: without it, the
    /// tail of a stream (anything after the last newline) never reaches the
    /// live view. A tool that forgets is still covered by the final result, but
    /// its last line would appear only once the card finalises.
    pub fn flush(&self) {
        let Some(state) = &self.stream else {
            return;
        };
        let mut state = state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let tail = state.redactor.finish();
        let stream = state.last_stream;
        let Some(text) = tail.filter(|t| !t.is_empty()) else {
            return;
        };
        self.emit_runtime_event(RuntimeEvent::ToolProgress {
            run_id: None,
            tool_id: self.tool_id.clone(),
            chunks: vec![ToolOutputChunk { stream, text }],
        });
    }

    fn redact(&self, chunks: Vec<ToolOutputChunk>) -> Vec<ToolOutputChunk> {
        let Some(state) = &self.stream else {
            return chunks;
        };
        let mut state = state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut out = Vec::with_capacity(chunks.len());
        for chunk in chunks {
            state.last_stream = chunk.stream;
            let Some(text) = state.redactor.push(&chunk.text) else {
                continue;
            };
            if text.is_empty() {
                continue;
            }
            out.push(ToolOutputChunk {
                stream: chunk.stream,
                text,
            });
        }
        out
    }

    /// Send an update tied to this invocation.
    ///
    /// For tools that both stream live output and finalize their own card —
    /// `background_run` sends a `BackgroundTaskFinished` carrying the output
    /// tail — so the redaction state and the channel stay in one place instead
    /// of being reimplemented per tool.
    pub fn send(&self, event: RuntimeEvent) {
        self.emit_runtime_event(event);
    }

    /// Redact a complete, final piece of text at the level this reporter
    /// streams at.
    ///
    /// Unlike [`Self::report`], this takes whole text — a settled payload, not a
    /// chunk — so nothing has to be held back.
    #[must_use]
    pub fn redact_text(&self, text: &str) -> String {
        match &self.stream {
            Some(_) => {
                crate::security::redact::redact(text, RedactionLevel::Basic, &[]).into_owned()
            }
            None => text.to_string(),
        }
    }

    /// The tool-invocation id this reporter is bound to.
    pub fn tool_id(&self) -> &str {
        &self.tool_id
    }
}

#[cfg(test)]
mod tests {
    use tact_view::AgentUpdate;

    use super::*;

    fn progress_texts(update: &AgentUpdate) -> Vec<String> {
        match update {
            AgentUpdate::ToolProgress { chunks, .. } => {
                chunks.iter().map(|c| c.text.clone()).collect()
            }
            other => panic!("expected ToolProgress, got {other:?}"),
        }
    }

    #[test]
    fn reporter_binds_progress_to_one_tool_id() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let reporter = ToolProgressReporter::new("bash-7", Some(tx));

        reporter.report(vec![ToolOutputChunk::stdout("hello\n")]);

        assert!(matches!(
            rx.try_recv().unwrap(),
            AgentUpdate::ToolProgress { tool_id, .. } if tool_id == "bash-7"
        ));
    }

    #[test]
    fn reporter_ignores_empty_batches_and_closed_receivers() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let reporter = ToolProgressReporter::new("bash-7", Some(tx));
        reporter.report(Vec::new());
        assert!(rx.try_recv().is_err());

        drop(rx);
        reporter.report(vec![ToolOutputChunk::stdout("ignored")]);
    }

    #[test]
    fn without_redaction_output_is_forwarded_unchanged() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let reporter = ToolProgressReporter::new("bash-7", Some(tx));
        reporter.report(vec![ToolOutputChunk::stdout(
            "sk-7325c3231cef402d8481c32a49c4898a",
        )]);
        assert_eq!(
            progress_texts(&rx.try_recv().unwrap()),
            vec!["sk-7325c3231cef402d8481c32a49c4898a"]
        );
    }

    /// A secret split across two progress batches must never appear whole, and
    /// no batch may be emitted before its line is complete.
    #[test]
    fn a_split_secret_never_reaches_the_live_view() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let reporter = ToolProgressReporter::new("bash-7", Some(tx))
            .with_stream_redaction(RedactionLevel::Basic);

        reporter.report(vec![ToolOutputChunk::stdout("prefix\nsk-7325c32")]);
        assert_eq!(
            progress_texts(&rx.try_recv().unwrap()),
            vec!["prefix\n"],
            "the partial token stays held back"
        );

        reporter.report(vec![ToolOutputChunk::stdout("31cef402d8481c32a49c4898a\n")]);
        assert_eq!(
            progress_texts(&rx.try_recv().unwrap()),
            vec!["[redacted:api-key]\n"]
        );
    }

    #[test]
    fn flush_releases_the_held_tail() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let reporter = ToolProgressReporter::new("bash-7", Some(tx))
            .with_stream_redaction(RedactionLevel::Basic);

        reporter.report(vec![ToolOutputChunk::stderr("no trailing newline")]);
        assert!(rx.try_recv().is_err(), "held back until flush");

        reporter.flush();
        let update = rx.try_recv().unwrap();
        assert_eq!(progress_texts(&update), vec!["no trailing newline"]);
        assert!(
            matches!(
                update,
                AgentUpdate::ToolProgress { chunks, .. }
                    if chunks[0].stream == ToolOutputStream::Stderr
            ),
            "a flushed tail keeps the stream it came from"
        );
    }

    #[test]
    fn flush_is_a_no_op_without_a_redactor() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let reporter = ToolProgressReporter::new("bash-7", Some(tx));
        reporter.flush();
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn off_level_does_not_install_a_redactor() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let reporter = ToolProgressReporter::new("bash-7", Some(tx))
            .with_stream_redaction(RedactionLevel::Off);
        reporter.report(vec![ToolOutputChunk::stdout("short")]);
        assert_eq!(progress_texts(&rx.try_recv().unwrap()), vec!["short"]);
    }
}
