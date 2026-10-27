//! Durable kernel transcript backed by the existing session store.

use async_trait::async_trait;
use tact_agent_api::{
    AgentMessage, CursorUpdate, MessageRange, MessageReceipt, ReplaceReceipt, SessionRef,
    StoreError, TranscriptSnapshot, TranscriptStore,
};
use tact_llm::{ProviderConversationState, neutral};

use crate::{runtime::facade_state::FacadeReceiptTracker, store::DynSessionStore};

pub(crate) struct SqliteTranscriptStore {
    store: DynSessionStore,
    receipts: Option<std::sync::Arc<FacadeReceiptTracker>>,
}

impl SqliteTranscriptStore {
    pub(crate) fn new(store: DynSessionStore) -> Self {
        Self {
            store,
            receipts: None,
        }
    }

    pub(crate) fn with_receipts(
        store: DynSessionStore,
        receipts: std::sync::Arc<FacadeReceiptTracker>,
    ) -> Self {
        Self {
            store,
            receipts: Some(receipts),
        }
    }
}

fn provider_update(
    cursor: CursorUpdate,
) -> Result<Option<Option<ProviderConversationState>>, StoreError> {
    match cursor {
        CursorUpdate::Unchanged => Ok(None),
        CursorUpdate::Replace { cursor: None } => Ok(Some(None)),
        CursorUpdate::Replace {
            cursor: Some(cursor),
        } => {
            let state = neutral::provider_state(&cursor)
                .map_err(|error| StoreError::Contract(error.to_string()))?;
            Ok(Some(Some(state)))
        }
    }
}

#[async_trait]
impl TranscriptStore for SqliteTranscriptStore {
    async fn load(&self, session: &SessionRef) -> Result<TranscriptSnapshot, StoreError> {
        let transcript = self
            .store
            .load_agent_session(session.as_str())
            .await
            .map_err(|error| StoreError::Load(error.to_string()))?;
        let cursor = self
            .store
            .load_provider_state(session.as_str())
            .await
            .map_err(|error| StoreError::Load(error.to_string()))?
            .as_ref()
            .map(neutral::model_cursor);
        Ok(TranscriptSnapshot {
            transcript,
            cursor,
            range: None,
        })
    }

    async fn append(
        &self,
        session: &SessionRef,
        message: &AgentMessage,
        cursor: CursorUpdate,
    ) -> Result<MessageReceipt, StoreError> {
        let count = self
            .store
            .count_messages_by_session(session.as_str())
            .await
            .map_err(|error| StoreError::Append(error.to_string()))?;
        let ordinal = count.saturating_add(1);
        let provider = provider_update(cursor)?;
        let id = match provider.as_ref() {
            Some(provider) => self
                .store
                .append_agent_message_and_provider_state(
                    session.as_str(),
                    message,
                    ordinal,
                    Some(provider.as_ref()),
                )
                .await
                .map_err(|error| StoreError::Append(error.to_string()))?,
            None => self
                .store
                .append_agent_message(session.as_str(), message, ordinal)
                .await
                .map_err(|error| StoreError::Append(error.to_string()))?,
        };
        let coordinate = u64::try_from(id)
            .map_err(|_| StoreError::Contract(format!("negative message row id {id}")))?;
        let range = MessageRange {
            start: coordinate,
            end: coordinate,
        };
        if let Some(receipts) = &self.receipts {
            receipts.record_append(message, range);
        }
        Ok(MessageReceipt { range })
    }

