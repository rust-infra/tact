//! Forward subagent `ui_tx` traffic as `ToolProgress` for the parent tool card.
//!
//! Two parallel streams leave this module for every child event:
//!
//! * the **flattened text** chunks that feed the parent card's live preview
//!   (and the popup's copy fallback), and
//! * the **structured transcript** ([`SubagentTranscriptEntry`]) the popup
//!   renders with the main log's own cells.
//!
//! Both are redacted. The structured stream is not a private channel: an entry
//! that skipped the reporter would put back exactly what the redactor removed
//! from the flat one, and the popup would show a secret the live card hid.

use std::collections::HashMap;

use tact_protocol::{
    AgentUpdate, SubagentStep, SubagentStepStatus, SubagentTranscriptEntry, ToolOutputChunk,
    ToolPresentationInfo,
};
use tokio::sync::mpsc::UnboundedSender;

use crate::tool::ToolProgressReporter;

/// Blank-line pad a structural UI block.
///
/// The completed subagent popup runs the whole transcript through Markdown.
/// A single `\n` is a soft break there and collapses into a space — that is
/// why `…help with! ⚡ 3228 tokens` appeared on one line. Two newlines make a
/// paragraph boundary so TokenUsage / steps / thinking stay separate.
fn structural_line(text: impl Into<String>) -> ToolOutputChunk {
    let body = text.into();
    let body = body.trim_matches('\n');
    ToolOutputChunk::other(format!("\n\n{body}\n\n"))
}

fn structural_stderr(text: impl Into<String>) -> ToolOutputChunk {
    let body = text.into();
    let body = body.trim_matches('\n');
    ToolOutputChunk::stderr(format!("\n\n{body}\n\n"))
}

/// Keep explicit newlines visible after Markdown rendering (soft-break → space).
/// Empty lines become paragraph breaks; non-empty lines use a hard break.
fn preserve_markdown_newlines(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    if lines.is_empty() {
        return String::new();
    }
    let mut out = String::new();
    for (i, line) in lines.iter().enumerate() {
        out.push_str(line);
        if i + 1 == lines.len() {
            break;
        }
        if line.is_empty() {
            out.push_str("\n\n");
        } else {
            // CommonMark hard line break: two trailing spaces before `\n`.
            out.push_str("  \n");
        }
    }
    out
}

/// Labeled thinking block. Not a Markdown blockquote — `>` + soft-break made
/// titles glue to the next sentence and doubled the quote gutter (`▎ >`).
fn format_thinking_block(summary: &str) -> String {
    format!(
        "🧠 Thinking\n\n{}",
        preserve_markdown_newlines(summary.trim())
    )
}

/// Redact every text field of a transcript entry.
///
/// See the module header: the structured stream has to be redacted at the same
/// level as the flat one, or it becomes a way around it.
fn redacted_entry(
    progress: &ToolProgressReporter,
    entry: SubagentTranscriptEntry,
) -> SubagentTranscriptEntry {
    match entry {
        SubagentTranscriptEntry::Assistant(text) => {
            SubagentTranscriptEntry::Assistant(progress.redact_text(&text))
        }
        SubagentTranscriptEntry::Thinking { text, duration_us } => {
            SubagentTranscriptEntry::Thinking {
                text: progress.redact_text(&text),
                duration_us,
            }
        }
        SubagentTranscriptEntry::Info(text) => {
            SubagentTranscriptEntry::Info(progress.redact_text(&text))
        }
        SubagentTranscriptEntry::Error(text) => {
            SubagentTranscriptEntry::Error(progress.redact_text(&text))
        }
        SubagentTranscriptEntry::Step(mut step) => {
            step.arg_summary = progress.redact_text(&step.arg_summary);
            step.arg_full = progress.redact_text(&step.arg_full);
            step.message = progress.redact_text(&step.message);
            step.detail = step.detail.map(|detail| progress.redact_text(&detail));
            SubagentTranscriptEntry::Step(step)
        }
    }
}

fn emit_transcript(
    inner: &UnboundedSender<AgentUpdate>,
    progress: &ToolProgressReporter,
    entry: SubagentTranscriptEntry,
) {
    // A final assistant entry supersedes the in-flight streaming snapshot; the
    // consumer drops its buffered copy for exactly this tool id.
    if matches!(entry, SubagentTranscriptEntry::Assistant(_)) {
        let _ = inner.send(AgentUpdate::ToolTranscriptStream {
            tool_id: progress.tool_id().to_string(),
            text: String::new(),
        });
    }
    let _ = inner.send(AgentUpdate::ToolTranscript {
        tool_id: progress.tool_id().to_string(),
        entry: redacted_entry(progress, entry),
    });
}

/// Flush the accumulated assistant text as one entry.
///
/// Consecutive `StreamChunk`s belong to one assistant turn; emitting one entry
/// per chunk would make the popup render one Markdown block per token batch.
/// Called before any other entry so the transcript keeps its order, and once
/// more when the channel closes.
fn flush_assistant(
    buf: &mut String,
    committed_lines: &mut usize,
    total_lines: &mut usize,
    inner: &UnboundedSender<AgentUpdate>,
    progress: &ToolProgressReporter,
) {
    // The turn is over either way, so the next one starts a fresh stream.
    *committed_lines = 0;
    *total_lines = 0;
    if buf.trim().is_empty() {
        buf.clear();
        return;
    }
    let text = std::mem::take(buf);
    emit_transcript(inner, progress, SubagentTranscriptEntry::Assistant(text));
}

