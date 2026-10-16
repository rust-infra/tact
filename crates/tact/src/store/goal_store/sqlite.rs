use std::{path::Path, time::Duration};

use anyhow::{Context, Result};
use async_trait::async_trait;
use sqlx::{Row, SqliteConnection};

use crate::{
    goal::{Goal, GoalEvent, GoalRun, GoalStatus, GoalUpdate},
    store::sqlite::{PoolRef, from_millis, now_millis, open_pool},
};

use super::{GoalStore, GoalStoreError, LeaseToken};

/// SQLite-backed Goal store. The database file is the workspace boundary.
pub struct SqliteGoalStore {
    pool: PoolRef,
}

impl SqliteGoalStore {
    pub async fn new(path: &Path) -> Result<Self> {
        let pool = open_pool(path).await?;
        for statement in [
            r#"CREATE TABLE IF NOT EXISTS goals (
                id TEXT PRIMARY KEY,
                data TEXT NOT NULL,
                status TEXT NOT NULL,
                version INTEGER NOT NULL,
                updated_at INTEGER NOT NULL
            )"#,
            r#"CREATE TABLE IF NOT EXISTS goal_events (
                seq INTEGER PRIMARY KEY AUTOINCREMENT,
                id TEXT NOT NULL UNIQUE,
                goal_id TEXT NOT NULL,
                data TEXT NOT NULL,
                occurred_at INTEGER NOT NULL
            )"#,
            r#"CREATE TABLE IF NOT EXISTS goal_runs (
                id TEXT PRIMARY KEY,
                goal_id TEXT NOT NULL,
                data TEXT NOT NULL,
                state TEXT NOT NULL CHECK (state IN ('running', 'completed', 'interrupted')),
                started_at INTEGER NOT NULL,
                finished_at INTEGER,
                stop_reason TEXT
            )"#,
            r#"CREATE TABLE IF NOT EXISTS goal_leases (
                singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
                owner TEXT NOT NULL,
                epoch INTEGER NOT NULL,
                expires_at INTEGER NOT NULL
            )"#,
        ] {
            sqlx::query(statement)
                .execute(&*pool)
                .await
                .context("failed to initialize goal store schema")?;
        }
        for statement in [
            "CREATE INDEX IF NOT EXISTS idx_goals_status ON goals(status)",
            "CREATE INDEX IF NOT EXISTS idx_goals_updated_at ON goals(updated_at)",
            "CREATE INDEX IF NOT EXISTS idx_goal_events_goal_seq ON goal_events(goal_id, seq)",
            "CREATE INDEX IF NOT EXISTS idx_goal_runs_state ON goal_runs(state)",
            "CREATE INDEX IF NOT EXISTS idx_goal_runs_goal_id ON goal_runs(goal_id)",
            "CREATE INDEX IF NOT EXISTS idx_goal_runs_started_at ON goal_runs(started_at)",
        ] {
            sqlx::query(statement)
                .execute(&*pool)
                .await
                .context("failed to initialize goal store indexes")?;
        }
        Ok(Self { pool })
    }

    async fn update_in_tx(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
        goal_id: &str,
        expected_version: u64,
        update: GoalUpdate,
        event: Option<GoalEvent>,
        lease: Option<&LeaseToken>,
    ) -> Result<Goal> {
        if let Some(token) = lease {
            validate_lease(tx, token).await?;
        } else if update.status.is_some() {
            // A status write without the run's fencing lease must not land
            // while a run is active: from `start_run` to
            // `finish_run_and_transition` the run owns the Goal's status, and a
            // lease-less writer (a second process pausing mid-activation) would
            // commit a paused Goal behind a still-`running` run row — the
            // "paused but run active" observation the invariants forbid. The
            // run's own finish transaction passes here because it marks the run
            // completed before applying transitions (2026-10-16).
            let active = sqlx::query(
                "SELECT 1 FROM goal_runs WHERE goal_id = ? AND state = 'running' LIMIT 1",
            )
            .bind(goal_id)
            .fetch_optional(&mut **tx)
            .await?;
            if active.is_some() {
                return Err(anyhow::Error::new(GoalStoreError::ActiveRunExists));
            }
        }
        let row = sqlx::query("SELECT data, version FROM goals WHERE id = ?")
            .bind(goal_id)
            .fetch_optional(&mut **tx)
            .await?
            .ok_or_else(|| not_found("goal", goal_id))?;
        let actual: i64 = row.try_get("version")?;
        if actual != expected_version as i64 {
            return Err(anyhow::Error::new(GoalStoreError::VersionConflict {
                goal_id: goal_id.to_string(),
                expected: expected_version,
                actual: actual as u64,
            }));
        }
        let mut goal = decode_goal(row.try_get("data")?)?;
        goal.update(update).map_err(anyhow::Error::new)?;
        let data = serde_json::to_string(&goal)?;
        let changed = sqlx::query(
            "UPDATE goals SET data = ?, status = ?, version = ?, updated_at = ? WHERE id = ? AND version = ?",
        )
        .bind(&data)
        .bind(status_string(goal.status))
        .bind(goal.version as i64)
        .bind(goal.updated_at.timestamp_millis())
        .bind(goal_id)
        .bind(expected_version as i64)
        .execute(&mut **tx)
        .await?
        .rows_affected();
        if changed != 1 {
            return Err(anyhow::Error::new(GoalStoreError::VersionConflict {
                goal_id: goal_id.to_string(),
                expected: expected_version,
                actual: expected_version,
            }));
        }
        if let Some(event) = event {
            if event.goal_id != goal_id {
                return Err(anyhow::anyhow!("event goal id does not match updated goal"));
            }
            insert_event(tx, &event).await?;
        }
        Ok(goal)
    }

    async fn lease_tx(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
        owner: &str,
        ttl: Duration,
    ) -> Result<LeaseToken> {
        let now = now_millis();
        let expires = now.saturating_add(ttl.as_millis().min(i64::MAX as u128) as i64);
        let row =
            sqlx::query("SELECT owner, epoch, expires_at FROM goal_leases WHERE singleton = 1")
                .fetch_optional(&mut **tx)
                .await?;
        match row {
            None => {
                sqlx::query("INSERT INTO goal_leases (singleton, owner, epoch, expires_at) VALUES (1, ?, 1, ?)")
                    .bind(owner)
                    .bind(expires)
                    .execute(&mut **tx)
                    .await?;
                Ok(LeaseToken {
                    owner: owner.to_string(),
                    epoch: 1,
                })
            }
            Some(row) => {
                let current_owner: String = row.try_get("owner")?;
                let current_epoch: i64 = row.try_get("epoch")?;
                let current_expires: i64 = row.try_get("expires_at")?;
                if current_expires > now {
                    if current_owner != owner {
                        return Err(anyhow::Error::new(GoalStoreError::LeaseHeld {
                            owner: current_owner,
                            epoch: current_epoch,
                        }));
                    }
                    sqlx::query("UPDATE goal_leases SET expires_at = ? WHERE singleton = 1 AND owner = ? AND epoch = ?")
                        .bind(expires).bind(owner).bind(current_epoch).execute(&mut **tx).await?;
                    Ok(LeaseToken {
                        owner: owner.to_string(),
                        epoch: current_epoch,
                    })
                } else {
                    interrupt_running(tx, now, "lease expired").await?;
                    let epoch = current_epoch.saturating_add(1);
                    sqlx::query("UPDATE goal_leases SET owner = ?, epoch = ?, expires_at = ? WHERE singleton = 1 AND epoch = ? AND expires_at <= ?")
                        .bind(owner).bind(epoch).bind(expires).bind(current_epoch).bind(now)
                        .execute(&mut **tx).await?;
                    Ok(LeaseToken {
                        owner: owner.to_string(),
                        epoch,
                    })
                }
            }
        }
    }

    /// Marks a stored `running` run as completed inside an open transaction.
    ///
    /// The caller validates the lease once at the top of its own transaction, so
    /// this helper never re-reads the lease row.
    async fn finish_run_in_tx(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
        run: &GoalRun,
    ) -> Result<GoalRun> {
        if run.finished_at.is_none() || run.outcome.is_none() {
            return Err(anyhow::Error::new(GoalStoreError::RunNotFinished {
                run_id: run.id.clone(),
            }));
        }
        let stored = sqlx::query(
            "SELECT id, goal_id, data FROM goal_runs WHERE id = ? AND state = 'running'",
        )
        .bind(&run.id)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or_else(|| not_found("running goal run", &run.id))?;
        let stored_id: String = stored.try_get("id")?;
        let stored_goal_id: String = stored.try_get("goal_id")?;
        let stored_run: GoalRun = serde_json::from_str(stored.try_get("data")?)?;
        if stored_id != run.id
            || stored_run.id != run.id
            || stored_goal_id != run.goal_id
            || stored_run.goal_id != run.goal_id
            || stored_run.started_at != run.started_at
        {
            return Err(anyhow::Error::new(GoalStoreError::RunIdentityMismatch {
                run_id: run.id.clone(),
                stored_id,
                supplied_id: run.id.clone(),
                stored_goal_id,
                supplied_goal_id: run.goal_id.clone(),
                stored_started_at: stored_run.started_at,
                supplied_started_at: run.started_at,
            }));
        }
        let data = serde_json::to_string(run)?;
        let changed = sqlx::query("UPDATE goal_runs SET data = ?, state = 'completed', finished_at = ?, stop_reason = ? WHERE id = ? AND goal_id = ? AND state = 'running'")
            .bind(data).bind(run.finished_at.map(|v| v.timestamp_millis())).bind(&run.stop_reason).bind(&run.id).bind(&run.goal_id)
            .execute(&mut **tx).await?.rows_affected();
        if changed != 1 {
            return Err(not_found("running goal run", &run.id));
        }
        Ok(run.clone())
    }

    /// Loads the current Goal aggregate (with its version) inside an open
    /// transaction, so a chained transition CASes against the right version.
    async fn load_goal_in_tx(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
        goal_id: &str,
    ) -> Result<Goal> {
        let row = sqlx::query("SELECT data FROM goals WHERE id = ?")
            .bind(goal_id)
            .fetch_optional(&mut **tx)
            .await?
            .ok_or_else(|| not_found("goal", goal_id))?;
        decode_goal(row.try_get("data")?)
    }
}

