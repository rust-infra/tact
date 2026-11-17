use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::{
    AgentMessage, CancelToken, CursorUpdate, ModelCallOptions, ModelCapabilities, ModelCursor,
    ModelMessage, ModelPort, RunInput, RunStop, SessionRef,
};

/// Provider-neutral request assembled by product context policy before the
/// kernel attaches the current tool snapshot.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PreparedRequest {
    pub system: String,
    pub messages: Vec<ModelMessage>,
    pub cursor: Option<ModelCursor>,
    pub options: ModelCallOptions,
}

/// Read-only state exposed to context policy.
pub struct ContextView<'a> {
    pub session: &'a SessionRef,
    pub transcript: &'a [AgentMessage],
    pub cursor: Option<&'a ModelCursor>,
    pub total_turns: u32,
    pub max_total_turns: Option<u32>,
    /// Last provider-reported token total for the logical transcript.
    ///
    /// Product context policies use this for the same usage-first
    /// auto-compaction estimate the legacy facade applied. `None` means no
    /// authoritative usage has been observed in this session yet.
    pub last_token_total: Option<u32>,
    pub pending_inputs: &'a [AgentMessage],
    pub capabilities: &'a ModelCapabilities,
    /// Model calls permitted from the prospective current request onward.
    pub remaining_turns: Option<u32>,
}

/// Typed preparation decision. `Compact` carries a non-looping fallback.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PreparationDecision<T> {
    Continue(T),
    Compact {
        request: CompactionRequest,
        if_skipped: T,
    },
    Block(RunStop),
}

/// Context persisted before the first request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextPreparation {
    pub inputs: Vec<AgentMessage>,
    pub appended_messages: Vec<AgentMessage>,
}

/// Request plus context that must be persisted before dispatch.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RequestPlan {
    pub request: PreparedRequest,
    pub appended_messages: Vec<AgentMessage>,
}

/// Typed request for compaction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CompactionRequest {
    Auto { focus: Option<String> },
    Manual { focus: Option<String> },
    Recovery { focus: Option<String> },
    Command { focus: Option<String> },
    Micro,
}

impl CompactionRequest {
    pub fn kind(&self) -> CompactionKind {
        match self {
            Self::Auto { .. } => CompactionKind::Auto,
            Self::Manual { .. } => CompactionKind::Manual,
            Self::Recovery { .. } => CompactionKind::Recovery,
            Self::Command { .. } => CompactionKind::Command,
            Self::Micro => CompactionKind::Micro,
        }
    }

    pub fn focus(&self) -> Option<&str> {
        match self {
            Self::Auto { focus }
            | Self::Manual { focus }
            | Self::Recovery { focus }
            | Self::Command { focus } => focus.as_deref(),
            Self::Micro => None,
        }
    }
}

/// Stable compaction classification for events and usage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompactionKind {
    Auto,
    Manual,
    Recovery,
    Command,
    Micro,
}

/// Result of asking context policy to compact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CompactionOutcome {
    Replaced(ContextReplacement),
    Skipped { reason: String },
}

/// Atomic transcript/cursor replacement.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextReplacement {
    /// `None` retains the transcript; `Some(vec![])` explicitly clears it.
    pub transcript: Option<Vec<AgentMessage>>,
    pub cursor: CursorUpdate,
    /// User-displayable summary of what replaced the transcript.
    ///
    /// For local summarization this is the prose summary. Native provider
    /// compaction replaces history inside the provider and yields no local
    /// text, so there it is the provider's read-out instead (the retained item
    /// count and the bounded compaction id, or a plain sentence when the
    /// cursor carries no product state) — a host that draws a compaction card
    /// needs a body, and an empty one reads as "nothing happened".
    ///
    /// It travels on the replacement because that value is the only channel the
    /// kernel has: the text is produced inside the context policy
    /// (`TactCompactor` keeps the local summary again in its own `last_summary`
    /// during `compaction_committed`), and the kernel holds the policy as
    /// `dyn ContextPolicy` and cannot read back through it. Putting it on the
    /// replacement keeps the text and the transcript it describes in one value
    /// that cannot disagree.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
}

/// Lifecycle of one compaction, classified by what triggered it.
///
/// [`AgentNotice::CompactionStarted`](crate::AgentNotice::CompactionStarted)
/// still drives the human-readable log line ("[auto compact]" and friends);
/// this carries the same lifecycle as data so a host that renders a compaction
/// card (ACP's `compaction_update`) can follow it without parsing text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompactionEvent {
    pub kind: CompactionKind,
    pub state: CompactionState,
}

/// Where a [`CompactionEvent`] sits in its lifecycle.
///
/// Every compaction that emits [`Self::Started`] emits exactly one terminal
/// state, so a host never has to fall back to "the turn ended, assume it
/// failed".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum CompactionState {
    Started,
    Completed {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        summary: Option<String>,
    },
    Failed {
        error: String,
    },
    /// Nothing was replaced: a pre-compaction hook vetoed it, the compactor
    /// found nothing worth shrinking, or the activation was cancelled.
    Cancelled {
        reason: String,
    },
}

