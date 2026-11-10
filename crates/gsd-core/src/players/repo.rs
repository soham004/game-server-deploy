//! The `player_sessions` table.

use chrono::{DateTime, Utc};
use sqlx::SqlitePool;

use super::Session;
use crate::{ids::InstanceId, store::RepoError};

/// Visits kept per server; the oldest go when the app starts.
pub const SESSIONS_KEPT: i64 = 10_000;

#[derive(Debug, Clone)]
pub(super) struct SessionRepo {
    pool: SqlitePool,
}

#[derive(sqlx::FromRow)]
struct SessionRow {
    name: String,
    joined_at: DateTime<Utc>,
    left_at: Option<DateTime<Utc>>,
    open: bool,
}

impl From<SessionRow> for Session {
    fn from(row: SessionRow) -> Self {
        Self {
            name: row.name,
            joined_at: row.joined_at,
            left_at: row.left_at,
            online: row.open,
        }
    }
}

impl SessionRepo {
    pub(super) fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    pub(super) async fn open(&self, id: InstanceId, name: &str, at: DateTime<Utc>) -> Result<(), RepoError> {
        sqlx::query("INSERT INTO player_sessions (instance_id, name, joined_at, open) VALUES (?, ?, ?, 1)")
            .bind(id.to_string())
            .bind(name)
            .bind(at)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Ends the open visits of one player (any case), or of everyone. `at` is `None` when the end wasn't seen.
    pub(super) async fn close(
        &self,
        id: InstanceId,
        name: Option<&str>,
        at: Option<DateTime<Utc>>,
    ) -> Result<(), RepoError> {
        sqlx::query(
            "UPDATE player_sessions SET open = 0, left_at = ? \
             WHERE instance_id = ? AND open = 1 AND (? IS NULL OR name = ? COLLATE NOCASE)",
        )
        .bind(at)
        .bind(id.to_string())
        .bind(name)
        .bind(name)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// At startup: whatever is open was left so by an earlier run, which can't say when it ended.
    pub(super) async fn close_all_unseen(&self) -> Result<(), RepoError> {
        sqlx::query("UPDATE player_sessions SET open = 0 WHERE open = 1")
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Keeps the newest [`SESSIONS_KEPT`] visits of each server.
    pub(super) async fn prune(&self) -> Result<(), RepoError> {
        sqlx::query(
            "DELETE FROM player_sessions WHERE id IN ( \
               SELECT id FROM ( \
                 SELECT id, ROW_NUMBER() OVER (PARTITION BY instance_id ORDER BY joined_at DESC, id DESC) AS place \
                 FROM player_sessions) \
               WHERE place > ?)",
        )
        .bind(SESSIONS_KEPT)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub(super) async fn delete_instance(&self, id: InstanceId) -> Result<(), RepoError> {
        sqlx::query("DELETE FROM player_sessions WHERE instance_id = ?")
            .bind(id.to_string())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Every visit to the server, oldest first.
    pub(super) async fn all(&self, id: InstanceId) -> Result<Vec<Session>, RepoError> {
        let rows: Vec<SessionRow> = sqlx::query_as(
            "SELECT name, joined_at, left_at, open FROM player_sessions WHERE instance_id = ? ORDER BY joined_at, id",
        )
        .bind(id.to_string())
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(Session::from).collect())
    }

    pub(super) async fn of_player(
        &self,
        id: InstanceId,
        name: &str,
        limit: u32,
    ) -> Result<Vec<Session>, RepoError> {
        let rows: Vec<SessionRow> = sqlx::query_as(
            "SELECT name, joined_at, left_at, open FROM player_sessions \
             WHERE instance_id = ? AND name = ? COLLATE NOCASE ORDER BY joined_at DESC, id DESC LIMIT ?",
        )
        .bind(id.to_string())
        .bind(name)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(Session::from).collect())
    }
}
