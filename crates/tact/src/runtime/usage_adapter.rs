//! Durable kernel usage backed by the session store.

use async_trait::async_trait;
use tact_agent_api::{ModelCallKind, UsageError, UsageRecord, UsageSink};

use super::host_neutral_token_usage;
use crate::store::DynSessionStore;

pub(crate) struct SqliteUsageSink {
    store: DynSessionStore,
    session_id: String,
}

/// Sessionless usage policy: accept records, persist nothing, flush immediately.
#[derive(Default)]
pub(crate) struct NoopUsageSink {
    records: std::sync::atomic::AtomicU64,
}

impl NoopUsageSink {
    #[cfg(test)]
    pub(crate) fn records(&self) -> u64 {
        self.records.load(std::sync::atomic::Ordering::Relaxed)
    }
}

#[async_trait]
impl UsageSink for NoopUsageSink {
    async fn record(&self, _record: UsageRecord) -> Result<(), UsageError> {
        self.records
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Ok(())
    }

    async fn flush(&self) -> Result<(), UsageError> {
        Ok(())
    }
}

impl SqliteUsageSink {
    pub(crate) fn new(store: DynSessionStore, session_id: impl Into<String>) -> Self {
        Self {
            store,
            session_id: session_id.into(),
        }
    }
}

fn call_type(kind: ModelCallKind) -> &'static str {
    match kind {
        ModelCallKind::Stream => "stream",
        ModelCallKind::Complete => "complete",
        ModelCallKind::LocalCompact => "compact",
        ModelCallKind::NativeCompact => "responses_compact",
    }
}

#[async_trait]
impl UsageSink for SqliteUsageSink {
    async fn record(&self, record: UsageRecord) -> Result<(), UsageError> {
        let usage = record.usage.as_ref().map(host_neutral_token_usage);
        let first = record
            .message_range
            .map(|range| i64::try_from(range.start))
            .transpose()
            .map_err(|_| UsageError::Record("message range start exceeds i64".into()))?
            .unwrap_or(0);
        let last = record
            .message_range
            .map(|range| i64::try_from(range.end))
            .transpose()
            .map_err(|_| UsageError::Record("message range end exceeds i64".into()))?
            .unwrap_or(0);
        self.store
            .record_token_usage_with_call_id(
                &self.session_id,
                Some(record.call_id.as_str()),
                call_type(record.kind),
                usage.as_ref(),
                first,
                last,
                Some(record.request_body.as_deref().unwrap_or_default()),
            )
            .await
            .map_err(|error| UsageError::Record(format!("{error:#}")))
    }

    async fn flush(&self) -> Result<(), UsageError> {
        // Every record is awaited through to SQLite before `record` returns.
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use sqlx::Row;
    use tact_agent_api::{MessageRange, TokenUsage};

    use super::*;
    use crate::store::session_store::{SessionStore, open_sqlite_session_store};

    #[tokio::test]
    async fn noop_usage_records_are_explicitly_dropped() {
        let sink = NoopUsageSink::default();
        sink.record(UsageRecord {
            call_id: "call-1".into(),
            run: None,
            kind: ModelCallKind::Stream,
            usage: None,
            request_body: None,
            message_range: None,
        })
        .await
        .unwrap();
        assert_eq!(sink.records(), 1);
        sink.flush().await.unwrap();
    }

    #[tokio::test]
    async fn idempotent_records_leave_one_current_row() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sessions.db");
        let store = open_sqlite_session_store(&path).await.unwrap();
        store
            .ensure_session_row("session", "/workspace", "")
            .await
            .unwrap();
        let sink = SqliteUsageSink::new(store, "session");
        let first = UsageRecord {
            call_id: "call-1".into(),
            run: None,
            kind: ModelCallKind::Stream,
            usage: Some(TokenUsage {
                input_tokens: 1,
                output_tokens: 2,
                total_tokens: 3,
                ..TokenUsage::default()
            }),
            request_body: Some(b"one".to_vec()),
            message_range: Some(MessageRange { start: 1, end: 1 }),
        };
        sink.record(first).await.unwrap();
        sink.record(UsageRecord {
            call_id: "call-1".into(),
            run: None,
            kind: ModelCallKind::Stream,
            usage: Some(TokenUsage {
                input_tokens: 4,
                output_tokens: 5,
                total_tokens: 9,
                ..TokenUsage::default()
            }),
            request_body: Some(b"two".to_vec()),
            message_range: Some(MessageRange { start: 1, end: 2 }),
        })
        .await
        .unwrap();
        sink.flush().await.unwrap();

        let pool = crate::store::sqlite::open_pool(&path).await.unwrap();
        let row = sqlx::query(
            "SELECT COUNT(*) AS count, MAX(total_tokens) AS total, MAX(first_message_id) AS first, \
             MAX(last_message_id) AS last, MAX(request_body) AS body FROM token_usages \
             WHERE session_id = 'session' AND call_id = 'call-1'",
        )
        .fetch_one(&*pool)
        .await
        .unwrap();
        assert_eq!(row.try_get::<i64, _>("count").unwrap(), 1);
        assert_eq!(row.try_get::<i64, _>("total").unwrap(), 9);
        assert_eq!(row.try_get::<i64, _>("first").unwrap(), 1);
        assert_eq!(row.try_get::<i64, _>("last").unwrap(), 2);
        assert_eq!(row.try_get::<Vec<u8>, _>("body").unwrap(), b"two");
    }

