//! Production sessionless transcript adapter.

use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

use async_trait::async_trait;
use tact_agent_api::{
    AgentMessage, CursorUpdate, MessageRange, MessageReceipt, ReplaceReceipt, SessionRef,
    StoreError, TranscriptSnapshot, TranscriptStore,
};

/// Preserves the transcript commit barrier without retaining durable rows.
#[derive(Clone, Default)]
pub(crate) struct NoopTranscriptStore {
    committed: Arc<AtomicU64>,
}

fn message_weight(message: &AgentMessage) -> u64 {
    message
        .content
        .iter()
        .map(|block| format!("{block:?}").len().max(1) as u64)
        .sum::<u64>()
        .max(1)
}

#[async_trait]
impl TranscriptStore for NoopTranscriptStore {
    async fn load(&self, _session: &SessionRef) -> Result<TranscriptSnapshot, StoreError> {
        Ok(TranscriptSnapshot::default())
    }

    async fn append(
        &self,
        _session: &SessionRef,
        message: &AgentMessage,
        _cursor: CursorUpdate,
    ) -> Result<MessageReceipt, StoreError> {
        let previous = self.committed.load(Ordering::Relaxed);
        let end = previous.saturating_add(message_weight(message));
        self.committed.store(end, Ordering::Relaxed);
        Ok(MessageReceipt {
            range: MessageRange {
                start: previous.saturating_add(1),
                end,
            },
        })
    }

    async fn replace(
        &self,
        _session: &SessionRef,
        transcript: Option<&[AgentMessage]>,
        _cursor: CursorUpdate,
    ) -> Result<ReplaceReceipt, StoreError> {
        match transcript {
            Some([]) => {
                self.committed.store(0, Ordering::Relaxed);
                Ok(ReplaceReceipt { range: None })
            }
            Some(messages) => {
                let end = messages.iter().map(message_weight).sum::<u64>().max(1);
                self.committed.store(end, Ordering::Relaxed);
                Ok(ReplaceReceipt {
                    range: Some(MessageRange { start: 1, end }),
                })
            }
            None => Ok(ReplaceReceipt { range: None }),
        }
    }
}

#[cfg(test)]
mod tests {
    use tact_agent_api::{AgentRole, MessageOrigin};

    use super::*;

    #[tokio::test]
    async fn sessionless_commits_return_monotonic_receipts() {
        let store = NoopTranscriptStore::default();
        let session = SessionRef::new("sessionless");
        let first = AgentMessage::text(AgentRole::User, MessageOrigin::User, "hello");
        let second = AgentMessage::text(AgentRole::Assistant, MessageOrigin::Assistant, "world");

        let first_receipt = store
            .append(&session, &first, CursorUpdate::Unchanged)
            .await
            .unwrap();
        let second_receipt = store
            .append(&session, &second, CursorUpdate::Unchanged)
            .await
            .unwrap();

        assert_eq!(first_receipt.range.start, 1);
        assert_eq!(second_receipt.range.start, first_receipt.range.end + 1);
        assert!(store.load(&session).await.unwrap().transcript.is_empty());
    }

    #[tokio::test]
    async fn an_explicit_clear_clears_the_noop_receipt_sequence() {
        let store = NoopTranscriptStore::default();
        let session = SessionRef::new("sessionless");
        store
            .append(
                &session,
                &AgentMessage::text(AgentRole::User, MessageOrigin::User, "hello"),
                CursorUpdate::Unchanged,
            )
            .await
            .unwrap();

        let cleared = store
            .replace(&session, Some(&[]), CursorUpdate::Replace { cursor: None })
            .await
            .unwrap();
        assert_eq!(cleared.range, None);
        let next = store
            .append(
                &session,
                &AgentMessage::text(AgentRole::User, MessageOrigin::User, "again"),
                CursorUpdate::Unchanged,
            )
            .await
            .unwrap();
        assert_eq!(next.range.start, 1);
    }
}