/// Spawn a forwarder that pushes subagent stream/steps/thinking as
/// [`AgentUpdate::ToolProgress`] into the parent tool card (plus a structured
/// [`AgentUpdate::ToolTranscript`] alongside each event for the popup), while
/// passing through permission selects with a `[Subagent]` prefix.
///
/// Returns a new sender for the subagent to use as `ui_tx`.
pub fn tagged_ui_channel_with_progress(
    inner: UnboundedSender<AgentUpdate>,
    progress: ToolProgressReporter,
    context_window: usize,
) -> UnboundedSender<AgentUpdate> {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    tokio::spawn(async move {
        let mut thinking_buf = String::new();
        // When the open thinking block started, so its card can report `⏱`.
        let mut thinking_started: Option<std::time::Instant> = None;
        let mut assistant_buf = String::new();
        // `StepFailed` carries no tool name or presentation (the main TUI
        // resolves them from the live card), so a failed child call remembers
        // what its `StepStarted` half said.
        let mut running_steps: HashMap<String, SubagentStep> = HashMap::new();
        // Newline count of the last published snapshot, and the running total
        // of the turn: a chunk that adds no line break cannot change the
        // rendered shape, so it does not publish.
        let mut assistant_committed_lines = 0usize;
        let mut assistant_total_lines = 0usize;
        while let Some(update) = rx.recv().await {
            match update {
                AgentUpdate::StreamChunk(text) => {
                    assistant_buf.push_str(&text);
                    progress.report(vec![ToolOutputChunk::other(text.clone())]);
                    // Running count: rescanning the whole turn per chunk is
                    // quadratic, and a long turn arrives in hundreds of chunks.
                    assistant_total_lines += text.bytes().filter(|b| *b == b'\n').count();
                    // Publish the in-flight turn so the popup grows while the
                    // text streams, like the main log. Setting the whole turn
                    // (not a delta) keeps the payload idempotent for a
                    // reconnect or a re-ordered consumer.
                    //
                    // Only a new line *boundary* can change the rendered shape,
                    // so a chunk that merely grows the last line will not
                    // publish: an appended word is visible in the next entry
                    // instead, and the O(turn) re-layout per token is avoided.
                    if assistant_total_lines > assistant_committed_lines {
                        assistant_committed_lines = assistant_total_lines;
                        let text = progress.redact_streaming(&assistant_buf);
                        if !text.trim().is_empty() {
                            let _ = inner.send(AgentUpdate::ToolTranscriptStream {
                                tool_id: progress.tool_id().to_string(),
                                text,
                            });
                        }
                    }
                }
                AgentUpdate::ThinkingChunk(chunk) => match chunk {
                    tact_protocol::ThinkingChunk::Started => {
                        flush_assistant(
                            &mut assistant_buf,
                            &mut assistant_committed_lines,
                            &mut assistant_total_lines,
                            &inner,
                            &progress,
                        );
                        thinking_buf.clear();
                        thinking_started = Some(std::time::Instant::now());
                    }
                    tact_protocol::ThinkingChunk::Delta(text) => {
                        thinking_buf.push_str(&text);
                    }
                    tact_protocol::ThinkingChunk::Finished => {
                        flush_assistant(
                            &mut assistant_buf,
                            &mut assistant_committed_lines,
                            &mut assistant_total_lines,
                            &inner,
                            &progress,
                        );
                        let summary = thinking_buf.trim().to_string();
                        thinking_buf.clear();
                        // The transcript is the only place that knows when the
                        // block started, so the card's `⏱` readout is measured
                        // here rather than guessed downstream.
                        let duration_us = thinking_started
                            .take()
                            .map(|started| started.elapsed().as_micros() as u64);
                        if !summary.is_empty() {
                            progress.report(vec![structural_line(format_thinking_block(&summary))]);
                            emit_transcript(
                                &inner,
                                &progress,
                                SubagentTranscriptEntry::Thinking {
                                    text: summary,
                                    duration_us,
                                },
                            );
                        }
                    }
                },
                AgentUpdate::StepStarted {
                    tool_id,
                    tool_name,
                    arg_summary,
                    arg_full,
                    presentation,
                    ..
                } => {
                    flush_assistant(
                        &mut assistant_buf,
                        &mut assistant_committed_lines,
                        &mut assistant_total_lines,
                        &inner,
                        &progress,
                    );
                    let line = if arg_summary.is_empty() {
                        format!("→ {tool_name}")
                    } else {
                        format!("→ {tool_name} {arg_summary}")
                    };
                    progress.report(vec![structural_line(line)]);
                    let step = SubagentStep {
                        tool_id: tool_id.clone(),
                        tool_name,
                        arg_summary,
                        arg_full,
                        status: SubagentStepStatus::Running,
                        message: String::new(),
                        detail: None,
                        duration_us: None,
                        presentation,
                    };
                    running_steps.insert(tool_id, step.clone());
                    emit_transcript(&inner, &progress, SubagentTranscriptEntry::Step(step));
                }
                AgentUpdate::StepFinished {
                    tool_id, result, ..
                } => {
                    flush_assistant(
                        &mut assistant_buf,
                        &mut assistant_committed_lines,
                        &mut assistant_total_lines,
                        &inner,
                        &progress,
                    );
                    running_steps.remove(&tool_id);
                    let preview = result.message.clone();
                    if !preview.is_empty() {
                        // Multi-line tool output (e.g. `ls -la`) must keep its
                        // newlines through the completed-popup Markdown pass.
                        progress.report(vec![structural_line(format!(
                            "✓ {}",
                            preserve_markdown_newlines(&preview)
                        ))]);
                    }
                    let arg_full = result
                        .arg_full
                        .clone()
                        .unwrap_or_else(|| result.arg_summary.clone());
                    emit_transcript(
                        &inner,
                        &progress,
                        SubagentTranscriptEntry::Step(SubagentStep {
                            tool_id,
                            tool_name: result.tool,
                            arg_summary: result.arg_summary,
                            arg_full,
                            status: SubagentStepStatus::Success,
                            message: result.message,
                            detail: result.detail,
                            duration_us: result.duration_us,
                            presentation: result.presentation,
                        }),
                    );
                }
                AgentUpdate::StepFailed {
                    tool_id,
                    arg_summary,
                    error,
                    ..
                } => {
                    flush_assistant(
                        &mut assistant_buf,
                        &mut assistant_committed_lines,
                        &mut assistant_total_lines,
                        &inner,
                        &progress,
                    );
                    progress.report(vec![structural_stderr(format!(
                        "✗ {}",
                        preserve_markdown_newlines(&error)
                    ))]);
                    let base = running_steps.remove(&tool_id);
                    let arg_summary = if arg_summary.is_empty() {
                        base.as_ref()
                            .map(|b| b.arg_summary.clone())
                            .unwrap_or_default()
                    } else {
                        arg_summary
                    };
                    emit_transcript(
                        &inner,
                        &progress,
                        SubagentTranscriptEntry::Step(SubagentStep {
                            tool_id,
                            tool_name: base
                                .as_ref()
                                .map(|b| b.tool_name.clone())
                                .unwrap_or_default(),
                            arg_summary,
                            arg_full: base
                                .as_ref()
                                .map(|b| b.arg_full.clone())
                                .unwrap_or_default(),
                            status: SubagentStepStatus::Failed,
                            message: error,
                            detail: None,
                            duration_us: None,
                            presentation: base
                                .map(|b| b.presentation)
                                .unwrap_or_else(|| ToolPresentationInfo::generic("unknown")),
                        }),
                    );
                }
                AgentUpdate::Info(msg) => {
                    flush_assistant(
                        &mut assistant_buf,
                        &mut assistant_committed_lines,
                        &mut assistant_total_lines,
                        &inner,
                        &progress,
                    );
                    progress.report(vec![structural_line(msg.clone())]);
                    emit_transcript(&inner, &progress, SubagentTranscriptEntry::Info(msg));
                }
                AgentUpdate::MdInfo(msg) => {
                    flush_assistant(
                        &mut assistant_buf,
                        &mut assistant_committed_lines,
                        &mut assistant_total_lines,
                        &inner,
                        &progress,
                    );
                    progress.report(vec![structural_line(msg.clone())]);
                    emit_transcript(&inner, &progress, SubagentTranscriptEntry::Info(msg));
                }
                AgentUpdate::Error(err) => {
                    flush_assistant(
                        &mut assistant_buf,
                        &mut assistant_committed_lines,
                        &mut assistant_total_lines,
                        &inner,
                        &progress,
                    );
                    let text = format!("error: {err:?}");
                    progress.report(vec![structural_stderr(text.clone())]);
                    emit_transcript(&inner, &progress, SubagentTranscriptEntry::Error(text));
                }
                AgentUpdate::TokenUsage(usage) => {
                    // Update the parent tool card header (model + ctx meter)
                    // instead of cluttering the output stream with inline lines.
                    let _ = inner.send(AgentUpdate::ToolMeta {
                        tool_id: progress.tool_id().to_string(),
                        model: None,
                        token_usage: Some(usage),
                        context_window: Some(context_window),
                        task_id: None,
                    });
                }
                AgentUpdate::ModelInfo(params) => {
                    let _ = inner.send(AgentUpdate::ToolMeta {
                        tool_id: progress.tool_id().to_string(),
                        model: Some(params.model),
                        token_usage: None,
                        context_window: None,
                        task_id: None,
                    });
                }
                AgentUpdate::RequestSelect {
                    request_id,
                    mut prompt,
                    options,
                    log_confirm,
                } => {
                    prompt = format!("[Subagent] {prompt}");
                    let _ = inner.send(AgentUpdate::RequestSelect {
                        request_id,
                        prompt,
                        options,
                        log_confirm,
                    });
                }
                AgentUpdate::RequestMultiSelect {
                    request_id,
                    mut prompt,
                    options,
                } => {
                    prompt = format!("[Subagent] {prompt}");
                    let _ = inner.send(AgentUpdate::RequestMultiSelect {
                        request_id,
                        prompt,
                        options,
                    });
                }
                AgentUpdate::TaskComplete(_) | AgentUpdate::TaskCancelled => {
                    flush_assistant(
                        &mut assistant_buf,
                        &mut assistant_committed_lines,
                        &mut assistant_total_lines,
                        &inner,
                        &progress,
                    );
                }
                // Unused/unknown variants — skip silently.
                _ => {}
            }
        }
        // The child's channel closed: whatever assistant text was still open is
        // a complete turn, not a partial one.
        flush_assistant(
            &mut assistant_buf,
            &mut assistant_committed_lines,
            &mut assistant_total_lines,
            &inner,
            &progress,
        );
    });
    tx
}

