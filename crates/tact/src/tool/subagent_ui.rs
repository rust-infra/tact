//! Tag subagent `ui_tx` traffic so the TUI can route it to the Subagent sticky.

use tact_protocol::AgentUpdate;
use tokio::sync::mpsc::UnboundedSender;

/// How a subagent update should be forwarded onto the parent UI channel.
#[derive(Debug)]
pub(crate) enum SubagentForward {
    /// Wrap as [`AgentUpdate::Subagent`].
    Tag(AgentUpdate),
    /// Send unchanged (permission / ask_user popups).
    Passthrough(AgentUpdate),
    /// Drop (must not end the parent task).
    Drop,
}

/// Classify an update emitted by a nested subagent.
pub(crate) fn classify_subagent_update(update: AgentUpdate) -> SubagentForward {
    match update {
        AgentUpdate::RequestSelect { .. } | AgentUpdate::RequestMultiSelect { .. } => {
            SubagentForward::Passthrough(update)
        }
        AgentUpdate::TaskComplete(_) | AgentUpdate::TaskCancelled => SubagentForward::Drop,
        // Avoid double-wrapping if a forwarder is stacked.
        AgentUpdate::Subagent { update, .. } => classify_subagent_update(*update),
        other => SubagentForward::Tag(other),
    }
}

/// Spawn a forwarder that tags subagent updates onto `inner`.
///
/// Returns a new sender for the subagent to use as `ui_tx`.
pub(crate) fn tagged_ui_channel(
    inner: UnboundedSender<AgentUpdate>,
    parent_tool_id: String,
    session_id: String,
) -> UnboundedSender<AgentUpdate> {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    tokio::spawn(async move {
        while let Some(update) = rx.recv().await {
            match classify_subagent_update(update) {
                SubagentForward::Passthrough(u) => {
                    let _ = inner.send(u);
                }
                SubagentForward::Drop => {}
                SubagentForward::Tag(u) => {
                    let _ = inner.send(AgentUpdate::Subagent {
                        parent_tool_id: parent_tool_id.clone(),
                        session_id: session_id.clone(),
                        update: Box::new(u),
                    });
                }
            }
        }
    });
    tx
}

#[cfg(test)]
mod tests {
    use super::*;
    use tact_protocol::TokenUsageInfo;

    #[test]
    fn classifies_stream_as_tag() {
        assert!(matches!(
            classify_subagent_update(AgentUpdate::StreamChunk("hi".into())),
            SubagentForward::Tag(AgentUpdate::StreamChunk(_))
        ));
    }

    #[test]
    fn classifies_token_usage_as_tag() {
        assert!(matches!(
            classify_subagent_update(AgentUpdate::TokenUsage(TokenUsageInfo::default())),
            SubagentForward::Tag(AgentUpdate::TokenUsage(_))
        ));
    }

    #[test]
    fn passthrough_select() {
        let (respond, _) = tokio::sync::oneshot::channel();
        assert!(matches!(
            classify_subagent_update(AgentUpdate::RequestSelect {
                prompt: "ok?".into(),
                options: vec!["a".into()],
                respond,
                log_confirm: false,
            }),
            SubagentForward::Passthrough(AgentUpdate::RequestSelect { .. })
        ));
    }

    #[test]
    fn drops_task_complete() {
        assert!(matches!(
            classify_subagent_update(AgentUpdate::TaskComplete("done".into())),
            SubagentForward::Drop
        ));
    }

    #[tokio::test]
    async fn forwarder_tags_stream_chunks() {
        let (inner_tx, mut inner_rx) = tokio::sync::mpsc::unbounded_channel();
        let tagged = tagged_ui_channel(inner_tx, "task-1".into(), "child-sess".into());
        tagged
            .send(AgentUpdate::StreamChunk("hello".into()))
            .unwrap();
        // Yield so the forwarder task runs.
        tokio::task::yield_now().await;
        let got = inner_rx.try_recv().unwrap();
        match got {
            AgentUpdate::Subagent {
                parent_tool_id,
                session_id,
                update,
            } => {
                assert_eq!(parent_tool_id, "task-1");
                assert_eq!(session_id, "child-sess");
                assert!(matches!(*update, AgentUpdate::StreamChunk(s) if s == "hello"));
            }
            other => panic!("expected Subagent, got {other:?}"),
        }
    }
}
