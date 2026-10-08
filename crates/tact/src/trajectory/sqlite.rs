use std::path::Path;

use anyhow::{Context, Result};
use serde_json::Value;
use sqlx::SqlitePool;
use tact_protocol::{RunId, StepId, TrajectoryId};

use super::{ActorId, Sensitivity, TrajectoryEvent, TrajectoryEventType};

#[derive(Clone)]
pub struct SqliteTrajectoryRecorder {
    pool: SqlitePool,
}

impl SqliteTrajectoryRecorder {
    pub async fn open(path: &Path) -> Result<Self> {
        let pool = crate::store::sqlite::open_pool(path).await?;
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS trajectory_events (
                trajectory_id TEXT NOT NULL,
                run_id TEXT NOT NULL,
                sequence INTEGER NOT NULL,
                timestamp INTEGER NOT NULL,
                actor TEXT NOT NULL,
                event_type TEXT NOT NULL,
                parent_step_id TEXT,
                payload TEXT NOT NULL,
                sensitivity TEXT NOT NULL,
                PRIMARY KEY (trajectory_id, sequence)
            )",
        )
        .execute(&*pool)
        .await
        .context("create trajectory_events table")?;
        Ok(Self {
            pool: (*pool).clone(),
        })
    }

    pub async fn append(
        &self,
        trajectory_id: TrajectoryId,
        run_id: RunId,
        actor: ActorId,
        event_type: TrajectoryEventType,
        parent_step_id: Option<StepId>,
        payload: Value,
        sensitivity: Sensitivity,
    ) -> Result<TrajectoryEvent> {
        let mut transaction = self.pool.begin().await.context("begin trajectory append")?;
        let sequence: i64 = sqlx::query_scalar(
            "SELECT COALESCE(MAX(sequence), -1) + 1 FROM trajectory_events WHERE trajectory_id = ?",
        )
        .bind(trajectory_id.as_str())
        .fetch_one(&mut *transaction)
        .await
        .context("allocate trajectory sequence")?;
        let timestamp = crate::store::sqlite::now_millis();
        sqlx::query(
            "INSERT INTO trajectory_events
             (trajectory_id, run_id, sequence, timestamp, actor, event_type, parent_step_id, payload, sensitivity)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(trajectory_id.as_str())
        .bind(run_id.as_str())
        .bind(sequence)
        .bind(timestamp)
        .bind(&actor)
        .bind(serde_json::to_string(&event_type)?)
        .bind(parent_step_id.as_ref().map(StepId::as_str))
        .bind(serde_json::to_string(&payload)?)
        .bind(serde_json::to_string(&sensitivity)?)
        .execute(&mut *transaction)
        .await
        .context("insert trajectory event")?;
        transaction
            .commit()
            .await
            .context("commit trajectory append")?;
        Ok(TrajectoryEvent {
            trajectory_id,
            run_id,
            sequence: sequence as u64,
            timestamp: chrono::DateTime::from_timestamp_millis(timestamp)
                .unwrap_or_else(chrono::Utc::now),
            actor,
            event_type,
            parent_step_id,
            payload,
            sensitivity,
        })
    }

    pub async fn query(
        &self,
        trajectory_id: &TrajectoryId,
        from_sequence: u64,
    ) -> Result<Vec<TrajectoryEvent>> {
        let rows = sqlx::query_as::<_, Row>(
            "SELECT trajectory_id, run_id, sequence, timestamp, actor, event_type,
                    parent_step_id, payload, sensitivity
             FROM trajectory_events WHERE trajectory_id = ? AND sequence >= ?
             ORDER BY sequence ASC",
        )
        .bind(trajectory_id.as_str())
        .bind(from_sequence as i64)
        .fetch_all(&self.pool)
        .await
        .context("query trajectory events")?;
        rows.into_iter().map(Row::into_event).collect()
    }
}

#[derive(sqlx::FromRow)]
struct Row {
    trajectory_id: String,
    run_id: String,
    sequence: i64,
    timestamp: i64,
    actor: String,
    event_type: String,
    parent_step_id: Option<String>,
    payload: String,
    sensitivity: String,
}

impl Row {
    fn into_event(self) -> Result<TrajectoryEvent> {
        Ok(TrajectoryEvent {
            trajectory_id: TrajectoryId::new(self.trajectory_id).map_err(anyhow::Error::msg)?,
            run_id: RunId::new(self.run_id).map_err(anyhow::Error::msg)?,
            sequence: self.sequence as u64,
            timestamp: chrono::DateTime::from_timestamp_millis(self.timestamp)
                .unwrap_or_else(chrono::Utc::now),
            actor: self.actor,
            event_type: serde_json::from_str(&self.event_type)?,
            parent_step_id: self
                .parent_step_id
                .map(StepId::new)
                .transpose()
                .map_err(anyhow::Error::msg)?,
            payload: serde_json::from_str(&self.payload)?,
            sensitivity: serde_json::from_str(&self.sensitivity)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn appends_and_replays_ordered_events() {
        let directory = tempfile::tempdir().expect("temp directory");
        let recorder = SqliteTrajectoryRecorder::open(&directory.path().join("tact.db"))
            .await
            .expect("open recorder");
        let trajectory = TrajectoryId::from("trajectory-test");
        let run = RunId::from("run-test");
        recorder
            .append(
                trajectory.clone(),
                run.clone(),
                "agent".into(),
                TrajectoryEventType::Message,
                None,
                serde_json::json!({"text": "hello"}),
                Sensitivity::Internal,
            )
            .await
            .expect("append event");
        let events = recorder.query(&trajectory, 0).await.expect("query events");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].sequence, 0);
        assert_eq!(events[0].run_id, run);
    }
}