#[async_trait]
impl GoalStore for SqliteGoalStore {
    async fn create(&self, goal: Goal) -> Result<Goal> {
        let data = serde_json::to_string(&goal)?;
        sqlx::query(
            "INSERT INTO goals (id, data, status, version, updated_at) VALUES (?, ?, ?, ?, ?)",
        )
        .bind(&goal.id)
        .bind(data)
        .bind(status_string(goal.status))
        .bind(goal.version as i64)
        .bind(goal.updated_at.timestamp_millis())
        .execute(&*self.pool)
        .await
        .context("failed to create goal")?;
        Ok(goal)
    }

    async fn get(&self, goal_id: &str) -> Result<Goal> {
        let row = sqlx::query("SELECT data FROM goals WHERE id = ?")
            .bind(goal_id)
            .fetch_optional(&*self.pool)
            .await?;
        row.map(|r| decode_goal(r.try_get("data").unwrap_or_default()))
            .transpose()?
            .ok_or_else(|| not_found("goal", goal_id))
    }

    async fn list(&self) -> Result<Vec<Goal>> {
        let rows = sqlx::query("SELECT data FROM goals ORDER BY json_extract(data, '$.title'), id")
            .fetch_all(&*self.pool)
            .await?;
        rows.into_iter()
            .map(|r| decode_goal(r.try_get("data")?))
            .collect()
    }