#[cfg(test)]
mod tests {
    use super::*;
    use tact_protocol::{
        StepResult, StepStatus, ThinkingChunk, TokenUsageInfo, ToolPresentationInfo,
    };

    #[tokio::test]
    async fn streams_become_tool_progress() {
        let (inner_tx, mut inner_rx) = tokio::sync::mpsc::unbounded_channel();
        let (progress_tx, mut progress_rx) = tokio::sync::mpsc::unbounded_channel();
        let progress = ToolProgressReporter::new("task-1", Some(progress_tx));
        let tagged = tagged_ui_channel_with_progress(inner_tx, progress, 200_000);

        tagged
            .send(AgentUpdate::StreamChunk("hello world\n".into()))
            .unwrap();
        tokio::task::yield_now().await;

        let got: Vec<AgentUpdate> = std::iter::from_fn(|| progress_rx.try_recv().ok()).collect();
        assert_eq!(got.len(), 1);
        match &got[0] {
            AgentUpdate::ToolProgress { tool_id, chunks } => {
                assert_eq!(tool_id, "task-1");
                assert_eq!(chunks.len(), 1);
                assert_eq!(chunks[0].text, "hello world\n");
            }
            other => panic!("expected ToolProgress, got {other:?}"),
        }
        // The same chunk also publishes the in-flight turn so the popup grows
        // while the child streams, instead of staying empty until it settles.
        match inner_rx.try_recv().unwrap() {
            AgentUpdate::ToolTranscriptStream { tool_id, text } => {
                assert_eq!(tool_id, "task-1");
                assert_eq!(text, "hello world\n");
            }
            other => panic!("expected ToolTranscriptStream, got {other:?}"),
        }
        assert!(
            inner_rx.try_recv().is_err(),
            "one snapshot per line, not chunk"
        );
    }

