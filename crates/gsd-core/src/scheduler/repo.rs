//! The `schedules` and `schedule_runs` tables.

use chrono::{DateTime, Utc};
use sqlx::SqlitePool;

use super::model::{RunCause, RunOutcome, Schedule, SchedulePlan, ScheduleRun};
use crate::{
    ids::{IdParseError, InstanceId, ScheduleId, UserId},
    store::RepoError,
};

/// Runs kept per schedule; older ones are removed when a run is recorded.
pub const RUNS_KEPT: i64 = 50;

#[derive(Debug, Clone)]
pub struct ScheduleRepo {
    pool: SqlitePool,
}

#[derive(sqlx::FromRow)]
struct ScheduleRow {
    id: String,
    instance_id: String,
    plan_json: String,
    run_as: Option<String>,
    created_at: DateTime<Utc>,
    last_run_at: Option<DateTime<Utc>>,
    next_run_at: Option<DateTime<Utc>>,
}

impl TryFrom<ScheduleRow> for Schedule {
    type Error = RepoError;

    fn try_from(row: ScheduleRow) -> Result<Self, Self::Error> {
        let corrupt = |detail: String| RepoError::corrupt("schedules", detail);
        let id_error = |e: IdParseError| corrupt(e.to_string());
        Ok(Self {
            id: row.id.parse().map_err(id_error)?,
            instance_id: row.instance_id.parse().map_err(id_error)?,
            plan: serde_json::from_str(&row.plan_json).map_err(|e| corrupt(e.to_string()))?,
            run_as: row.run_as.map(|id| id.parse()).transpose().map_err(id_error)?,
            created_at: row.created_at,
            last_run_at: row.last_run_at,
            next_run_at: row.next_run_at,
        })
    }
}

#[derive(sqlx::FromRow)]
struct RunRow {
    id: i64,
    schedule_id: String,
    cause: String,
    started_at: DateTime<Utc>,
    finished_at: Option<DateTime<Utc>>,
    outcome: String,
    message: Option<String>,
}

impl TryFrom<RunRow> for ScheduleRun {
    type Error = RepoError;

    fn try_from(row: RunRow) -> Result<Self, Self::Error> {
        let corrupt = |detail: String| RepoError::corrupt("schedule_runs", detail);
        Ok(Self {
            id: row.id,
            schedule_id: row
                .schedule_id
                .parse()
                .map_err(|e: IdParseError| corrupt(e.to_string()))?,
            cause: parse_text(&row.cause).map_err(corrupt)?,
            started_at: row.started_at,
            finished_at: row.finished_at,
            outcome: parse_text(&row.outcome).map_err(corrupt)?,
            message: row.message,
        })
    }
}