    #[tokio::test]
    async fn sqlite_usage_is_visible_before_flush_returns() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sessions.db");
        let store = open_sqlite_session_store(&path).await.unwrap();
        store
            .ensure_session_row("session", "/workspace", "")
            .await
            .unwrap();
        let sink = SqliteUsageSink::new(store, "session");

        sink.record(UsageRecord {
            call_id: "call-visible".into(),
            run: None,
            kind: ModelCallKind::Complete,
            usage: Some(TokenUsage {
                input_tokens: 2,
                output_tokens: 3,
                total_tokens: 5,
                ..TokenUsage::default()
            }),
            request_body: Some(b"wire-body".to_vec()),
            message_range: Some(MessageRange { start: 7, end: 8 }),
        })
        .await
        .unwrap();

        let pool = crate::store::sqlite::open_pool(&path).await.unwrap();
        let row = sqlx::query(
            "SELECT call_type, total_tokens, first_message_id, last_message_id, request_body \
             FROM token_usages WHERE session_id = 'session' AND call_id = 'call-visible'",
        )
        .fetch_one(&*pool)
        .await
        .unwrap();
        assert_eq!(row.try_get::<String, _>("call_type").unwrap(), "complete");
        assert_eq!(row.try_get::<i64, _>("total_tokens").unwrap(), 5);
        assert_eq!(row.try_get::<i64, _>("first_message_id").unwrap(), 7);
        assert_eq!(row.try_get::<i64, _>("last_message_id").unwrap(), 8);
        assert_eq!(
            row.try_get::<Vec<u8>, _>("request_body").unwrap(),
            b"wire-body"
        );

        sink.flush().await.unwrap();
    }

    #[tokio::test]
    async fn sqlite_usage_record_failure_is_reported() {
        let dir = tempfile::tempdir().unwrap();
        let store =
            crate::store::session_store::SqliteSessionStore::new(&dir.path().join("sessions.db"))
                .await
                .unwrap();
        store
            .ensure_session_row("session", "/workspace", "")
            .await
            .unwrap();
        store.inject_token_usage_insert_failure().await.unwrap();
        let sink = SqliteUsageSink::new(std::sync::Arc::new(store), "session");

        let error = sink
            .record(UsageRecord {
                call_id: "call-fails".into(),
                run: None,
                kind: ModelCallKind::NativeCompact,
                usage: None,
                request_body: Some(b"{}".to_vec()),
                message_range: None,
            })
            .await
            .expect_err("a SQLite write failure must not be dropped");

        assert!(
            matches!(error, UsageError::Record(ref message) if message.contains("injected token usage failure")),
            "{error:?}"
        );
    }
}