    async fn update(
        &self,
        goal_id: &str,
        expected_version: u64,
        update: GoalUpdate,
        event: Option<GoalEvent>,
        lease: Option<&LeaseToken>,
    ) -> Result<Goal> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let goal = self
            .update_in_tx(&mut tx, goal_id, expected_version, update, event, lease)
            .await?;
        tx.commit().await?;
        Ok(goal)
    }

    async fn append_event(&self, event: GoalEvent) -> Result<GoalEvent> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        sqlx::query("SELECT 1 FROM goals WHERE id = ?")
            .bind(&event.goal_id)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or_else(|| not_found("goal", &event.goal_id))?;
        insert_event(&mut tx, &event).await?;
        tx.commit().await?;
        Ok(event)
    }

    async fn list_events(&self, goal_id: &str) -> Result<Vec<GoalEvent>> {
        let rows = sqlx::query("SELECT data FROM goal_events WHERE goal_id = ? ORDER BY seq")
            .bind(goal_id)
            .fetch_all(&*self.pool)
            .await?;
        rows.into_iter()
            .map(|r| Ok(serde_json::from_str(r.try_get("data")?)?))
            .collect()
    }

    async fn count_events(&self, goal_id: &str) -> Result<u64> {
        let row = sqlx::query("SELECT COUNT(*) AS n FROM goal_events WHERE goal_id = ?")
            .bind(goal_id)
            .fetch_one(&*self.pool)
            .await?;
        Ok(row.try_get::<i64, _>("n")? as u64)
    }

    async fn list_recent_events(&self, goal_id: &str, limit: usize) -> Result<Vec<GoalEvent>> {
        let rows =
            sqlx::query("SELECT data FROM goal_events WHERE goal_id = ? ORDER BY seq DESC LIMIT ?")
                .bind(goal_id)
                .bind(limit as i64)
                .fetch_all(&*self.pool)
                .await?;
        let mut events = rows
            .into_iter()
            .map(|r| Ok(serde_json::from_str(r.try_get("data")?)?))
            .collect::<Result<Vec<_>>>()?;
        events.reverse();
        Ok(events)
    }

    async fn latest_event_of_kind(&self, goal_id: &str, kind: &str) -> Result<Option<GoalEvent>> {
        // The `kind` / `status` facts live in the event JSON; `ORDER BY seq
        // DESC LIMIT 1` scans the goal's rows newest-first and stops at the
        // first match, so the cost is the distance to that event, not the
        // goal's whole history.
        let row = sqlx::query(
            "SELECT data FROM goal_events \
             WHERE goal_id = ? AND json_extract(data, '$.kind') = ? \
             ORDER BY seq DESC LIMIT 1",
        )
        .bind(goal_id)
        .bind(kind)
        .fetch_optional(&*self.pool)
        .await?;
        row.map(|r| Ok(serde_json::from_str(r.try_get("data")?)?))
            .transpose()
    }

    async fn latest_event_with_status(
        &self,
        goal_id: &str,
        status: GoalStatus,
    ) -> Result<Option<GoalEvent>> {
        let row = sqlx::query(
            "SELECT data FROM goal_events \
             WHERE goal_id = ? AND json_extract(data, '$.status') = ? \
             ORDER BY seq DESC LIMIT 1",
        )
        .bind(goal_id)
        .bind(status_string(status))
        .fetch_optional(&*self.pool)
        .await?;
        row.map(|r| Ok(serde_json::from_str(r.try_get("data")?)?))
            .transpose()
    }

    async fn has_event_of_kind_since(
        &self,
        goal_id: &str,
        kind: &str,
        since: chrono::DateTime<chrono::Utc>,
    ) -> Result<bool> {
        let row = sqlx::query(
            "SELECT 1 FROM goal_events \
             WHERE goal_id = ? AND json_extract(data, '$.kind') = ? AND occurred_at >= ? \
             LIMIT 1",
        )
        .bind(goal_id)
        .bind(kind)
        .bind(since.timestamp_millis())
        .fetch_optional(&*self.pool)
        .await?;
        Ok(row.is_some())
    }

    async fn claim_lease(&self, owner: &str, ttl: Duration) -> Result<LeaseToken> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let token = self.lease_tx(&mut tx, owner, ttl).await?;
        tx.commit().await?;
        Ok(token)
    }

    async fn heartbeat_lease(&self, token: &LeaseToken, ttl: Duration) -> Result<LeaseToken> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        validate_lease(&mut tx, token).await?;
        let expires = now_millis().saturating_add(ttl.as_millis().min(i64::MAX as u128) as i64);
        let changed = sqlx::query(
            "UPDATE goal_leases SET expires_at = ? WHERE singleton = 1 AND owner = ? AND epoch = ?",
        )
        .bind(expires)
        .bind(&token.owner)
        .bind(token.epoch)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if changed != 1 {
            return Err(stale(token));
        }
        tx.commit().await?;
        Ok(token.clone())
    }

    async fn release_lease(&self, token: &LeaseToken) -> Result<()> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        validate_lease(&mut tx, token).await?;
        sqlx::query(
            "UPDATE goal_leases SET expires_at = ? WHERE singleton = 1 AND owner = ? AND epoch = ?",
        )
        .bind(now_millis())
        .bind(&token.owner)
        .bind(token.epoch)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    async fn start_run(
        &self,
        owner: &str,
        ttl: Duration,
        run: GoalRun,
    ) -> Result<(LeaseToken, GoalRun)> {
        if run.finished_at.is_some() || run.outcome.is_some() {
            return Err(anyhow::Error::new(GoalStoreError::RunNotFinished {
                run_id: run.id.clone(),
            }));
        }
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let token = self.lease_tx(&mut tx, owner, ttl).await?;
        let active_run = sqlx::query("SELECT 1 FROM goal_runs WHERE state = 'running' LIMIT 1")
            .fetch_optional(&mut *tx)
            .await?;
        if active_run.is_some() {
            return Err(anyhow::Error::new(GoalStoreError::ActiveRunExists));
        }
        let data = serde_json::to_string(&run)?;
        sqlx::query("INSERT INTO goal_runs (id, goal_id, data, state, started_at, finished_at, stop_reason) VALUES (?, ?, ?, 'running', ?, NULL, NULL)")
            .bind(&run.id).bind(&run.goal_id).bind(data).bind(run.started_at.timestamp_millis())
            .execute(&mut *tx).await.context("failed to start goal run")?;
        tx.commit().await?;
        Ok((token, run))
    }

    async fn finish_run(&self, token: &LeaseToken, run: GoalRun) -> Result<GoalRun> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        validate_lease(&mut tx, token).await?;
        let finished = self.finish_run_in_tx(&mut tx, &run).await?;
        tx.commit().await?;
        Ok(finished)
    }

    async fn finish_run_and_transition(
        &self,
        token: &LeaseToken,
        run: GoalRun,
        transitions: Vec<(GoalUpdate, GoalEvent)>,
        run_event: GoalEvent,
    ) -> Result<(GoalRun, Goal)> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        validate_lease(&mut tx, token).await?;
        let finished = self.finish_run_in_tx(&mut tx, &run).await?;
        // Each transition CASes against the version the previous one produced,
        // so a multi-step transition (`Verifying → Succeeded`) stays consistent
        // inside this one transaction.
        let mut goal = self.load_goal_in_tx(&mut tx, &run.goal_id).await?;
        for (update, event) in transitions {
            goal = self
                .update_in_tx(
                    &mut tx,
                    &run.goal_id,
                    goal.version,
                    update,
                    Some(event),
                    None,
                )
                .await?;
        }
        if run_event.goal_id != goal.id {
            return Err(anyhow::anyhow!(
                "run-finished event goal id does not match the finished goal"
            ));
        }
        insert_event(&mut tx, &run_event).await?;
        tx.commit().await?;
        Ok((finished, goal))
    }

    async fn list_running_runs(&self) -> Result<Vec<GoalRun>> {
        let rows = sqlx::query(
            "SELECT data FROM goal_runs WHERE state = 'running' ORDER BY started_at, id",
        )
        .fetch_all(&*self.pool)
        .await?;
        rows.into_iter()
            .map(|r| Ok(serde_json::from_str(r.try_get("data")?)?))
            .collect()
    }

    async fn recover_expired_active_runs(&self) -> Result<Vec<GoalRun>> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let lease = sqlx::query("SELECT expires_at FROM goal_leases WHERE singleton = 1")
            .fetch_optional(&mut *tx)
            .await?;
        let expired = lease
            .map(|r| r.try_get::<i64, _>("expires_at").unwrap_or(0) <= now_millis())
            .unwrap_or(false);
        let recovered = if expired {
            interrupt_running(&mut tx, now_millis(), "lease expired").await?
        } else {
            Vec::new()
        };
        tx.commit().await?;
        Ok(recovered)
    }
}

