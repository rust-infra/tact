//! Forward subagent `ui_tx` traffic as `ToolProgress` for the parent tool card.

use tact_protocol::{AgentUpdate, ToolOutputChunk};
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

/// Spawn a forwarder that pushes subagent stream/steps/thinking as
/// [`AgentUpdate::ToolProgress`] into the parent tool card, while passing
/// through permission selects with a `[Subagent]` prefix.
///
/// Returns a new sender for the subagent to use as `ui_tx`.
pub fn tagged_ui_channel_with_progress(
    inner: UnboundedSender<AgentUpdate>,
    progress: ToolProgressReporter,
) -> UnboundedSender<AgentUpdate> {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    tokio::spawn(async move {
        let mut thinking_buf = String::new();
        while let Some(update) = rx.recv().await {
            match update {
                AgentUpdate::StreamChunk(text) => {
                    progress.report(vec![ToolOutputChunk::other(text)]);
                }
                AgentUpdate::ThinkingChunk(chunk) => match chunk {
                    tact_protocol::ThinkingChunk::Started => {
                        thinking_buf.clear();
                    }
                    tact_protocol::ThinkingChunk::Delta(text) => {
                        thinking_buf.push_str(&text);
                    }
                    tact_protocol::ThinkingChunk::Finished => {
                        let summary = thinking_buf.trim().to_string();
                        thinking_buf.clear();
                        if !summary.is_empty() {
                            progress.report(vec![structural_line(format_thinking_block(&summary))]);
                        }
                    }
                },
                AgentUpdate::StepStarted {
                    tool_name,
                    arg_summary,
                    ..
                } => {
                    let line = if arg_summary.is_empty() {
                        format!("→ {tool_name}")
                    } else {
                        format!("→ {tool_name} {arg_summary}")
                    };
                    progress.report(vec![structural_line(line)]);
                }
                AgentUpdate::StepFinished { result, .. } => {
                    let preview = result.message;
                    if !preview.is_empty() {
                        // Multi-line tool output (e.g. `ls -la`) must keep its
                        // newlines through the completed-popup Markdown pass.
                        progress.report(vec![structural_line(format!(
                            "✓ {}",
                            preserve_markdown_newlines(&preview)
                        ))]);
                    }
                }
                AgentUpdate::StepFailed { error, .. } => {
                    progress.report(vec![structural_stderr(format!(
                        "✗ {}",
                        preserve_markdown_newlines(&error)
                    ))]);
                }
                AgentUpdate::Info(msg) => {
                    progress.report(vec![structural_line(msg)]);
                }
                AgentUpdate::Error(err) => {
                    progress.report(vec![structural_stderr(format!("error: {err:?}"))]);
                }
                AgentUpdate::TokenUsage(usage) => {
                    // Update the parent tool card header (model + token count)
                    // instead of cluttering the output stream with inline lines.
                    let _ = inner.send(AgentUpdate::ToolMeta {
                        tool_id: progress.tool_id().to_string(),
                        model: None,
                        token_usage: Some(usage),
                    });
                }
                AgentUpdate::ModelInfo(params) => {
                    let _ = inner.send(AgentUpdate::ToolMeta {
                        tool_id: progress.tool_id().to_string(),
                        model: Some(params.model),
                        token_usage: None,
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
                AgentUpdate::TaskComplete(_) | AgentUpdate::TaskCancelled => {}
                // Unused/unknown variants — skip silently.
                _ => {}
            }
        }
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
        let tagged = tagged_ui_channel_with_progress(inner_tx, progress);

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
        assert!(inner_rx.try_recv().is_err(), "nothing on inner");
    }

    #[tokio::test]
    async fn thinking_finished_emits_summary_with_prefix() {
        let (inner_tx, _inner_rx) = tokio::sync::mpsc::unbounded_channel();
        let (progress_tx, mut progress_rx) = tokio::sync::mpsc::unbounded_channel();
        let progress = ToolProgressReporter::new("t1", Some(progress_tx));
        let tagged = tagged_ui_channel_with_progress(inner_tx, progress);

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
        let tagged = tagged_ui_channel_with_progress(inner_tx, progress);

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
        let tagged = tagged_ui_channel_with_progress(inner_tx, progress);

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
        let tagged = tagged_ui_channel_with_progress(inner_tx, progress);

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
        let tagged = tagged_ui_channel_with_progress(inner_tx, progress);

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
        let tagged = tagged_ui_channel_with_progress(inner_tx, progress);

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
        let tagged = tagged_ui_channel_with_progress(inner_tx, progress);

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
            } => {
                assert_eq!(tool_id, "t1");
                assert!(model.is_none());
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
        let tagged = tagged_ui_channel_with_progress(inner_tx, progress);

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
        let tagged = tagged_ui_channel_with_progress(inner_tx, progress);

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
            } => {
                assert_eq!(tool_id, "t1");
                assert_eq!(model.unwrap(), "fake");
                assert!(token_usage.is_none());
            }
            other => panic!("expected ToolMeta, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn task_complete_and_cancelled_are_dropped() {
        let (inner_tx, mut inner_rx) = tokio::sync::mpsc::unbounded_channel();
        let (progress_tx, mut progress_rx) = tokio::sync::mpsc::unbounded_channel();
        let progress = ToolProgressReporter::new("t1", Some(progress_tx));
        let tagged = tagged_ui_channel_with_progress(inner_tx, progress);

        tagged
            .send(AgentUpdate::TaskComplete("done".into()))
            .unwrap();
        tagged.send(AgentUpdate::TaskCancelled).unwrap();
        tokio::task::yield_now().await;

        assert!(progress_rx.try_recv().is_err());
        assert!(inner_rx.try_recv().is_err());
    }
}
