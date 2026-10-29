//! Small fake adapters for contract and kernel tests.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;

use crate::{
    AgentMessage, CursorUpdate, MessageRange, MessageReceipt, ModelCursor, ReplaceReceipt,
    SessionRef, SessionStateSnapshot, StoreError, TranscriptSnapshot, TranscriptStore, UsageError,
    UsageRecord, UsageSink,
};

#[derive(Default)]
struct InMemoryTranscriptInner {
    transcripts: HashMap<String, Vec<AgentMessage>>,
    cursors: HashMap<String, Option<ModelCursor>>,
    ranges: HashMap<String, MessageRange>,
    states: HashMap<String, SessionStateSnapshot>,
    next_id: u64,
}

/// Sessionless in-memory persistence barrier for tests.
#[derive(Default)]
pub struct InMemoryTranscriptStore {
    inner: Mutex<InMemoryTranscriptInner>,
}

impl InMemoryTranscriptStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn transcript(&self, session: &SessionRef) -> Vec<AgentMessage> {
        self.inner
            .lock()
            .expect("in-memory transcript lock")
            .transcripts
            .get(session.as_str())
            .cloned()
            .unwrap_or_default()
    }

    fn allocate(inner: &mut InMemoryTranscriptInner, count: usize) -> MessageRange {
        let start = inner.next_id;
        inner.next_id = inner.next_id.saturating_add(count.max(1) as u64);
        MessageRange {
            start,
            end: inner.next_id.saturating_sub(1),
        }
    }
}

#[async_trait]
impl TranscriptStore for InMemoryTranscriptStore {
    async fn load(&self, session: &SessionRef) -> Result<TranscriptSnapshot, StoreError> {
        let inner = self
            .inner
            .lock()
            .map_err(|_| StoreError::Load("transcript lock poisoned".into()))?;
        Ok(TranscriptSnapshot {
            transcript: inner
                .transcripts
                .get(session.as_str())
                .cloned()
                .unwrap_or_default(),
            cursor: inner.cursors.get(session.as_str()).cloned().flatten(),
            range: inner.ranges.get(session.as_str()).copied(),
        })
    }

    async fn append(
        &self,
        session: &SessionRef,
        message: &AgentMessage,
        cursor: CursorUpdate,
    ) -> Result<MessageReceipt, StoreError> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| StoreError::Append("transcript lock poisoned".into()))?;
        inner
            .transcripts
            .entry(session.as_str().to_owned())
            .or_default()
            .push(message.clone());
        if let CursorUpdate::Replace { cursor } = cursor {
            inner.cursors.insert(session.as_str().to_owned(), cursor);
        }
        let range = Self::allocate(&mut inner, 1);
        inner.ranges.insert(session.as_str().to_owned(), range);
        Ok(MessageReceipt { range })
    }

    async fn replace(
        &self,
        session: &SessionRef,
        transcript: Option<&[AgentMessage]>,
        cursor: CursorUpdate,
    ) -> Result<ReplaceReceipt, StoreError> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| StoreError::Replace("transcript lock poisoned".into()))?;
        if let Some(transcript) = transcript {
            if transcript.is_empty() {
                inner.transcripts.remove(session.as_str());
                inner.ranges.remove(session.as_str());
            } else {
                inner
                    .transcripts
                    .insert(session.as_str().to_owned(), transcript.to_vec());
                let range = Self::allocate(&mut inner, transcript.len());
                inner.ranges.insert(session.as_str().to_owned(), range);
            }
        }
        if let CursorUpdate::Replace { cursor } = cursor {
            inner.cursors.insert(session.as_str().to_owned(), cursor);
        }
        Ok(ReplaceReceipt {
            range: inner.ranges.get(session.as_str()).copied(),
        })
    }

    async fn load_state(
        &self,
        session: &SessionRef,
    ) -> Result<Option<SessionStateSnapshot>, StoreError> {
        let inner = self
            .inner
            .lock()
            .map_err(|_| StoreError::Load("state lock poisoned".into()))?;
        Ok(inner.states.get(session.as_str()).cloned())
    }

    async fn save_state(
        &self,
        session: &SessionRef,
        state: &SessionStateSnapshot,
    ) -> Result<(), StoreError> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| StoreError::Append("state lock poisoned".into()))?;
        inner
            .states
            .insert(session.as_str().to_owned(), state.clone());
        Ok(())
    }
}

/// Thread-safe recording usage sink.
#[derive(Default)]
pub struct RecordingUsageSink {
    records: Mutex<Vec<UsageRecord>>,
    flushes: Mutex<u32>,
}

impl RecordingUsageSink {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn records(&self) -> Vec<UsageRecord> {
        self.records.lock().expect("usage records lock").clone()
    }

    pub fn flush_count(&self) -> u32 {
        *self.flushes.lock().expect("usage flush lock")
    }
}

#[async_trait]
impl UsageSink for RecordingUsageSink {
    async fn record(&self, record: UsageRecord) -> Result<(), UsageError> {
        let mut records = self
            .records
            .lock()
            .map_err(|_| UsageError::Record("usage lock poisoned".into()))?;
        if !records
            .iter()
            .any(|existing| existing.call_id == record.call_id)
        {
            records.push(record);
        }
        Ok(())
    }

    async fn flush(&self) -> Result<(), UsageError> {
        let mut flushes = self
            .flushes
            .lock()
            .map_err(|_| UsageError::Flush("flush lock poisoned".into()))?;
        *flushes = flushes.saturating_add(1);
        Ok(())
    }
}

/// Production-style no-op usage sink for a sessionless host.
pub struct NoopUsageSink;

#[async_trait]
impl UsageSink for NoopUsageSink {
    async fn record(&self, _record: UsageRecord) -> Result<(), UsageError> {
        Ok(())
    }

    async fn flush(&self) -> Result<(), UsageError> {
        Ok(())
    }
}

/// Convenience alias for shared recording sinks.
pub type SharedRecordingUsageSink = Arc<RecordingUsageSink>;