    /// A chunk that adds no line break cannot change the popup's rendered
    /// shape, so it must not trigger a full re-layout — the turn's next line
    /// boundary carries the text instead.
    #[tokio::test]
    async fn a_chunk_without_a_line_break_does_not_publish_a_snapshot() {
        let (inner_tx, mut inner_rx) = tokio::sync::mpsc::unbounded_channel();
        let (progress_tx, _progress_rx) = tokio::sync::mpsc::unbounded_channel();
        let progress = ToolProgressReporter::new("t1", Some(progress_tx));
        let tagged = tagged_ui_channel_with_progress(inner_tx, progress, 200_000);

        tagged
            .send(AgentUpdate::StreamChunk("partial".into()))
            .unwrap();
        tokio::task::yield_now().await;
        assert!(
            inner_rx.try_recv().is_err(),
            "a line that has not ended yet must not publish"
        );

        tagged
            .send(AgentUpdate::StreamChunk(" line\n".into()))
            .unwrap();
        tokio::task::yield_now().await;
        match inner_rx.try_recv().unwrap() {
            AgentUpdate::ToolTranscriptStream { text, .. } => {
                assert_eq!(text, "partial line\n");
            }
            other => panic!("expected ToolTranscriptStream, got {other:?}"),
        }
    }

    /// The review-found leak: a private key arriving line by line must never
    /// publish its body, because the whole-text rule cannot match until the END
    /// marker arrives.
    #[tokio::test]
    async fn a_streaming_private_key_never_publishes_its_body() {
        use crate::security::RedactionLevel;
        let (inner_tx, mut inner_rx) = tokio::sync::mpsc::unbounded_channel();
        let (progress_tx, _progress_rx) = tokio::sync::mpsc::unbounded_channel();
        let progress = ToolProgressReporter::new("t1", Some(progress_tx))
            .with_stream_redaction(RedactionLevel::Basic);
        let tagged = tagged_ui_channel_with_progress(inner_tx, progress, 200_000);

        // Built by concatenation so the marker never appears as a literal here.
        let begin = concat!("-----BEGIN ", "RSA PRIVATE KEY-----\n");
        let end = concat!("-----END ", "RSA PRIVATE KEY-----\n");
        for chunk in ["key:\n", begin, "SECRETBODYLINE1\n", end] {
            tagged
                .send(AgentUpdate::StreamChunk(chunk.to_string()))
                .unwrap();
            tokio::task::yield_now().await;
            while let Ok(update) = inner_rx.try_recv() {
                if let AgentUpdate::ToolTranscriptStream { text, .. } = update {
                    assert!(
                        !text.contains("SECRETBODY"),
                        "the key body leaked in a streaming snapshot: {text:?}"
                    );
                }
            }
        }
    }

    /// A single-line secret is withheld until the turn settles, and the settled
    /// entry carries it redacted.
    #[tokio::test]
    async fn a_streaming_token_is_withheld_until_the_turn_ends() {
        use crate::security::RedactionLevel;
        let (inner_tx, mut inner_rx) = tokio::sync::mpsc::unbounded_channel();
        let (progress_tx, _progress_rx) = tokio::sync::mpsc::unbounded_channel();
        let progress = ToolProgressReporter::new("t1", Some(progress_tx))
            .with_stream_redaction(RedactionLevel::Basic);
        let tagged = tagged_ui_channel_with_progress(inner_tx, progress, 200_000);

        // `sk-` plus a long body - the provider-token rule.
        let token = concat!("token sk-", "abcdefghijklmnopqrstuvwxyz0123456789", "\n");
        tagged.send(AgentUpdate::StreamChunk(token.into())).unwrap();
        tokio::task::yield_now().await;
        // The published prefix ends at the newline, so the whole-text rule has
        // already redacted the token.
        while let Ok(update) = inner_rx.try_recv() {
            if let AgentUpdate::ToolTranscriptStream { text, .. } = update {
                assert!(
                    !text.contains("abcdefghijklmnop"),
                    "an unredacted token leaked: {text:?}"
                );
            }
        }
    }