    async fn replace(
        &self,
        session: &SessionRef,
        transcript: Option<&[AgentMessage]>,
        cursor: CursorUpdate,
    ) -> Result<ReplaceReceipt, StoreError> {
        let provider = provider_update(cursor)?;
        let (first, last) = self
            .store
            .replace_agent_session_state(
                session.as_str(),
                transcript,
                provider.as_ref().map(Option::as_ref),
            )
            .await
            .map_err(|error| StoreError::Replace(error.to_string()))?;
        let range =
            (first > 0 && last > 0).then(|| (u64::try_from(first).ok(), u64::try_from(last).ok()));
        let range = match range {
            Some((Some(start), Some(end))) => Some(MessageRange { start, end }),
            Some(_) => {
                return Err(StoreError::Contract(
                    "transcript replacement produced a negative range".into(),
                ));
            }
            None => None,
        };
        if let Some(receipts) = &self.receipts {
            receipts.record_replace(range);
        }
        Ok(ReplaceReceipt { range })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use tact_agent_api::{AgentContentBlock, AgentImageSource, AgentRole, MessageOrigin};

    use super::*;
    use crate::store::session_store::open_sqlite_session_store;

    #[tokio::test]
    async fn explicit_origins_round_trip_through_sqlite() {
        let dir = tempfile::tempdir().unwrap();
        let store = open_sqlite_session_store(&dir.path().join("sessions.db"))
            .await
            .unwrap();
        store
            .ensure_session_row("session", "/workspace", "")
            .await
            .unwrap();
        let adapter = SqliteTranscriptStore::new(store.clone());
        let session = SessionRef::new("session");
        let message = AgentMessage::text(
            AgentRole::User,
            MessageOrigin::InjectedContext {
                source: "plugin".into(),
            },
            "context",
        );
        let receipt = adapter
            .append(&session, &message, CursorUpdate::Unchanged)
            .await
            .unwrap();
        assert_eq!(receipt.range.start, receipt.range.end);
        let loaded = adapter.load(&session).await.unwrap();
        assert_eq!(loaded.transcript, vec![message]);
    }

    #[tokio::test]
    async fn mixed_tool_result_images_keep_their_companion_order_through_sqlite() {
        let dir = tempfile::tempdir().unwrap();
        let store = open_sqlite_session_store(&dir.path().join("sessions.db"))
            .await
            .unwrap();
        store
            .ensure_session_row("session", "/workspace", "")
            .await
            .unwrap();
        let adapter = SqliteTranscriptStore::new(store);
        let session = SessionRef::new("session");
        let message = AgentMessage::new(
            AgentRole::User,
            MessageOrigin::ToolResult,
            vec![
                AgentContentBlock::ToolResult {
                    tool_use_id: "call-image".into(),
                    content: "preview".into(),
                },
                AgentContentBlock::Image {
                    source: AgentImageSource {
                        type_: "base64".into(),
                        media_type: "image/png".into(),
                        data: "AAAA".into(),
                    },
                },
                AgentContentBlock::ToolResult {
                    tool_use_id: "call-text".into(),
                    content: "lookup result".into(),
                },
            ],
        );

        adapter
            .append(&session, &message, CursorUpdate::Unchanged)
            .await
            .unwrap();

        let loaded = adapter.load(&session).await.unwrap();
        assert_eq!(loaded.transcript, vec![message]);
    }

    #[tokio::test]
    async fn assistant_append_and_cursor_replacement_are_one_store_call() {
        let dir = tempfile::tempdir().unwrap();
        let store = open_sqlite_session_store(&dir.path().join("sessions.db"))
            .await
            .unwrap();
        store
            .ensure_session_row("session", "/workspace", "")
            .await
            .unwrap();
        let state =
            ProviderConversationState::OpenAiResponses(tact_llm::ResponsesConversationState {
                version: 1,
                provider: "openai".into(),
                base_url: "https://example.invalid".into(),
                model: "test".into(),
                input_items: Vec::new(),
                compaction_id: None,
                is_compacted: false,
                logical_message_count: 0,
                logical_context_hash: String::new(),
            });
        let cursor = neutral::model_cursor(&state);
        let adapter = SqliteTranscriptStore::new(store.clone());
        let session = SessionRef::new("session");
        let assistant = AgentMessage::new(
            AgentRole::Assistant,
            MessageOrigin::Assistant,
            vec![AgentContentBlock::Text {
                text: "answer".into(),
            }],
        );
        adapter
            .append(
                &session,
                &assistant,
                CursorUpdate::Replace {
                    cursor: Some(cursor.clone()),
                },
            )
            .await
            .unwrap();
        let loaded = adapter.load(&session).await.unwrap();
        assert_eq!(loaded.cursor, Some(cursor));
        assert_eq!(loaded.transcript, vec![assistant]);
    }

    #[tokio::test]
    async fn no_durable_store_is_not_silently_assumed_by_the_sql_adapter() {
        let dir = tempfile::tempdir().unwrap();
        let store: DynSessionStore = Arc::new(
            crate::store::session_store::SqliteSessionStore::new(&dir.path().join("s.db"))
                .await
                .unwrap(),
        );
        let adapter = SqliteTranscriptStore::new(store);
        assert!(
            adapter
                .load(&SessionRef::new("missing"))
                .await
                .unwrap()
                .transcript
                .is_empty()
        );
    }
}