fn status_string(status: GoalStatus) -> &'static str {
    match status {
        GoalStatus::Draft => "draft",
        GoalStatus::Queued => "queued",
        GoalStatus::Running => "running",
        GoalStatus::WaitingUser => "waiting_user",
        GoalStatus::WaitingPermission => "waiting_permission",
        GoalStatus::Paused => "paused",
        GoalStatus::Verifying => "verifying",
        GoalStatus::Succeeded => "succeeded",
        GoalStatus::Failed => "failed",
        GoalStatus::Cancelled => "cancelled",
    }
}

fn decode_goal(data: String) -> Result<Goal> {
    Ok(serde_json::from_str(&data)?)
}
fn not_found(kind: &'static str, id: &str) -> anyhow::Error {
    anyhow::Error::new(GoalStoreError::NotFound {
        kind,
        id: id.to_string(),
    })
}
fn stale(token: &LeaseToken) -> anyhow::Error {
    anyhow::Error::new(GoalStoreError::StaleLease {
        owner: token.owner.clone(),
        epoch: token.epoch,
    })
}

async fn validate_lease(conn: &mut SqliteConnection, token: &LeaseToken) -> Result<()> {
    let row = sqlx::query("SELECT owner, epoch, expires_at FROM goal_leases WHERE singleton = 1")
        .fetch_optional(&mut *conn)
        .await?;
    let valid = row
        .map(|row| {
            let owner: String = row.try_get("owner").unwrap_or_default();
            let epoch: i64 = row.try_get("epoch").unwrap_or_default();
            let expires: i64 = row.try_get("expires_at").unwrap_or_default();
            owner == token.owner && epoch == token.epoch && expires > now_millis()
        })
        .unwrap_or(false);
    if valid { Ok(()) } else { Err(stale(token)) }
}

async fn insert_event(conn: &mut SqliteConnection, event: &GoalEvent) -> Result<()> {
    sqlx::query("INSERT INTO goal_events (id, goal_id, data, occurred_at) VALUES (?, ?, ?, ?)")
        .bind(&event.id)
        .bind(&event.goal_id)
        .bind(serde_json::to_string(event)?)
        .bind(event.occurred_at.timestamp_millis())
        .execute(&mut *conn)
        .await
        .context("failed to append goal event")?;
    Ok(())
}