    #[tokio::test]
    async fn thinking_finished_emits_summary_with_prefix() {
        let (inner_tx, _inner_rx) = tokio::sync::mpsc::unbounded_channel();
        let (progress_tx, mut progress_rx) = tokio::sync::mpsc::unbounded_channel();
        let progress = ToolProgressReporter::new("t1", Some(progress_tx));
        let tagged = tagged_ui_channel_with_progress(inner_tx, progress, 200_000);

        tagged
            .send(AgentUpdate::ThinkingChunk(ThinkingChunk::Started))
            .unwrap();
        tagged
            .send(AgentUpdate::ThinkingChunk(ThinkingChunk::Delta(
                "reasoning line\nsecond".into(),
            )))
            .unwrap();
        tagged
            .send(AgentUpdate::ThinkingChunk(ThinkingChunk::Finished))
            .unwrap();
        tokio::task::yield_now().await;

        let got: Vec<AgentUpdate> = std::iter::from_fn(|| progress_rx.try_recv().ok()).collect();
        assert_eq!(got.len(), 1);
        match &got[0] {
            AgentUpdate::ToolProgress { chunks, .. } => {
                let text = &chunks[0].text;
                assert!(text.contains("🧠 Thinking"), "got {text:?}");
                assert!(
                    !text.contains("> 🧠"),
                    "blockquote markers should be gone: {text:?}"
                );
                assert!(text.contains("reasoning line"), "got {text:?}");
                assert!(text.contains("second"), "got {text:?}");
                // Hard break between thinking lines (two spaces + newline).
                assert!(text.contains("reasoning line  \nsecond"), "got {text:?}");
                assert!(text.starts_with("\n\n"), "got {text:?}");
                assert!(text.ends_with("\n\n"), "got {text:?}");
            }
            other => panic!("expected ToolProgress, got {other:?}"),
        }
    }

    #[test]
    fn thinking_block_is_distinct_from_assistant_stream() {
        assert_eq!(
            format_thinking_block("plan the answer"),
            "🧠 Thinking\n\nplan the answer"
        );
        assert_eq!(
            format_thinking_block("line one\nline two"),
            "🧠 Thinking\n\nline one  \nline two"
        );
    }

    #[test]
    fn structural_line_uses_blank_paragraph_breaks() {
        // Regression: single `\n` before ⚡ soft-breaks into the previous
        // assistant sentence under Markdown (`help with! ⚡ 3228 tokens`).
        let chunk = structural_line("⚡ 3228 tokens · ▣ cache% 71% · ctx 2.7K");
        assert_eq!(
            chunk.text,
            "\n\n⚡ 3228 tokens · ▣ cache% 71% · ctx 2.7K\n\n"
        );
    }

    #[test]
    fn preserve_markdown_newlines_keeps_ls_rows() {
        let preview = "total 2456\ndrwxr-xr-x@ 30 rg staff 960";
        assert_eq!(
            preserve_markdown_newlines(preview),
            "total 2456  \ndrwxr-xr-x@ 30 rg staff 960"
        );
    }

