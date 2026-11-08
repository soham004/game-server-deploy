//! The `backups` table: one row per finished archive.

use chrono::{DateTime, Utc};
use sqlx::SqlitePool;

use crate::{
    backups::model::BackupRecord,
    ids::{BackupId, InstanceId},
    store::RepoError,
};

/// The column list, as a macro so every query stays a literal string (sqlx only accepts those).
macro_rules! columns {
    () => {
        "id, instance_id, kind, scope, file_name, size_bytes, sha256, software, note, pinned, created_at, created_by"
    };
}

#[derive(Debug, Clone)]
pub struct BackupRepo {
    pool: SqlitePool,
}

#[derive(sqlx::FromRow)]
struct BackupRow {
    id: String,
    instance_id: String,
    kind: String,
    scope: String,
    file_name: String,
    size_bytes: i64,
    sha256: String,
    software: Option<String>,
    note: Option<String>,
    pinned: bool,
    created_at: DateTime<Utc>,
    created_by: Option<String>,
}

impl TryFrom<BackupRow> for BackupRecord {
    type Error = RepoError;

    fn try_from(row: BackupRow) -> Result<Self, Self::Error> {
        let corrupt = |detail: String| RepoError::corrupt("backups", detail);
        Ok(Self {
            id: row
                .id
                .parse()
                .map_err(|e: crate::ids::IdParseError| corrupt(e.to_string()))?,
            instance_id: row
                .instance_id
                .parse()
                .map_err(|e: crate::ids::IdParseError| corrupt(e.to_string()))?,
            kind: row.kind.parse().map_err(corrupt)?,
            scope: row.scope.parse().map_err(corrupt)?,
            file_name: row.file_name,
            size_bytes: u64::try_from(row.size_bytes).map_err(|e| corrupt(e.to_string()))?,
            sha256: row.sha256,
            software: row.software,
            note: row.note,
            pinned: row.pinned,
            created_at: row.created_at,
            created_by: row
                .created_by
                .map(|id| id.parse())
                .transpose()
                .map_err(|e: crate::ids::IdParseError| corrupt(e.to_string()))?,
        })
    }
}

impl BackupRepo {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    pub async fn insert(&self, backup: &BackupRecord) -> Result<(), RepoError> {
        let size = i64::try_from(backup.size_bytes).map_err(|e| RepoError::corrupt("backups", e))?;
        sqlx::query(concat!(
            "INSERT INTO backups (",
            columns!(),
            ") VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"
        ))
        .bind(backup.id.to_string())
        .bind(backup.instance_id.to_string())
        .bind(backup.kind.as_str())
        .bind(backup.scope.as_str())
        .bind(&backup.file_name)
        .bind(size)
        .bind(&backup.sha256)
        .bind(&backup.software)
        .bind(&backup.note)
        .bind(backup.pinned)
        .bind(backup.created_at)
        .bind(backup.created_by.map(|id| id.to_string()))
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// One instance's backups, newest first.
    pub async fn list(&self, instance: InstanceId) -> Result<Vec<BackupRecord>, RepoError> {
        let rows: Vec<BackupRow> = sqlx::query_as(concat!(
            "SELECT ",
            columns!(),
            " FROM backups WHERE instance_id = ? ORDER BY created_at DESC, id DESC"
        ))
        .bind(instance.to_string())
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(BackupRecord::try_from).collect()
    }

    pub async fn get(&self, instance: InstanceId, id: BackupId) -> Result<Option<BackupRecord>, RepoError> {
        let row: Option<BackupRow> = sqlx::query_as(concat!(
            "SELECT ",
            columns!(),
            " FROM backups WHERE instance_id = ? AND id = ?"
        ))
        .bind(instance.to_string())
        .bind(id.to_string())
        .fetch_optional(&self.pool)
        .await?;
        row.map(BackupRecord::try_from).transpose()
    }

    /// Sets the pin and the note. Returns false when there is no such backup.
    pub async fn update(
        &self,
        instance: InstanceId,
        id: BackupId,
        pinned: bool,
        note: Option<&str>,
    ) -> Result<bool, RepoError> {
        let result = sqlx::query("UPDATE backups SET pinned = ?, note = ? WHERE instance_id = ? AND id = ?")
            .bind(pinned)
            .bind(note)
            .bind(instance.to_string())
            .bind(id.to_string())
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() == 1)
    }

    pub async fn delete(&self, instance: InstanceId, id: BackupId) -> Result<bool, RepoError> {
        let result = sqlx::query("DELETE FROM backups WHERE instance_id = ? AND id = ?")
            .bind(instance.to_string())
            .bind(id.to_string())
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() == 1)
    }

    /// Forgets every backup of an instance (deleted along with it).
    pub async fn delete_all(&self, instance: InstanceId) -> Result<u64, RepoError> {
        let result = sqlx::query("DELETE FROM backups WHERE instance_id = ?")
            .bind(instance.to_string())
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected())
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use chrono::Duration;

    use super::*;
    use crate::{
        backups::model::{BackupKind, BackupScope},
        store,
    };

    fn record(instance: InstanceId, hours_ago: i64) -> BackupRecord {
        BackupRecord {
            id: BackupId::generate(),
            instance_id: instance,
            kind: BackupKind::Manual,
            scope: BackupScope::Full,
            file_name: format!("backup-{hours_ago}.zip"),
            size_bytes: 5_000_000_000,
            sha256: "ab".repeat(32),
            software: Some("Paper 26.3".into()),
            note: None,
            pinned: false,
            created_at: Utc::now() - Duration::hours(hours_ago),
            created_by: None,
        }
    }

    #[tokio::test]
    async fn stores_lists_newest_first_and_keeps_instances_apart() {
        let repo = BackupRepo::new(store::open_in_memory().await.unwrap());
        let (mine, other) = (InstanceId::generate(), InstanceId::generate());
        let older = record(mine, 5);
        let newer = record(mine, 1);
        for backup in [&older, &newer, &record(other, 2)] {
            repo.insert(backup).await.unwrap();
        }

        let listed = repo.list(mine).await.unwrap();
        // Timestamps round-trip through text; compare everything.
        assert_eq!(listed, [newer.clone(), older.clone()]);
        assert_eq!(repo.get(mine, older.id).await.unwrap(), Some(older.clone()));
        assert_eq!(repo.get(other, older.id).await.unwrap(), None, "wrong instance");
    }

    #[tokio::test]
    async fn pins_notes_and_deletes() {
        let repo = BackupRepo::new(store::open_in_memory().await.unwrap());
        let instance = InstanceId::generate();
        let backup = record(instance, 1);
        repo.insert(&backup).await.unwrap();

        assert!(
            repo.update(instance, backup.id, true, Some("before the dragon"))
                .await
                .unwrap()
        );
        let updated = repo.get(instance, backup.id).await.unwrap().unwrap();
        assert!(updated.pinned);
        assert_eq!(updated.note.as_deref(), Some("before the dragon"));
        assert!(
            !repo
                .update(InstanceId::generate(), backup.id, false, None)
                .await
                .unwrap()
        );

        assert!(repo.delete(instance, backup.id).await.unwrap());
        assert!(!repo.delete(instance, backup.id).await.unwrap());
        repo.insert(&record(instance, 2)).await.unwrap();
        repo.insert(&record(instance, 3)).await.unwrap();
        assert_eq!(repo.delete_all(instance).await.unwrap(), 2);
        assert_eq!(repo.list(instance).await.unwrap(), []);
    }
}
