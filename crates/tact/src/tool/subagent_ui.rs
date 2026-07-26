//! Forward subagent `ui_tx` traffic — currently a no-op while the sticky pane
//! is removed. In a future step this will push stream chunks as `ToolProgress`
//! for parent tool-card live output.

use tact_protocol::AgentUpdate;
use tokio::sync::mpsc::UnboundedSender;

/// Spawn a forwarder that drops subagent updates.
///
/// Returns a new sender for the subagent to use as `ui_tx`, keeping the channel
/// contract without routing any content to the parent UI.
pub fn tagged_ui_channel(
    _inner: UnboundedSender<AgentUpdate>,
    _parent_tool_id: String,
    _session_id: String,
) -> UnboundedSender<AgentUpdate> {
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    // All subagent updates are dropped silently — no sticky pane.
    // Future: forward StreamChunk as ToolProgress to parent tool card.
    tx
}
