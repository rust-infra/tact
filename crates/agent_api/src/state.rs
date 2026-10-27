use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::{AgentContentBlock, AgentMessage, AgentRole, MessageId, MessageOrigin, ModelCursor};

/// Identity for one bounded kernel activation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RunId(u64);

impl RunId {
    pub const fn new(id: u64) -> Self {
        Self(id)
    }

    pub const fn get(self) -> u64 {
        self.0
    }
}

/// Opaque logical session identity supplied by the host.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SessionRef(String);

impl SessionRef {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Activation-scoped cancellation signal.
#[derive(Debug, Clone, Default)]
pub struct CancelToken {
    token: CancellationToken,
    source: Option<Arc<AtomicBool>>,
}

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }

    /// Follow an existing public flag synchronously. The token only observes
    /// the flag; cancelling the token never writes back to it. The activation
    /// bridge is responsible for waking async waiters when the flag changes.
    pub fn from_flag(flag: Arc<AtomicBool>) -> Self {
        let token = CancellationToken::new();
        if flag.load(Ordering::Relaxed) {
            token.cancel();
        }
        Self {
            token,
            source: Some(flag),
        }
    }

    pub fn cancel(&self) {
        self.token.cancel();
    }

    pub fn is_cancelled(&self) -> bool {
        self.token.is_cancelled()
            || self
                .source
                .as_ref()
                .is_some_and(|flag| flag.load(Ordering::Relaxed))
    }

    pub async fn cancelled(&self) {
        if self.is_cancelled() {
            return;
        }
        self.token.cancelled().await;
    }

    pub fn child_token(&self) -> Self {
        Self {
            token: self.token.child_token(),
            source: self.source.clone(),
        }
    }
}

/// Inclusive message range returned by a transcript adapter. The values are
/// opaque receipt coordinates, not database columns exposed to the kernel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageRange {
    pub start: u64,
    pub end: u64,
}

/// Host-owned logical agent state.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AgentState {
    pub transcript: Vec<AgentMessage>,
    pub cursor: Option<ModelCursor>,
    pub total_turns: u32,
    pub last_token_total: Option<u32>,
    pub initialized: bool,
}

/// New input for one activation.
#[derive(Debug, Clone)]
pub struct RunInput {
    pub session: SessionRef,
    pub inputs: Vec<AgentMessage>,
    pub max_total_turns: Option<u32>,
}

impl RunInput {
    pub fn new(session: SessionRef) -> Self {
        Self {
            session,
            inputs: Vec::new(),
            max_total_turns: None,
        }
    }
}

/// Final result of one bounded activation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunOutcome {
    pub stop: RunStop,
    pub turns: u32,
    pub total_turns: u32,
    pub tool_calls: u64,
    /// Deterministic recovery counters for this activation.
    pub recovery: RecoveryStats,
}

/// Recovery work performed during one bounded activation.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoveryStats {
    /// Context-overflow compactions attempted by this activation.
    pub compact_attempts: u32,
    /// Transient model failures retried by this activation.
    pub transport_attempts: u32,
    /// Max-token continuation requests issued by this activation.
    pub continuation_attempts: u32,
}

/// Why an activation returned.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RunStop {
    Completed,
    YieldToHost { reason: HostYieldReason },
    MaxTurns,
    MaxTokens,
    Cancelled,
    Blocked { phase: RunPhase, reason: String },
}

/// Product-neutral host yield reasons.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HostYieldReason {
    NeedsUser,
}

/// Where a run stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunPhase {
    Load,
    Reconcile,
    PrepareRun,
    PrepareRequest,
    Stream,
    Gate,
    Execute,
    Commit,
}

/// Serializable terminal error information for host reporting.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeErrorInfo {
    pub phase: RunPhase,
    pub message: String,
    pub detail: Option<String>,
}

impl RuntimeErrorInfo {
    pub fn new(phase: RunPhase, message: impl Into<String>) -> Self {
        Self {
            phase,
            message: message.into(),
            detail: None,
        }
    }
}

/// Convenience constructor for a grouped tool-result message.
pub fn grouped_tool_results(results: Vec<(String, String)>) -> AgentMessage {
    AgentMessage::new(
        AgentRole::User,
        MessageOrigin::ToolResult,
        results
            .into_iter()
            .map(|(tool_use_id, content)| AgentContentBlock::ToolResult {
                tool_use_id,
                content,
            })
            .collect(),
    )
}

/// Convenience constructor for one assistant response.
pub fn assistant_message(
    message_id: impl Into<String>,
    content: Vec<AgentContentBlock>,
) -> (MessageId, AgentMessage) {
    (
        MessageId::new(message_id),
        AgentMessage::new(AgentRole::Assistant, MessageOrigin::Assistant, content),
    )
}

/// Shared cancellation handle retained for adapters that need cheap clones.
pub type SharedCancelToken = Arc<CancelToken>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grouped_tool_results_are_one_ordered_message() {
        let message = grouped_tool_results(vec![
            ("call_1".into(), "first".into()),
            ("call_2".into(), "second".into()),
        ]);
        assert_eq!(message.role, AgentRole::User);
        assert_eq!(message.origin, MessageOrigin::ToolResult);
        assert_eq!(
            message.tool_result_ids().collect::<Vec<_>>(),
            ["call_1", "call_2"]
        );
    }

    #[test]
    fn cancel_token_is_observable_and_cloneable() {
        let token = CancelToken::new();
        let clone = token.clone();
        token.cancel();
        assert!(clone.is_cancelled());
    }
}