async fn interrupt_running(
    conn: &mut SqliteConnection,
    finished_at: i64,
    reason: &str,
) -> Result<Vec<GoalRun>> {
    let rows = sqlx::query(
        "SELECT id, data FROM goal_runs WHERE state = 'running' ORDER BY started_at, id",
    )
    .fetch_all(&mut *conn)
    .await?;
    let mut recovered = Vec::with_capacity(rows.len());
    for row in rows {
        let id: String = row.try_get("id")?;
        let mut run: GoalRun = serde_json::from_str(row.try_get("data")?)?;
        run.finished_at = Some(from_millis(finished_at));
        run.stop_reason = Some(reason.to_string());
        sqlx::query("UPDATE goal_runs SET data = ?, state = 'interrupted', finished_at = ?, stop_reason = ? WHERE id = ? AND state = 'running'")
            .bind(serde_json::to_string(&run)?).bind(finished_at).bind(reason).bind(id)
            .execute(&mut *conn).await?;
        recovered.push(run);
    }
    Ok(recovered)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::goal::{GoalCreate, GoalCriterion, GoalEvent, GoalRunOutcome};
    use chrono::{Duration as ChronoDuration, Utc};

    fn temp_db(name: &str) -> std::path::PathBuf {
        crate::store::test_support::temp_db("tact-goalstore-test", name)
    }
    fn goal(title: &str) -> Goal {
        Goal::new(GoalCreate {
            title: title.to_string(),
            objective: format!("objective for {title}"),
            constraints: vec!["keep atomic".into()],
            criteria: vec![GoalCriterion::required_with_id(
                "criterion-1",
                "nested acceptance",
            )],
        })
        .unwrap()
    }

    #[tokio::test]
    async fn create_get_list_round_trip_and_deterministic_order() {
        let store = SqliteGoalStore::new(&temp_db("round_trip")).await.unwrap();
        let mut first = goal("first");
        first.criteria[0].record_evidence("evidence", first.criteria_version);
        let second = goal("second");
        store.create(second.clone()).await.unwrap();
        store.create(first.clone()).await.unwrap();
        assert_eq!(store.get(&first.id).await.unwrap(), first);
        let listed = store.list().await.unwrap();
        assert_eq!(
            listed.iter().map(|g| g.title.as_str()).collect::<Vec<_>>(),
            vec!["first", "second"]
        );
        assert_eq!(listed[0].criteria[0].evidence[0].value, "evidence");
    }

    /// `archived_at` travels in the Goal JSON, so filing a Goal away needs no
    /// column and no migration — and the `status` projection keeps reporting
    /// the outcome the Goal actually reached.
    #[tokio::test]
    async fn archived_at_round_trips_and_keeps_the_status() {
        let store = SqliteGoalStore::new(&temp_db("archived")).await.unwrap();
        let created = store.create(goal("filed")).await.unwrap();
        store
            .update(
                &created.id,
                created.version,
                GoalUpdate {
                    status: Some(GoalStatus::Cancelled),
                    ..GoalUpdate::default()
                },
                None,
                None,
            )
            .await
            .unwrap();

        let cancelled = store.get(&created.id).await.unwrap();
        let archived = store
            .update(
                &created.id,
                cancelled.version,
                GoalUpdate {
                    archived: Some(true),
                    ..GoalUpdate::default()
                },
                None,
                None,
            )
            .await
            .unwrap();
        assert!(archived.is_archived());

        let reloaded = store.get(&created.id).await.unwrap();
        assert_eq!(reloaded, archived);
        assert_eq!(reloaded.status, GoalStatus::Cancelled);
        assert!(store.list().await.unwrap()[0].is_archived());
    }

    /// The bounded reads the runner's cycles use: a count, a tail, and the two
    /// targeted lookups — none of them materialize a Goal's whole history, and
    /// every activation used to re-read and JSON-parse all of it (2026-10-16).
    #[tokio::test]
    async fn count_recent_and_latest_event_reads_are_bounded() {
        let store = SqliteGoalStore::new(&temp_db("bounded_events"))
            .await
            .unwrap();
        let created = store.create(goal("bounded")).await.unwrap();

        let checkpoint = GoalEvent {
            id: "event-checkpoint".into(),
            goal_id: created.id.clone(),
            run_id: None,
            kind: "checkpoint".into(),
            status: None,
            outcome: None,
            payload: Some("first checkpoint".into()),
            occurred_at: Utc::now(),
        };
        let paused = GoalEvent {
            id: "event-paused".into(),
            goal_id: created.id.clone(),
            run_id: None,
            kind: "status_changed".into(),
            status: Some(GoalStatus::Paused),
            outcome: None,
            payload: Some("budget exhausted".into()),
            occurred_at: Utc::now() + ChronoDuration::seconds(1),
        };
        let finished = GoalEvent {
            id: "event-finished".into(),
            goal_id: created.id.clone(),
            run_id: Some("run-1".into()),
            kind: "run_finished".into(),
            status: Some(GoalStatus::Paused),
            outcome: Some(GoalRunOutcome::BudgetExhausted),
            payload: Some("outcome=BudgetExhausted".into()),
            occurred_at: Utc::now() + ChronoDuration::seconds(2),
        };
        for event in [&checkpoint, &paused, &finished] {
            store.append_event(event.clone()).await.unwrap();
        }

        assert_eq!(store.count_events(&created.id).await.unwrap(), 3);

        // The tail is bounded and chronological, not the newest rows reversed.
        let recent = store.list_recent_events(&created.id, 2).await.unwrap();
        assert_eq!(
            recent
                .iter()
                .map(|event| event.id.as_str())
                .collect::<Vec<_>>(),
            vec!["event-paused", "event-finished"],
            "the tail keeps occurrence order"
        );

        // The lookups find the newest match, not the first.
        assert_eq!(
            store
                .latest_event_of_kind(&created.id, "checkpoint")
                .await
                .unwrap()
                .map(|event| event.id),
            Some("event-checkpoint".to_string())
        );
        assert_eq!(
            store
                .latest_event_of_kind(&created.id, "run_finished")
                .await
                .unwrap()
                .map(|event| event.id),
            Some("event-finished".to_string())
        );
        assert!(
            store
                .latest_event_of_kind(&created.id, "needs_user")
                .await
                .unwrap()
                .is_none(),
            "an absent kind answers None, not an error"
        );
        assert_eq!(
            store
                .latest_event_with_status(&created.id, GoalStatus::Paused)
                .await
                .unwrap()
                .map(|event| event.id),
            Some("event-finished".to_string()),
            "the newest Paused-status event, whichever kind it is"
        );
    }

    /// The completion-proposal predicate is asked exactly: an event at the
    /// boundary counts, an older one does not. A bounded read could miss a
    /// proposal from early in a long run, and a missed proposal is a Goal that
    /// can never succeed (2026-10-16).
    #[tokio::test]
    async fn has_event_of_kind_since_is_an_exact_predicate() {
        let store = SqliteGoalStore::new(&temp_db("proposal_since"))
            .await
            .unwrap();
        let created = store.create(goal("proposals")).await.unwrap();
        let boundary = Utc::now();
        let older = GoalEvent {
            id: "event-old".into(),
            goal_id: created.id.clone(),
            run_id: None,
            kind: "completion_proposed".into(),
            status: None,
            outcome: None,
            payload: Some("old proposal".into()),
            occurred_at: boundary - ChronoDuration::seconds(1),
        };
        let at_boundary = GoalEvent {
            id: "event-boundary".into(),
            goal_id: created.id.clone(),
            run_id: None,
            kind: "completion_proposed".into(),
            status: None,
            outcome: None,
            payload: Some("boundary proposal".into()),
            occurred_at: boundary,
        };
        store.append_event(older).await.unwrap();
        store.append_event(at_boundary).await.unwrap();

        assert!(
            store
                .has_event_of_kind_since(&created.id, "completion_proposed", boundary)
                .await
                .unwrap(),
            "an event at the boundary is inside the window"
        );
        assert!(
            !store
                .has_event_of_kind_since(
                    &created.id,
                    "completion_proposed",
                    boundary + ChronoDuration::seconds(1),
                )
                .await
                .unwrap(),
            "a window after every event finds none"
        );
        assert!(
            !store
                .has_event_of_kind_since(&created.id, "checkpoint", boundary)
                .await
                .unwrap(),
            "another kind is not a proposal"
        );
    }

    /// A lease-less status write is refused while a run is active: from
    /// `start_run` the run owns the Goal's status, and a second writer pausing
    /// mid-activation would commit a paused Goal behind a still-`running` run
    /// row — the "paused but run active" observation the invariants forbid
    /// (2026-10-16).
    #[tokio::test]
    async fn a_lease_less_status_write_is_refused_while_a_run_is_active() {
        let store = SqliteGoalStore::new(&temp_db("fence")).await.unwrap();
        let created = store.create(goal("fenced")).await.unwrap();
        let queued = store
            .update(
                &created.id,
                created.version,
                GoalUpdate {
                    status: Some(GoalStatus::Queued),
                    ..GoalUpdate::default()
                },
                None,
                None,
            )
            .await
            .unwrap();
        let running = store
            .update(
                &created.id,
                queued.version,
                GoalUpdate {
                    status: Some(GoalStatus::Running),
                    ..GoalUpdate::default()
                },
                None,
                None,
            )
            .await
            .unwrap();
        let (_token, _run) = store
            .start_run(
                "owner",
                Duration::from_secs(60),
                GoalRun::new(created.id.clone()),
            )
            .await
            .unwrap();

        // The hole: a second writer pauses the Goal while the run is active,
        // holding no lease to fence it.
        let error = store
            .update(
                &created.id,
                running.version,
                GoalUpdate {
                    status: Some(GoalStatus::Paused),
                    ..GoalUpdate::default()
                },
                None,
                None,
            )
            .await
            .unwrap_err();
        assert!(
            matches!(
                error.downcast_ref::<GoalStoreError>(),
                Some(GoalStoreError::ActiveRunExists)
            ),
            "the write is refused: {error:#}"
        );
        assert_eq!(
            store.get(&created.id).await.unwrap().status,
            GoalStatus::Running,
            "the Goal is untouched by the refused write"
        );

        // The same write with the run's own lease is legal — that is the
        // fenced path the runner uses.
        store
            .update(
                &created.id,
                running.version,
                GoalUpdate {
                    status: Some(GoalStatus::Paused),
                    ..GoalUpdate::default()
                },
                None,
                Some(&_token),
            )
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn cas_and_event_are_atomic_and_stale_update_is_unchanged() {
        let store = SqliteGoalStore::new(&temp_db("cas")).await.unwrap();
        let created = store.create(goal("before")).await.unwrap();
        let event = GoalEvent {
            id: "event-1".into(),
            goal_id: created.id.clone(),
            run_id: None,
            kind: "updated".into(),
            status: None,
            outcome: None,
            payload: Some("payload".into()),
            occurred_at: Utc::now(),
        };
        let updated = store
            .update(
                &created.id,
                1,
                GoalUpdate {
                    title: Some("after".into()),
                    ..Default::default()
                },
                Some(event.clone()),
                None,
            )
            .await
            .unwrap();
        assert_eq!(updated.version, 2);
        assert_eq!(store.list_events(&created.id).await.unwrap(), vec![event]);
        let err = store
            .update(
                &created.id,
                1,
                GoalUpdate {
                    title: Some("rejected".into()),
                    ..Default::default()
                },
                Some(GoalEvent {
                    id: "bad".into(),
                    goal_id: created.id.clone(),
                    run_id: None,
                    kind: "bad".into(),
                    status: None,
                    outcome: None,
                    payload: None,
                    occurred_at: Utc::now(),
                }),
                None,
            )
            .await
            .unwrap_err();
        assert!(err.downcast_ref::<GoalStoreError>().is_some());
        assert_eq!(store.get(&created.id).await.unwrap().title, "after");
        assert_eq!(store.list_events(&created.id).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn finish_run_and_transition_commits_run_goal_and_events_together() {
        let store = SqliteGoalStore::new(&temp_db("finish_atomic_ok"))
            .await
            .unwrap();
        let created = store.create(goal("atomic")).await.unwrap();
        let (token, run) = store
            .start_run(
                "owner",
                Duration::from_secs(60),
                GoalRun::new(created.id.clone()),
            )
            .await
            .unwrap();
        let mut finished = run.clone();
        finished.finish(GoalRunOutcome::ProgressMade);

        let transition_event = GoalEvent {
            id: "transition-event".into(),
            goal_id: created.id.clone(),
            run_id: None,
            kind: "status_changed".into(),
            status: Some(GoalStatus::Queued),
            outcome: None,
            payload: Some("Draft: runner activation".into()),
            occurred_at: Utc::now(),
        };
        let run_event = GoalEvent {
            id: "run-event".into(),
            goal_id: created.id.clone(),
            run_id: Some(finished.id.clone()),
            kind: "run_finished".into(),
            status: Some(GoalStatus::Queued),
            outcome: Some(GoalRunOutcome::ProgressMade),
            payload: Some("outcome=ProgressMade".into()),
            occurred_at: Utc::now(),
        };

        let (stored_run, updated) = store
            .finish_run_and_transition(
                &token,
                finished.clone(),
                vec![(
                    GoalUpdate {
                        status: Some(GoalStatus::Queued),
                        ..Default::default()
                    },
                    transition_event.clone(),
                )],
                run_event.clone(),
            )
            .await
            .unwrap();

        assert_eq!(stored_run.outcome, Some(GoalRunOutcome::ProgressMade));
        assert_eq!(updated.status, GoalStatus::Queued);
        assert!(store.list_running_runs().await.unwrap().is_empty());
        assert_eq!(
            store.list_events(&created.id).await.unwrap(),
            vec![transition_event, run_event],
            "the transition and the run's history entry are durable together"
        );
    }

    #[tokio::test]
    async fn finish_run_and_transition_rolls_back_the_run_when_the_event_is_rejected() {
        let store = SqliteGoalStore::new(&temp_db("finish_atomic_rollback"))
            .await
            .unwrap();
        let created = store.create(goal("atomic")).await.unwrap();
        let (token, run) = store
            .start_run(
                "owner",
                Duration::from_secs(60),
                GoalRun::new(created.id.clone()),
            )
            .await
            .unwrap();
        let mut finished = run.clone();
        finished.finish(GoalRunOutcome::ProgressMade);

        // A run event for a different Goal can never be recorded, and it must
        // take the run's own state change down with it: one transaction.
        let bad_event = GoalEvent {
            id: "bad-run-event".into(),
            goal_id: "some-other-goal".into(),
            run_id: Some(finished.id.clone()),
            kind: "run_finished".into(),
            status: Some(GoalStatus::Queued),
            outcome: Some(GoalRunOutcome::ProgressMade),
            payload: None,
            occurred_at: Utc::now(),
        };
        let error = store
            .finish_run_and_transition(&token, finished, Vec::new(), bad_event)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("does not match"), "{error:#}");

        assert_eq!(
            store.list_running_runs().await.unwrap().len(),
            1,
            "the run-finish rolled back with the rejected event"
        );
        assert!(
            store.list_events(&created.id).await.unwrap().is_empty(),
            "no history was written"
        );
        assert_eq!(
            store.get(&created.id).await.unwrap(),
            created,
            "the Goal itself is untouched"
        );
    }

    #[tokio::test]
    async fn schema_initialization_is_idempotent() {
        let db = temp_db("reopen");
        let first = SqliteGoalStore::new(&db).await.unwrap();
        first.create(goal("persisted")).await.unwrap();
        drop(first);
        let second = SqliteGoalStore::new(&db).await.unwrap();
        assert_eq!(second.list().await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn released_lease_preserves_fencing_epoch_for_same_owner() {
        let store = SqliteGoalStore::new(&temp_db("release_epoch"))
            .await
            .unwrap();
        let goal = store.create(goal("release")).await.unwrap();
        let first = store
            .claim_lease("same-owner", Duration::from_secs(60))
            .await
            .unwrap();

        store.release_lease(&first).await.unwrap();
        assert!(
            store
                .heartbeat_lease(&first, Duration::from_secs(60))
                .await
                .is_err()
        );

        let second = store
            .claim_lease("same-owner", Duration::from_secs(60))
            .await
            .unwrap();
        assert_eq!(second.epoch, first.epoch + 1);

        let error = store
            .update(
                &goal.id,
                goal.version,
                GoalUpdate {
                    title: Some("stale update".into()),
                    ..Default::default()
                },
                None,
                Some(&first),
            )
            .await
            .unwrap_err();
        assert!(matches!(
            error.downcast_ref::<GoalStoreError>(),
            Some(GoalStoreError::StaleLease { .. })
        ));
        assert_eq!(store.get(&goal.id).await.unwrap().title, "release");
    }

    #[tokio::test]
    async fn live_lease_owner_cannot_start_a_second_running_run() {
        let store = SqliteGoalStore::new(&temp_db("duplicate_running_run"))
            .await
            .unwrap();
        let goal = store.create(goal("single run")).await.unwrap();
        let (_token, first) = store
            .start_run(
                "same-owner",
                Duration::from_secs(60),
                GoalRun::new(goal.id.clone()),
            )
            .await
            .unwrap();

        let error = store
            .start_run(
                "same-owner",
                Duration::from_secs(60),
                GoalRun::new(goal.id.clone()),
            )
            .await
            .unwrap_err();
        assert!(matches!(
            error.downcast_ref::<GoalStoreError>(),
            Some(GoalStoreError::ActiveRunExists)
        ));
        assert_eq!(store.list_running_runs().await.unwrap(), vec![first]);
    }

    #[tokio::test]
    async fn finish_run_rejects_incomplete_and_mismatched_records_without_mutation() {
        let store = SqliteGoalStore::new(&temp_db("finish_validation"))
            .await
            .unwrap();
        let first_goal = store.create(goal("first goal")).await.unwrap();
        let second_goal = store.create(goal("second goal")).await.unwrap();
        let (token, original) = store
            .start_run(
                "runner",
                Duration::from_secs(60),
                GoalRun::new(first_goal.id.clone()),
            )
            .await
            .unwrap();

        let incomplete = original.clone();
        let error = store.finish_run(&token, incomplete).await.unwrap_err();
        assert!(matches!(
            error.downcast_ref::<GoalStoreError>(),
            Some(GoalStoreError::RunNotFinished { .. })
        ));
        assert_eq!(
            store.list_running_runs().await.unwrap(),
            vec![original.clone()]
        );

        let mut mismatched = original.clone();
        mismatched.goal_id = second_goal.id;
        mismatched.finish(GoalRunOutcome::ProgressMade);
        let error = store.finish_run(&token, mismatched).await.unwrap_err();
        assert!(matches!(
            error.downcast_ref::<GoalStoreError>(),
            Some(GoalStoreError::RunIdentityMismatch { .. })
        ));
        assert_eq!(store.list_running_runs().await.unwrap(), vec![original]);
    }

    #[tokio::test]
    async fn start_run_rejects_completed_payload_without_persisting_running_row() {
        let store = SqliteGoalStore::new(&temp_db("start_completed"))
            .await
            .unwrap();
        let goal = store.create(goal("completed start")).await.unwrap();
        let mut run = GoalRun::new(goal.id.clone());
        run.finish(GoalRunOutcome::ProgressMade);

        let error = store
            .start_run("runner", Duration::from_secs(60), run)
            .await
            .unwrap_err();

        assert!(matches!(
            error.downcast_ref::<GoalStoreError>(),
            Some(GoalStoreError::RunNotFinished { .. })
        ));
        assert!(store.list_running_runs().await.unwrap().is_empty());
        let lease = sqlx::query("SELECT 1 FROM goal_leases WHERE singleton = 1")
            .fetch_optional(&*store.pool)
            .await
            .unwrap();
        assert!(lease.is_none());
    }

    #[tokio::test]
    async fn finish_run_rejects_changed_started_at_and_preserves_stored_identity() {
        let store = SqliteGoalStore::new(&temp_db("finish_started_at"))
            .await
            .unwrap();
        let goal = store.create(goal("started at")).await.unwrap();
        let (token, original) = store
            .start_run(
                "runner",
                Duration::from_secs(60),
                GoalRun::new(goal.id.clone()),
            )
            .await
            .unwrap();
        let mut changed = original.clone();
        changed.started_at += ChronoDuration::seconds(1);
        changed.finish(GoalRunOutcome::ProgressMade);

        let error = store.finish_run(&token, changed).await.unwrap_err();

        assert!(matches!(
            error.downcast_ref::<GoalStoreError>(),
            Some(GoalStoreError::RunIdentityMismatch { .. })
        ));
        assert_eq!(store.list_running_runs().await.unwrap(), vec![original]);
    }

    #[tokio::test]
    async fn append_orphan_event_returns_not_found_and_rolls_back() {
        let store = SqliteGoalStore::new(&temp_db("orphan_event"))
            .await
            .unwrap();
        let event = GoalEvent {
            id: "orphan-event".into(),
            goal_id: "missing-goal".into(),
            run_id: None,
            kind: "orphan".into(),
            status: None,
            outcome: None,
            payload: None,
            occurred_at: Utc::now(),
        };

        let error = store.append_event(event).await.unwrap_err();
        assert!(matches!(
            error.downcast_ref::<GoalStoreError>(),
            Some(GoalStoreError::NotFound { kind: "goal", id }) if id == "missing-goal"
        ));
        assert!(store.list_events("missing-goal").await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn events_are_ordered_and_duplicate_ids_are_atomic() {
        let store = SqliteGoalStore::new(&temp_db("event_order_duplicate"))
            .await
            .unwrap();
        let created = store.create(goal("events")).await.unwrap();
        let events = (1..=3)
            .map(|index| GoalEvent {
                id: format!("event-{index}"),
                goal_id: created.id.clone(),
                run_id: None,
                kind: format!("kind-{index}"),
                status: None,
                outcome: None,
                payload: None,
                occurred_at: Utc::now(),
            })
            .collect::<Vec<_>>();
        for event in &events {
            store.append_event(event.clone()).await.unwrap();
        }
        assert_eq!(store.list_events(&created.id).await.unwrap(), events);

        let duplicate = store.append_event(events[0].clone()).await.unwrap_err();
        assert!(
            duplicate
                .to_string()
                .contains("failed to append goal event")
        );
        assert_eq!(store.list_events(&created.id).await.unwrap(), events);
    }

    #[tokio::test]
    async fn mismatched_event_rolls_back_aggregate_update() {
        let store = SqliteGoalStore::new(&temp_db("event_goal_mismatch"))
            .await
            .unwrap();
        let created = store.create(goal("rollback")).await.unwrap();
        let other = store.create(goal("other")).await.unwrap();
        let event = GoalEvent {
            id: "mismatched-event".into(),
            goal_id: other.id,
            run_id: None,
            kind: "invalid".into(),
            status: None,
            outcome: None,
            payload: None,
            occurred_at: Utc::now(),
        };

        assert!(
            store
                .update(
                    &created.id,
                    created.version,
                    GoalUpdate {
                        title: Some("must roll back".into()),
                        ..Default::default()
                    },
                    Some(event),
                    None,
                )
                .await
                .is_err()
        );
        assert_eq!(store.get(&created.id).await.unwrap(), created);
        assert!(store.list_events(&created.id).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn duplicate_event_id_rolls_back_cas_aggregate_update() {
        let store = SqliteGoalStore::new(&temp_db("event_duplicate_cas"))
            .await
            .unwrap();
        let created = store.create(goal("duplicate rollback")).await.unwrap();
        let event = GoalEvent {
            id: "duplicate-event".into(),
            goal_id: created.id.clone(),
            run_id: None,
            kind: "first".into(),
            status: None,
            outcome: None,
            payload: None,
            occurred_at: Utc::now(),
        };
        store.append_event(event.clone()).await.unwrap();

        let mut duplicate = event;
        duplicate.kind = "duplicate".into();
        assert!(
            store
                .update(
                    &created.id,
                    created.version,
                    GoalUpdate {
                        title: Some("must roll back".into()),
                        ..Default::default()
                    },
                    Some(duplicate),
                    None,
                )
                .await
                .is_err()
        );
        assert_eq!(store.get(&created.id).await.unwrap(), created);
        assert_eq!(store.list_events(&created.id).await.unwrap().len(), 1);
    }
    #[tokio::test]
    async fn leases_fence_writers_and_expired_takeover_increments_epoch() {
        let store = SqliteGoalStore::new(&temp_db("leases")).await.unwrap();
        let first = store
            .claim_lease("first", Duration::from_millis(20))
            .await
            .unwrap();
        assert_eq!(first.epoch, 1);
        let held = store
            .claim_lease("second", Duration::from_secs(60))
            .await
            .unwrap_err();
        assert!(held.downcast_ref::<GoalStoreError>().is_some());
        tokio::time::sleep(Duration::from_millis(30)).await;
        let second = store
            .claim_lease("second", Duration::from_secs(60))
            .await
            .unwrap();
        assert_eq!(second.epoch, 2);
        assert!(
            store
                .heartbeat_lease(&first, Duration::from_secs(60))
                .await
                .is_err()
        );
        assert!(store.release_lease(&first).await.is_err());
    }

    #[tokio::test]
    async fn run_start_takeover_recovery_and_finish_round_trip() {
        let db = temp_db("runs");
        let store = SqliteGoalStore::new(&db).await.unwrap();
        let goal = store.create(goal("run")).await.unwrap();
        let (first, run) = store
            .start_run(
                "first",
                Duration::from_millis(20),
                GoalRun::new(goal.id.clone()),
            )
            .await
            .unwrap();
        assert_eq!(store.list_running_runs().await.unwrap().len(), 1);
        tokio::time::sleep(Duration::from_millis(30)).await;
        let (second, recovered) = store
            .start_run(
                "second",
                Duration::from_secs(60),
                GoalRun::new(goal.id.clone()),
            )
            .await
            .unwrap();
        assert_eq!(second.epoch, first.epoch + 1);
        assert_eq!(recovered.goal_id, goal.id);
        assert_eq!(store.list_running_runs().await.unwrap().len(), 1);
        let mut finished = run;
        finished.turns_used = 3;
        finished.finish(GoalRunOutcome::ProgressMade);
        assert!(store.finish_run(&first, finished).await.is_err());
        let mut current = recovered;
        current.finish(GoalRunOutcome::CompletionProposed);
        let done = store.finish_run(&second, current).await.unwrap();
        assert_eq!(done.outcome, Some(GoalRunOutcome::CompletionProposed));
    }

    #[tokio::test]
    async fn leases_are_independent_per_database_path() {
        let a = SqliteGoalStore::new(&temp_db("path_a")).await.unwrap();
        let b = SqliteGoalStore::new(&temp_db("path_b")).await.unwrap();
        assert_eq!(
            a.claim_lease("same", Duration::from_secs(60))
                .await
                .unwrap()
                .epoch,
            1
        );
        assert_eq!(
            b.claim_lease("same", Duration::from_secs(60))
                .await
                .unwrap()
                .epoch,
            1
        );
    }
}