    #[tokio::test]
    async fn empty_thinking_skips_summary() {
        let (inner_tx, _inner_rx) = tokio::sync::mpsc::unbounded_channel();
        let (progress_tx, mut progress_rx) = tokio::sync::mpsc::unbounded_channel();
        let progress = ToolProgressReporter::new("t1", Some(progress_tx));
        let tagged = tagged_ui_channel_with_progress(inner_tx, progress, 200_000);

        tagged
            .send(AgentUpdate::ThinkingChunk(ThinkingChunk::Started))
            .unwrap();
        tagged
            .send(AgentUpdate::ThinkingChunk(ThinkingChunk::Finished))
            .unwrap();
        tokio::task::yield_now().await;

        assert!(progress_rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn step_started_and_failed_map_to_progress() {
        let (inner_tx, _inner_rx) = tokio::sync::mpsc::unbounded_channel();
        let (progress_tx, mut progress_rx) = tokio::sync::mpsc::unbounded_channel();
        let progress = ToolProgressReporter::new("t1", Some(progress_tx));
        let tagged = tagged_ui_channel_with_progress(inner_tx, progress, 200_000);

        tagged
            .send(AgentUpdate::StepStarted {
                idx: 0,
                tool_id: "b1".into(),
                tool_name: "read_file".into(),
                arg_summary: "main.rs".into(),
                arg_full: "main.rs".into(),
                presentation: tact_protocol::ToolPresentationInfo::generic("read_file"),
            })
            .unwrap();
        tagged
            .send(AgentUpdate::StepFailed {
                idx: 0,
                tool_id: "b1".into(),
                arg_summary: String::new(),
                error: "not found".into(),
            })
            .unwrap();
        tokio::task::yield_now().await;

        let got: Vec<AgentUpdate> = std::iter::from_fn(|| progress_rx.try_recv().ok()).collect();
        assert_eq!(got.len(), 2);
        match &got[0] {
            AgentUpdate::ToolProgress { chunks, .. } => {
                assert!(chunks[0].text.contains("read_file"));
                assert!(chunks[0].text.contains("main.rs"));
            }
            other => panic!("expected ToolProgress, got {other:?}"),
        }
        match &got[1] {
            AgentUpdate::ToolProgress { chunks, .. } => {
                assert!(chunks[0].text.contains("✗ "));
                assert!(chunks[0].text.contains("not found"));
                assert!(chunks[0].text.starts_with("\n\n"));
                assert!(matches!(
                    chunks[0].stream,
                    tact_protocol::ToolOutputStream::Stderr
                ));
            }
            other => panic!("expected ToolProgress, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn step_finished_emits_preview() {
        let (inner_tx, _inner_rx) = tokio::sync::mpsc::unbounded_channel();
        let (progress_tx, mut progress_rx) = tokio::sync::mpsc::unbounded_channel();
        let progress = ToolProgressReporter::new("t1", Some(progress_tx));
        let tagged = tagged_ui_channel_with_progress(inner_tx, progress, 200_000);

        tagged
            .send(AgentUpdate::StepFinished {
                idx: 0,
                tool_id: "b1".into(),
                result: StepResult {
                    tool: "bash".into(),
                    arg_summary: "echo hi".into(),
                    arg_full: Some("echo hi".into()),
                    status: StepStatus::Success,
                    message: "hi".into(),
                    detail: None,
                    duration_us: None,
                    permission_label: None,
                    presentation: ToolPresentationInfo::generic("bash"),
                },
            })
            .unwrap();
        tokio::task::yield_now().await;

        let got: Vec<AgentUpdate> = std::iter::from_fn(|| progress_rx.try_recv().ok()).collect();
        assert_eq!(got.len(), 1);
        match &got[0] {
            AgentUpdate::ToolProgress { chunks, .. } => {
                assert!(chunks[0].text.contains("✓ "));
                assert!(chunks[0].text.contains("hi"));
                assert!(chunks[0].text.starts_with("\n\n"));
                assert!(chunks[0].text.ends_with("\n\n"));
            }
            other => panic!("expected ToolProgress, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn empty_step_finished_message_is_skipped() {
        let (inner_tx, _inner_rx) = tokio::sync::mpsc::unbounded_channel();
        let (progress_tx, mut progress_rx) = tokio::sync::mpsc::unbounded_channel();
        let progress = ToolProgressReporter::new("t1", Some(progress_tx));
        let tagged = tagged_ui_channel_with_progress(inner_tx, progress, 200_000);

        tagged
            .send(AgentUpdate::StepFinished {
                idx: 0,
                tool_id: "b1".into(),
                result: StepResult {
                    tool: "bash".into(),
                    arg_summary: "".into(),
                    arg_full: None,
                    status: StepStatus::Success,
                    message: "".into(),
                    detail: None,
                    duration_us: None,
                    permission_label: None,
                    presentation: ToolPresentationInfo::generic("bash"),
                },
            })
            .unwrap();
        tokio::task::yield_now().await;

        assert!(progress_rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn select_prefixed_with_subagent() {
        let (inner_tx, mut inner_rx) = tokio::sync::mpsc::unbounded_channel();
        let (progress_tx, _progress_rx) = tokio::sync::mpsc::unbounded_channel();
        let progress = ToolProgressReporter::new("t1", Some(progress_tx));
        let tagged = tagged_ui_channel_with_progress(inner_tx, progress, 200_000);

        tagged
            .send(AgentUpdate::RequestSelect {
                request_id: 42,
                prompt: "Allow bash?".into(),
                options: vec!["Yes".into()],
                log_confirm: false,
            })
            .unwrap();
        tokio::task::yield_now().await;

        let got = inner_rx.try_recv().unwrap();
        match got {
            AgentUpdate::RequestSelect {
                request_id, prompt, ..
            } => {
                assert_eq!(
                    request_id, 42,
                    "request id must survive subagent forwarding"
                );
                assert!(prompt.starts_with("[Subagent]"));
            }
            other => panic!("expected RequestSelect, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn token_usage_sends_tool_meta_to_parent() {
        let (inner_tx, mut inner_rx) = tokio::sync::mpsc::unbounded_channel();
        let (progress_tx, mut progress_rx) = tokio::sync::mpsc::unbounded_channel();
        let progress = ToolProgressReporter::new("t1", Some(progress_tx));
        let tagged = tagged_ui_channel_with_progress(inner_tx, progress, 200_000);

        tagged
            .send(AgentUpdate::TokenUsage(TokenUsageInfo {
                total: 999,
                prompt: 800,
                prompt_cache_hit_tokens: 600,
                prompt_cache_miss_tokens: 200,
                ..Default::default()
            }))
            .unwrap();
        tokio::task::yield_now().await;

        // No progress chunks — token usage updates the tool card header via ToolMeta.
        assert!(
            progress_rx.try_recv().is_err(),
            "TokenUsage must not produce ToolProgress chunks"
        );
        // ToolMeta sent to parent channel for the TUI to update the tool card header.
        match inner_rx.try_recv().unwrap() {
            AgentUpdate::ToolMeta {
                tool_id,
                model,
                token_usage,
                context_window,
                task_id,
            } => {
                assert_eq!(tool_id, "t1");
                assert!(model.is_none());
                assert!(task_id.is_none());
                assert_eq!(context_window, Some(200_000));
                let u = token_usage.unwrap();
                assert_eq!(u.total, 999);
                assert_eq!(u.prompt_cache_hit_tokens, 600);
            }
            other => panic!("expected ToolMeta, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn structural_events_are_newline_delimited() {
        let (inner_tx, _inner_rx) = tokio::sync::mpsc::unbounded_channel();
        let (progress_tx, mut progress_rx) = tokio::sync::mpsc::unbounded_channel();
        let progress = ToolProgressReporter::new("t1", Some(progress_tx));
        let tagged = tagged_ui_channel_with_progress(inner_tx, progress, 200_000);

        // Stream chunk with no trailing newline — previously glued to the next event.
        tagged
            .send(AgentUpdate::StreamChunk("partial".into()))
            .unwrap();
        tagged
            .send(AgentUpdate::StepStarted {
                idx: 0,
                tool_id: "b1".into(),
                tool_name: "bash".into(),
                arg_summary: "ls".into(),
                arg_full: "ls".into(),
                presentation: ToolPresentationInfo::generic("bash"),
            })
            .unwrap();
        tokio::task::yield_now().await;

        let got: Vec<AgentUpdate> = std::iter::from_fn(|| progress_rx.try_recv().ok()).collect();
        assert_eq!(got.len(), 2);
        match &got[0] {
            AgentUpdate::ToolProgress { chunks, .. } => {
                assert_eq!(chunks[0].text, "partial");
            }
            other => panic!("expected ToolProgress, got {other:?}"),
        }
        match &got[1] {
            AgentUpdate::ToolProgress { chunks, .. } => {
                assert_eq!(chunks[0].text, "\n\n→ bash ls\n\n");
            }
            other => panic!("expected ToolProgress, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn model_info_sends_tool_meta_to_parent() {
        let (inner_tx, mut inner_rx) = tokio::sync::mpsc::unbounded_channel();
        let (progress_tx, mut progress_rx) = tokio::sync::mpsc::unbounded_channel();
        let progress = ToolProgressReporter::new("t1", Some(progress_tx));
        let tagged = tagged_ui_channel_with_progress(inner_tx, progress, 200_000);

        tagged
            .send(AgentUpdate::ModelInfo(tact_protocol::ModelCallParams {
                model: "fake".into(),
                max_tokens: 4096,
                thinking_budget: None,
                reasoning_effort: None,
                extra_body: None,
            }))
            .unwrap();
        tokio::task::yield_now().await;

        // No progress chunks — model info updates the tool card header via ToolMeta.
        assert!(
            progress_rx.try_recv().is_err(),
            "ModelInfo must not produce ToolProgress chunks"
        );
        match inner_rx.try_recv().unwrap() {
            AgentUpdate::ToolMeta {
                tool_id,
                model,
                token_usage,
                context_window,
                task_id,
            } => {
                assert_eq!(tool_id, "t1");
                assert!(task_id.is_none());
                assert_eq!(model.unwrap(), "fake");
                assert!(token_usage.is_none());
                assert!(context_window.is_none());
            }
            other => panic!("expected ToolMeta, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn task_complete_and_cancelled_are_dropped() {
        let (inner_tx, mut inner_rx) = tokio::sync::mpsc::unbounded_channel();
        let (progress_tx, mut progress_rx) = tokio::sync::mpsc::unbounded_channel();
        let progress = ToolProgressReporter::new("t1", Some(progress_tx));
        let tagged = tagged_ui_channel_with_progress(inner_tx, progress, 200_000);

        tagged
            .send(AgentUpdate::TaskComplete("done".into()))
            .unwrap();
        tagged.send(AgentUpdate::TaskCancelled).unwrap();
        tokio::task::yield_now().await;

        assert!(progress_rx.try_recv().is_err());
        assert!(inner_rx.try_recv().is_err());
    }

    // ── Structured transcript ────────────────────────────────────────────
    //
    // The popup renders from `ToolTranscript`, not from the flattened text, so
    // these pin the structure the flat stream cannot carry.

    fn transcript_entries(
        rx: &mut tokio::sync::mpsc::UnboundedReceiver<AgentUpdate>,
    ) -> Vec<SubagentTranscriptEntry> {
        std::iter::from_fn(|| rx.try_recv().ok())
            .filter_map(|update| match update {
                AgentUpdate::ToolTranscript { entry, .. } => Some(entry),
                _ => None,
            })
            .collect()
    }

    #[tokio::test]
    async fn consecutive_stream_chunks_become_one_assistant_entry() {
        let (inner_tx, mut inner_rx) = tokio::sync::mpsc::unbounded_channel();
        let (progress_tx, _progress_rx) = tokio::sync::mpsc::unbounded_channel();
        let progress = ToolProgressReporter::new("t1", Some(progress_tx));
        let tagged = tagged_ui_channel_with_progress(inner_tx, progress, 200_000);

        for chunk in ["Looking ", "it ", "up."] {
            tagged.send(AgentUpdate::StreamChunk(chunk.into())).unwrap();
        }
        // The flush happens on the *next* non-stream event.
        tagged
            .send(AgentUpdate::StepStarted {
                idx: 0,
                tool_id: "c1".into(),
                tool_name: "web_search".into(),
                arg_summary: "bitcoin".into(),
                arg_full: "bitcoin".into(),
                presentation: tact_protocol::ToolPresentationInfo::generic("web_search"),
            })
            .unwrap();
        tokio::task::yield_now().await;

        let entries = transcript_entries(&mut inner_rx);
        assert_eq!(
            entries.len(),
            2,
            "one assistant run + one step: {entries:?}"
        );
        match &entries[0] {
            SubagentTranscriptEntry::Assistant(text) => assert_eq!(text, "Looking it up."),
            other => panic!("expected Assistant, got {other:?}"),
        }
        match &entries[1] {
            SubagentTranscriptEntry::Step(step) => {
                assert_eq!(step.tool_id, "c1");
                assert_eq!(step.status, SubagentStepStatus::Running);
                assert_eq!(step.tool_name, "web_search");
            }
            other => panic!("expected Step, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_step_reports_both_halves_under_one_tool_id() {
        let (inner_tx, mut inner_rx) = tokio::sync::mpsc::unbounded_channel();
        let (progress_tx, _progress_rx) = tokio::sync::mpsc::unbounded_channel();
        let progress = ToolProgressReporter::new("t1", Some(progress_tx));
        let tagged = tagged_ui_channel_with_progress(inner_tx, progress, 200_000);

        tagged
            .send(AgentUpdate::StepStarted {
                idx: 0,
                tool_id: "c1".into(),
                tool_name: "read_file".into(),
                arg_summary: "main.rs".into(),
                arg_full: "main.rs".into(),
                presentation: tact_protocol::ToolPresentationInfo::generic("read_file"),
            })
            .unwrap();
        tokio::task::yield_now().await;
        tagged
            .send(AgentUpdate::StepFinished {
                idx: 0,
                tool_id: "c1".into(),
                result: StepResult {
                    tool: "read_file".into(),
                    arg_summary: "main.rs".into(),
                    arg_full: Some("main.rs".into()),
                    status: StepStatus::Success,
                    message: "fn main() {}".into(),
                    detail: Some("fn main() {}".into()),
                    duration_us: Some(1_400),
                    permission_label: None,
                    presentation: tact_protocol::ToolPresentationInfo::generic("read_file"),
                },
            })
            .unwrap();
        tokio::task::yield_now().await;

        let entries = transcript_entries(&mut inner_rx);
        assert_eq!(entries.len(), 2);
        let ids: Vec<String> = entries
            .iter()
            .filter_map(|entry| match entry {
                SubagentTranscriptEntry::Step(step) => Some(step.tool_id.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(ids, vec!["c1", "c1"], "the UI pairs the halves by this id");
        match &entries[1] {
            SubagentTranscriptEntry::Step(step) => {
                assert_eq!(step.status, SubagentStepStatus::Success);
                assert_eq!(step.duration_us, Some(1_400));
                assert_eq!(step.detail.as_deref(), Some("fn main() {}"));
            }
            other => panic!("expected Step, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_failed_step_keeps_what_its_started_half_knew() {
        // `StepFailed` carries no tool name or presentation; the transcript
        // entry still has to be a complete card.
        let (inner_tx, mut inner_rx) = tokio::sync::mpsc::unbounded_channel();
        let (progress_tx, _progress_rx) = tokio::sync::mpsc::unbounded_channel();
        let progress = ToolProgressReporter::new("t1", Some(progress_tx));
        let tagged = tagged_ui_channel_with_progress(inner_tx, progress, 200_000);

        tagged
            .send(AgentUpdate::StepStarted {
                idx: 0,
                tool_id: "c1".into(),
                tool_name: "bash".into(),
                arg_summary: "cargo test".into(),
                arg_full: "cargo test".into(),
                presentation: tact_protocol::ToolPresentationInfo::generic("bash"),
            })
            .unwrap();
        tokio::task::yield_now().await;
        tagged
            .send(AgentUpdate::StepFailed {
                idx: 0,
                tool_id: "c1".into(),
                arg_summary: String::new(),
                error: "exit 101".into(),
            })
            .unwrap();
        tokio::task::yield_now().await;

        let entries = transcript_entries(&mut inner_rx);
        match entries.last().expect("a step entry") {
            SubagentTranscriptEntry::Step(step) => {
                assert_eq!(step.status, SubagentStepStatus::Failed);
                assert_eq!(step.tool_name, "bash");
                assert_eq!(step.arg_summary, "cargo test");
                assert_eq!(step.message, "exit 101");
            }
            other => panic!("expected Step, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn thinking_entry_carries_the_measured_duration() {
        let (inner_tx, mut inner_rx) = tokio::sync::mpsc::unbounded_channel();
        let (progress_tx, _progress_rx) = tokio::sync::mpsc::unbounded_channel();
        let progress = ToolProgressReporter::new("t1", Some(progress_tx));
        let tagged = tagged_ui_channel_with_progress(inner_tx, progress, 200_000);

        tagged
            .send(AgentUpdate::ThinkingChunk(ThinkingChunk::Started))
            .unwrap();
        tagged
            .send(AgentUpdate::ThinkingChunk(ThinkingChunk::Delta(
                "check two sources".into(),
            )))
            .unwrap();
        tagged
            .send(AgentUpdate::ThinkingChunk(ThinkingChunk::Finished))
            .unwrap();
        tokio::task::yield_now().await;

        let entries = transcript_entries(&mut inner_rx);
        assert_eq!(entries.len(), 1);
        match &entries[0] {
            SubagentTranscriptEntry::Thinking { text, duration_us } => {
                assert_eq!(text, "check two sources");
                assert!(
                    duration_us.is_some(),
                    "the card's ⏱ readout comes from the forwarder"
                );
            }
            other => panic!("expected Thinking, got {other:?}"),
        }
    }

    /// The structured stream must not be a way around the redactor: it carries
    /// the same tool output the flat stream has scrubbed.
    #[tokio::test]
    async fn transcript_entries_are_redacted() {
        use crate::security::RedactionLevel;

        let (inner_tx, mut inner_rx) = tokio::sync::mpsc::unbounded_channel();
        let (progress_tx, _progress_rx) = tokio::sync::mpsc::unbounded_channel();
        let progress = ToolProgressReporter::new("t1", Some(progress_tx))
            .with_stream_redaction(RedactionLevel::Basic);
        let tagged = tagged_ui_channel_with_progress(inner_tx, progress, 200_000);

        let secret = "sk-abcdefghijklmnopqrstuvwxyz";
        tagged
            .send(AgentUpdate::StepFinished {
                idx: 0,
                tool_id: "c1".into(),
                result: StepResult {
                    tool: "read_file".into(),
                    arg_summary: "env".into(),
                    arg_full: Some("env".into()),
                    status: StepStatus::Success,
                    message: format!("OPENAI_API_KEY={secret}"),
                    detail: Some(format!("OPENAI_API_KEY={secret}")),
                    duration_us: Some(1),
                    permission_label: None,
                    presentation: tact_protocol::ToolPresentationInfo::generic("read_file"),
                },
            })
            .unwrap();
        tokio::task::yield_now().await;

        let entries = transcript_entries(&mut inner_rx);
        match entries.last().expect("a step entry") {
            SubagentTranscriptEntry::Step(step) => {
                assert!(
                    !step.message.contains(secret),
                    "message leaked the secret: {}",
                    step.message
                );
                assert!(
                    !step.detail.as_deref().unwrap_or_default().contains(secret),
                    "detail leaked the secret"
                );
                assert!(step.message.contains("[redacted:api-key]"));
            }
            other => panic!("expected Step, got {other:?}"),
        }
    }
}