impl CompactionEvent {
    pub fn started(kind: CompactionKind) -> Self {
        Self {
            kind,
            state: CompactionState::Started,
        }
    }

    pub fn completed(kind: CompactionKind, summary: Option<String>) -> Self {
        Self {
            kind,
            state: CompactionState::Completed { summary },
        }
    }

    pub fn failed(kind: CompactionKind, error: impl Into<String>) -> Self {
        Self {
            kind,
            state: CompactionState::Failed {
                error: error.into(),
            },
        }
    }

    pub fn cancelled(kind: CompactionKind, reason: impl Into<String>) -> Self {
        Self {
            kind,
            state: CompactionState::Cancelled {
                reason: reason.into(),
            },
        }
    }

    pub fn is_terminal(&self) -> bool {
        !matches!(self.state, CompactionState::Started)
    }
}

/// Context policy failure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
pub enum ContextError {
    #[error("context preparation failed: {0}")]
    Preparation(String),
    #[error("context compaction failed: {0}")]
    Compaction(String),
    #[error("context contract violation: {0}")]
    Contract(String),
    #[error("context policy cancelled")]
    Cancelled,
}

/// Required product context capability.
#[async_trait]
pub trait ContextPolicy: Send + Sync {
    async fn prepare_run(
        &self,
        input: &RunInput,
        state: &ContextView<'_>,
        model: &dyn ModelPort,
        cancel: CancelToken,
    ) -> Result<PreparationDecision<ContextPreparation>, ContextError>;

    async fn prepare_request(
        &self,
        state: &ContextView<'_>,
        model: &dyn ModelPort,
        cancel: CancelToken,
    ) -> Result<PreparationDecision<RequestPlan>, ContextError>;

    async fn compact(
        &self,
        state: &ContextView<'_>,
        request: CompactionRequest,
        model: &dyn ModelPort,
        cancel: CancelToken,
    ) -> Result<CompactionOutcome, ContextError>;

    /// Observe one replacement that the kernel has durably committed.
    ///
    /// The kernel calls this only after `TranscriptStore::replace` succeeds and
    /// the caller-owned transcript/cursor have been updated. Product policies
    /// use it to commit compaction bookkeeping and run PostCompact hooks
    /// without guessing whether persistence succeeded.
    async fn compaction_committed(
        &self,
        _state: &ContextView<'_>,
        _request: &CompactionRequest,
    ) -> Result<(), ContextError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_compaction_request_has_a_stable_kind() {
        let requests = [
            CompactionRequest::Auto { focus: None },
            CompactionRequest::Manual {
                focus: Some("tests".into()),
            },
            CompactionRequest::Recovery { focus: None },
            CompactionRequest::Command { focus: None },
            CompactionRequest::Micro,
        ];
        assert_eq!(
            requests
                .iter()
                .map(CompactionRequest::kind)
                .collect::<Vec<_>>(),
            [
                CompactionKind::Auto,
                CompactionKind::Manual,
                CompactionKind::Recovery,
                CompactionKind::Command,
                CompactionKind::Micro,
            ]
        );

        for request in requests {
            let json = serde_json::to_string(&request).unwrap();
            assert_eq!(
                serde_json::from_str::<CompactionRequest>(&json).unwrap(),
                request
            );
        }
    }

    #[test]
    fn replacement_distinguishes_retain_and_clear() {
        let retain = ContextReplacement {
            transcript: None,
            cursor: CursorUpdate::Replace { cursor: None },
            summary: None,
        };
        let clear = ContextReplacement {
            transcript: Some(Vec::new()),
            cursor: CursorUpdate::Replace { cursor: None },
            summary: None,
        };
        assert_ne!(retain, clear);
    }

    #[test]
    fn preparation_and_compaction_boundaries_round_trip() {
        let preparation = ContextPreparation {
            inputs: vec![AgentMessage::text(
                crate::AgentRole::User,
                crate::MessageOrigin::User,
                "question",
            )],
            appended_messages: vec![AgentMessage::text(
                crate::AgentRole::User,
                crate::MessageOrigin::GoalContext,
                "goal",
            )],
        };
        let json = serde_json::to_string(&preparation).unwrap();
        assert_eq!(
            serde_json::from_str::<ContextPreparation>(&json).unwrap(),
            preparation
        );

        let request = RequestPlan {
            request: PreparedRequest {
                system: "system".into(),
                messages: Vec::new(),
                cursor: None,
                options: ModelCallOptions::default(),
            },
            appended_messages: Vec::new(),
        };
        let json = serde_json::to_string(&request).unwrap();
        assert_eq!(serde_json::from_str::<RequestPlan>(&json).unwrap(), request);

        let outcome = CompactionOutcome::Replaced(ContextReplacement {
            transcript: Some(Vec::new()),
            cursor: CursorUpdate::Unchanged,
            summary: Some("condensed".into()),
        });
        let json = serde_json::to_string(&outcome).unwrap();
        assert_eq!(
            serde_json::from_str::<CompactionOutcome>(&json).unwrap(),
            outcome
        );

        let decision = PreparationDecision::Compact {
            request: CompactionRequest::Micro,
            if_skipped: (),
        };
        assert!(matches!(decision, PreparationDecision::Compact { .. }));
    }
}