impl ScheduleRepo {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    pub async fn insert(&self, schedule: &Schedule) -> Result<(), RepoError> {
        sqlx::query(
            "INSERT INTO schedules (id, instance_id, plan_json, run_as, created_at, last_run_at, next_run_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(schedule.id.to_string())
        .bind(schedule.instance_id.to_string())
        .bind(plan_json(&schedule.plan)?)
        .bind(schedule.run_as.map(|id| id.to_string()))
        .bind(schedule.created_at)
        .bind(schedule.last_run_at)
        .bind(schedule.next_run_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Replaces the plan and who it runs as (an edit). Returns false when there is no such schedule.
    pub async fn update_plan(
        &self,
        id: ScheduleId,
        plan: &SchedulePlan,
        run_as: UserId,
        next_run_at: Option<DateTime<Utc>>,
    ) -> Result<bool, RepoError> {
        let result =
            sqlx::query("UPDATE schedules SET plan_json = ?, run_as = ?, next_run_at = ? WHERE id = ?")
                .bind(plan_json(plan)?)
                .bind(run_as.to_string())
                .bind(next_run_at)
                .bind(id.to_string())
                .execute(&self.pool)
                .await?;
        Ok(result.rows_affected() == 1)
    }

    /// Records when it last ran and when it runs next.
    pub async fn set_times(
        &self,
        id: ScheduleId,
        last_run_at: Option<DateTime<Utc>>,
        next_run_at: Option<DateTime<Utc>>,
    ) -> Result<(), RepoError> {
        sqlx::query("UPDATE schedules SET last_run_at = ?, next_run_at = ? WHERE id = ?")
            .bind(last_run_at)
            .bind(next_run_at)
            .bind(id.to_string())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn get(&self, id: ScheduleId) -> Result<Option<Schedule>, RepoError> {
        let row: Option<ScheduleRow> = sqlx::query_as(
            "SELECT id, instance_id, plan_json, run_as, created_at, last_run_at, next_run_at FROM schedules WHERE id = ?",
        )
        .bind(id.to_string())
        .fetch_optional(&self.pool)
        .await?;
        row.map(Schedule::try_from).transpose()
    }

    /// Every schedule, or one instance's, oldest first.
    pub async fn list(&self, instance: Option<InstanceId>) -> Result<Vec<Schedule>, RepoError> {
        let rows: Vec<ScheduleRow> = sqlx::query_as(
            "SELECT id, instance_id, plan_json, run_as, created_at, last_run_at, next_run_at FROM schedules \
             WHERE ?1 IS NULL OR instance_id = ?1 ORDER BY created_at, id",
        )
        .bind(instance.map(|id| id.to_string()))
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(Schedule::try_from).collect()
    }

    /// Deletes a schedule and its runs.
    pub async fn delete(&self, id: ScheduleId) -> Result<bool, RepoError> {
        let result = sqlx::query("DELETE FROM schedules WHERE id = ?")
            .bind(id.to_string())
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() == 1)
    }

    /// Deletes every schedule of an instance (it was deleted).
    pub async fn delete_for_instance(&self, instance: InstanceId) -> Result<u64, RepoError> {
        let result = sqlx::query("DELETE FROM schedules WHERE instance_id = ?")
            .bind(instance.to_string())
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected())
    }

    /// Starts a run record; returns its id. Old runs beyond [`RUNS_KEPT`] are removed.
    pub async fn start_run(
        &self,
        schedule: ScheduleId,
        cause: RunCause,
        started_at: DateTime<Utc>,
    ) -> Result<i64, RepoError> {
        let id = sqlx::query(
            "INSERT INTO schedule_runs (schedule_id, cause, started_at, outcome) VALUES (?, ?, ?, 'running')",
        )
        .bind(schedule.to_string())
        .bind(cause.as_str())
        .bind(started_at)
        .execute(&self.pool)
        .await?
        .last_insert_rowid();
        sqlx::query(
            "DELETE FROM schedule_runs WHERE schedule_id = ?1 AND id NOT IN \
             (SELECT id FROM schedule_runs WHERE schedule_id = ?1 ORDER BY id DESC LIMIT ?2)",
        )
        .bind(schedule.to_string())
        .bind(RUNS_KEPT)
        .execute(&self.pool)
        .await?;
        Ok(id)
    }

    pub async fn finish_run(
        &self,
        run: i64,
        outcome: RunOutcome,
        message: Option<&str>,
        finished_at: DateTime<Utc>,
    ) -> Result<(), RepoError> {
        sqlx::query("UPDATE schedule_runs SET outcome = ?, message = ?, finished_at = ? WHERE id = ?")
            .bind(outcome.as_str())
            .bind(message)
            .bind(finished_at)
            .bind(run)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// A schedule's runs, newest first.
    pub async fn runs(&self, schedule: ScheduleId, limit: i64) -> Result<Vec<ScheduleRun>, RepoError> {
        let rows: Vec<RunRow> = sqlx::query_as(
            "SELECT id, schedule_id, cause, started_at, finished_at, outcome, message FROM schedule_runs \
             WHERE schedule_id = ? ORDER BY id DESC LIMIT ?",
        )
        .bind(schedule.to_string())
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(ScheduleRun::try_from).collect()
    }

    /// Runs left "running" by an app that stopped mid-run.
    pub async fn close_interrupted_runs(&self, at: DateTime<Utc>) -> Result<u64, RepoError> {
        let result = sqlx::query(
            "UPDATE schedule_runs SET outcome = 'failed', message = 'The app stopped during this run.', \
             finished_at = ? WHERE outcome = 'running'",
        )
        .bind(at)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected())
    }
}

fn plan_json(plan: &SchedulePlan) -> Result<String, RepoError> {
    serde_json::to_string(plan).map_err(|e| RepoError::corrupt("schedules", e))
}

/// Reads a `snake_case` enum stored as text, through its serde name.
fn parse_text<T: serde::de::DeserializeOwned>(text: &str) -> Result<T, String> {
    serde_json::from_value(serde_json::Value::String(text.to_owned())).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests;
