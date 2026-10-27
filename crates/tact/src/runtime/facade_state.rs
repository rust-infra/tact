//! Product-owned state transferred across one kernel activation.
//!
//! The kernel owns the logical transcript and cursor for a bounded run. Tact
//! keeps the rest of the facade's behavioral state here so an activation can
//! move it out and restore it through one guard, including on cancellation and
//! unwind.

use std::sync::{Arc, Mutex};

use tact_agent_api::{AgentMessage, AgentRole, AgentState, MessageRange};

use crate::{
    compact::CompactState,
    hook::{HookContextChunk, SessionStartSource},
    recovery::RecoveryState,
    tool::GoalYieldReason,
};

/// The two authorities moved across a kernel activation.
pub(crate) struct FacadeActivationState {
    pub state: AgentState,
    pub sidecar: FacadeSidecar,
    /// Original facade transcript shapes. The kernel works with neutral
    /// messages, so an unchanged transcript would otherwise turn legacy text
    /// shorthand into one-text-block messages on the way back.
    pub original_context: Vec<tact_llm::Message>,
}

/// Facade-only state that must survive a kernel run.
///
/// Shared queues, stats, the permission authority, the MCP catalog, and the
/// cancel flag deliberately stay outside this envelope.
pub(crate) struct FacadeSidecar {
    pub first_message_db_id: i64,
    pub last_message_db_id: i64,
    pub llm_call_last_message_id: i64,
    pub compact_state: CompactState,
    pub recovery_state: RecoveryState,
    pub goal_yield_reason: Option<GoalYieldReason>,
    pub max_turns_reached: bool,
    pub tool_use_counter: usize,
    pub interrupt_hooks_fired: bool,
    pub session_start_hooks_pending: bool,
    pub session_start_source: SessionStartSource,
    pub pending_session_context: Vec<HookContextChunk>,
    pub cached_dir_snapshot: Option<String>,
    pub cached_agents_md: Option<String>,
}

impl FacadeSidecar {
    pub(crate) fn apply_receipts(&mut self, receipts: FacadeReceipts) {
        self.first_message_db_id = receipts.first_message_db_id;
        self.last_message_db_id = receipts.last_message_db_id;
        self.llm_call_last_message_id = receipts.llm_call_last_message_id;
    }
}

/// Opaque store coordinates retained by the facade.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct FacadeReceipts {
    pub first_message_db_id: i64,
    pub last_message_db_id: i64,
    pub llm_call_last_message_id: i64,
}

/// Receives opaque store receipts without exposing row ids to the kernel API.
#[derive(Debug)]
pub(crate) struct FacadeReceiptTracker {
    receipts: Mutex<FacadeReceipts>,
}

impl FacadeReceiptTracker {
    pub(crate) fn new(receipts: FacadeReceipts) -> Arc<Self> {
        Arc::new(Self {
            receipts: Mutex::new(receipts),
        })
    }

    pub(crate) fn snapshot(&self) -> FacadeReceipts {
        *self
            .receipts
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Record one successful append. An assistant append marks the range sent
    /// to the model before its response row, matching the legacy anchor.
    pub(crate) fn record_append(&self, message: &AgentMessage, range: MessageRange) {
        let mut receipts = self
            .receipts
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if message.role == AgentRole::Assistant {
            receipts.llm_call_last_message_id = receipts.last_message_db_id;
        }
        if receipts.first_message_db_id == 0 {
            receipts.first_message_db_id = i64::try_from(range.start).unwrap_or(i64::MAX);
        }
        receipts.last_message_db_id = i64::try_from(range.end).unwrap_or(i64::MAX);
    }

    /// Record a successful whole-transcript replacement.
    pub(crate) fn record_replace(&self, range: Option<MessageRange>) {
        let mut receipts = self
            .receipts
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let (first, last) = range
            .map(|range| {
                (
                    i64::try_from(range.start).unwrap_or(i64::MAX),
                    i64::try_from(range.end).unwrap_or(i64::MAX),
                )
            })
            .unwrap_or((0, 0));
        receipts.first_message_db_id = first;
        receipts.last_message_db_id = last;
        receipts.llm_call_last_message_id = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tact_agent_api::{AgentRole, MessageOrigin};

    #[test]
    fn receipts_track_the_first_window_and_the_pre_assistant_anchor() {
        let tracker = FacadeReceiptTracker::new(FacadeReceipts::default());
        let user = AgentMessage::text(AgentRole::User, MessageOrigin::User, "question");
        tracker.record_append(&user, MessageRange { start: 4, end: 4 });
        let assistant =
            AgentMessage::text(AgentRole::Assistant, MessageOrigin::Assistant, "answer");
        tracker.record_append(&assistant, MessageRange { start: 5, end: 5 });

        assert_eq!(
            tracker.snapshot(),
            FacadeReceipts {
                first_message_db_id: 4,
                last_message_db_id: 5,
                llm_call_last_message_id: 4,
            }
        );
    }

    #[test]
    fn replacement_resets_the_range_and_anchor() {
        let tracker = FacadeReceiptTracker::new(FacadeReceipts {
            first_message_db_id: 1,
            last_message_db_id: 9,
            llm_call_last_message_id: 8,
        });
        tracker.record_replace(Some(MessageRange { start: 10, end: 12 }));
        assert_eq!(
            tracker.snapshot(),
            FacadeReceipts {
                first_message_db_id: 10,
                last_message_db_id: 12,
                llm_call_last_message_id: 0,
            }
        );
    }
}
