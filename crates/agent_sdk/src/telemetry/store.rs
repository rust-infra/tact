use std::sync::Arc;

use async_trait::async_trait;
use tact_agent_api::{
    AgentMessage, CursorUpdate, MessageReceipt, ReplaceReceipt, SessionRef, StoreError,
    TranscriptSnapshot, TranscriptStore,
};
use tracing::info_span;

pub(crate) struct InstrumentedTranscriptStore {
    inner: Arc<dyn TranscriptStore>,
}

impl InstrumentedTranscriptStore {
    pub(crate) fn new(inner: Arc<dyn TranscriptStore>) -> Self {
        Self { inner }
    }
}

#[async_trait]
impl TranscriptStore for InstrumentedTranscriptStore {
    async fn load(&self, session: &SessionRef) -> Result<TranscriptSnapshot, StoreError> {
        let span = info_span!(
            "agent.store.load",
            session_id = %session.as_str(),
            duration_ms = tracing::field::Empty
        );
        super::timed(span, self.inner.load(session)).await
    }

    async fn append(
        &self,
        session: &SessionRef,
        message: &AgentMessage,
        cursor: CursorUpdate,
    ) -> Result<MessageReceipt, StoreError> {
        let span = info_span!(
            "agent.store.append",
            session_id = %session.as_str(),
            duration_ms = tracing::field::Empty
        );
        super::timed(span, self.inner.append(session, message, cursor)).await
    }

    async fn replace(
        &self,
        session: &SessionRef,
        transcript: Option<&[AgentMessage]>,
        cursor: CursorUpdate,
    ) -> Result<ReplaceReceipt, StoreError> {
        let span = info_span!(
            "agent.store.replace",
            session_id = %session.as_str(),
            duration_ms = tracing::field::Empty
        );
        super::timed(span, self.inner.replace(session, transcript, cursor)).await
    }
}
