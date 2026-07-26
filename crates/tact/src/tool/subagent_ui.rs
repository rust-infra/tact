//! Forward subagent `ui_tx` traffic as `ToolProgress` for the parent tool card.

use tact_protocol::{AgentUpdate, ToolOutputChunk};
use tokio::sync::mpsc::UnboundedSender;

use crate::tool::ToolProgressReporter;

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
                            let line = format!("… {summary}");
                            progress.report(vec![ToolOutputChunk::other(line)]);
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
                    progress.report(vec![ToolOutputChunk::other(line)]);
                }
                AgentUpdate::StepFinished { result, .. } => {
                    let preview = result.message;
                    if !preview.is_empty() {
                        let line = format!("✓ {preview}");
                        progress.report(vec![ToolOutputChunk::other(line)]);
                    }
                }
                AgentUpdate::StepFailed { error, .. } => {
                    let line = format!("✗ {error}");
                    progress.report(vec![ToolOutputChunk::stderr(line)]);
                }
                AgentUpdate::Info(msg) => {
                    progress.report(vec![ToolOutputChunk::other(msg)]);
                }
                AgentUpdate::Error(err) => {
                    let line = format!("error: {err:?}");
                    progress.report(vec![ToolOutputChunk::stderr(line)]);
                }
                AgentUpdate::TokenUsage(usage) => {
                    let line = format!("⚡ {} tokens", usage.total);
                    progress.report(vec![ToolOutputChunk::other(line)]);
                }
                AgentUpdate::ModelInfo(_) => {}
                AgentUpdate::RequestSelect {
                    mut prompt,
                    options,
                    respond,
                    log_confirm,
                } => {
                    prompt = format!("[Subagent] {prompt}");
                    let _ = inner.send(AgentUpdate::RequestSelect {
                        prompt,
                        options,
                        respond,
                        log_confirm,
                    });
                }
                AgentUpdate::RequestMultiSelect {
                    mut prompt,
                    options,
                    respond,
                } => {
                    prompt = format!("[Subagent] {prompt}");
                    let _ = inner.send(AgentUpdate::RequestMultiSelect {
                        prompt,
                        options,
                        respond,
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
    use tact_protocol::{StepResult, StepStatus, ThinkingChunk, TokenUsageInfo};

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
                "reasoning".into(),
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
                assert!(chunks[0].text.starts_with("… "));
                assert!(chunks[0].text.contains("reasoning"));
            }
            other => panic!("expected ToolProgress, got {other:?}"),
        }
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
            })
            .unwrap();
        tagged
            .send(AgentUpdate::StepFailed {
                idx: 0,
                tool_id: "b1".into(),
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
                assert!(chunks[0].text.starts_with("✗ "));
                assert!(chunks[0].text.contains("not found"));
                assert!(matches!(chunks[0].stream, tact_protocol::ToolOutputStream::Stderr));
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
                },
            })
            .unwrap();
        tokio::task::yield_now().await;

        let got: Vec<AgentUpdate> = std::iter::from_fn(|| progress_rx.try_recv().ok()).collect();
        assert_eq!(got.len(), 1);
        match &got[0] {
            AgentUpdate::ToolProgress { chunks, .. } => {
                assert!(chunks[0].text.starts_with("✓ "));
                assert!(chunks[0].text.contains("hi"));
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

        let (respond, _) = tokio::sync::oneshot::channel();
        tagged
            .send(AgentUpdate::RequestSelect {
                prompt: "Allow bash?".into(),
                options: vec!["Yes".into()],
                respond,
                log_confirm: false,
            })
            .unwrap();
        tokio::task::yield_now().await;

        let got = inner_rx.try_recv().unwrap();
        match got {
            AgentUpdate::RequestSelect { prompt, .. } => {
                assert!(prompt.starts_with("[Subagent]"));
            }
            other => panic!("expected RequestSelect, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn token_usage_becomes_progress_chunk() {
        let (inner_tx, _inner_rx) = tokio::sync::mpsc::unbounded_channel();
        let (progress_tx, mut progress_rx) = tokio::sync::mpsc::unbounded_channel();
        let progress = ToolProgressReporter::new("t1", Some(progress_tx));
        let tagged = tagged_ui_channel_with_progress(inner_tx, progress);

        tagged
            .send(AgentUpdate::TokenUsage(TokenUsageInfo {
                total: 999,
                ..Default::default()
            }))
            .unwrap();
        tokio::task::yield_now().await;

        let got: Vec<AgentUpdate> = std::iter::from_fn(|| progress_rx.try_recv().ok()).collect();
        assert_eq!(got.len(), 1);
        match &got[0] {
            AgentUpdate::ToolProgress { chunks, .. } => {
                assert!(chunks[0].text.contains("⚡"));
                assert!(chunks[0].text.contains("999"));
            }
            other => panic!("expected ToolProgress, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn model_info_is_silently_ignored() {
        let (inner_tx, _inner_rx) = tokio::sync::mpsc::unbounded_channel();
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

        assert!(progress_rx.try_recv().is_err());
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
        tagged
            .send(AgentUpdate::TaskCancelled)
            .unwrap();
        tokio::task::yield_now().await;

        assert!(progress_rx.try_recv().is_err());
        assert!(inner_rx.try_recv().is_err());
    }
}
