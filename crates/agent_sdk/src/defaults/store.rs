use std::sync::{Arc, Mutex, PoisonError};

use async_trait::async_trait;
use tact_agent_api::{
    AgentMessage, CursorUpdate, MessageRange, MessageReceipt, ReplaceReceipt, SessionRef,
    StoreError, TranscriptSnapshot, TranscriptStore,
};

/// In-process transcript barrier that retains no durable rows.
#[derive(Clone, Default)]
pub struct NoopTranscriptStore {
    committed: Arc<Mutex<u64>>,
}

fn message_weight(message: &AgentMessage) -> u64 {
    message.content.len().max(1) as u64
}

fn allocator_range(committed: &mut u64, count: u64) -> MessageRange {
    let start = committed.saturating_add(1);
    let end = committed.saturating_add(count.max(1));
    *committed = end;
    MessageRange { start, end }
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
        let mut committed = self
            .committed
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let range = allocator_range(&mut committed, message_weight(message));
        Ok(MessageReceipt { range })
    }

    async fn replace(
        &self,
        _session: &SessionRef,
        transcript: Option<&[AgentMessage]>,
        _cursor: CursorUpdate,
    ) -> Result<ReplaceReceipt, StoreError> {
        let mut committed = self
            .committed
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        match transcript {
            Some([]) => Ok(ReplaceReceipt { range: None }),
            Some(messages) => {
                let count = messages.iter().map(message_weight).sum::<u64>().max(1);
                let range = allocator_range(&mut committed, count);
                Ok(ReplaceReceipt { range: Some(range) })
            }
            None => Ok(ReplaceReceipt { range: None }),
        }
    }
}

#[cfg(test)]
mod tests {
    use tact_agent_api::{
        AgentMessage, AgentRole, CursorUpdate, MessageOrigin, SessionRef, TranscriptStore,
    };

    use super::*;

    #[tokio::test]
    async fn receipts_are_monotonic_and_clear_never_rewinds_the_allocator() {
        let store = NoopTranscriptStore::default();
        let session = SessionRef::new("s");
        let first = store
            .append(
                &session,
                &AgentMessage::text(AgentRole::User, MessageOrigin::User, "one"),
                CursorUpdate::Unchanged,
            )
            .await
            .unwrap();
        let second = store
            .append(
                &session,
                &AgentMessage::text(AgentRole::Assistant, MessageOrigin::Assistant, "two"),
                CursorUpdate::Unchanged,
            )
            .await
            .unwrap();
        assert!(second.range.start > first.range.end);
        assert!(store.load(&session).await.unwrap().transcript.is_empty());

        let replacement = store
            .replace(
                &session,
                Some(&[AgentMessage::text(
                    AgentRole::User,
                    MessageOrigin::User,
                    "replacement",
                )]),
                CursorUpdate::Unchanged,
            )
            .await
            .unwrap()
            .range
            .unwrap();
        assert!(replacement.start > second.range.end);

        assert_eq!(
            store
                .replace(&session, Some(&[]), CursorUpdate::Unchanged)
                .await
                .unwrap()
                .range,
            None
        );
        let after_clear = store
            .append(
                &session,
                &AgentMessage::text(AgentRole::User, MessageOrigin::User, "again"),
                CursorUpdate::Unchanged,
            )
            .await
            .unwrap();
        assert!(after_clear.range.start > replacement.end);
    }
}
