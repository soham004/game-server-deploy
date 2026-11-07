//! Per-instance grants for `member` users.

use std::collections::HashMap;

use sqlx::SqlitePool;

use crate::{
    auth::{Grant, PermissionSet},
    ids::{InstanceId, UserId},
    store::RepoError,
};

#[derive(sqlx::FromRow)]
struct GrantRow {
    instance_id: String,
    preset: String,
    permissions_json: Option<String>,
}

impl GrantRow {
    fn into_entry(self) -> Result<(InstanceId, Grant), RepoError> {
        let corrupt = |detail: String| RepoError::corrupt("instance_grants", detail);
        let instance_id: InstanceId = self.instance_id.parse().map_err(|e| corrupt(format!("{e}")))?;
        let custom = self
            .permissions_json
            .as_deref()
            .map(serde_json::from_str::<PermissionSet>)
            .transpose()
            .map_err(|e| corrupt(e.to_string()))?;
        let grant = Grant::from_stored(&self.preset, custom)
            .ok_or_else(|| corrupt(format!("unknown preset {:?}", self.preset)))?;
        Ok((instance_id, grant))
    }
}

#[derive(Debug, Clone)]
pub struct GrantRepo {
    pool: SqlitePool,
}

impl GrantRepo {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// Effective permissions per instance for one user.
    pub async fn for_user(&self, user_id: UserId) -> Result<HashMap<InstanceId, PermissionSet>, RepoError> {
        let rows: Vec<GrantRow> = sqlx::query_as(
            "SELECT instance_id, preset, permissions_json FROM instance_grants WHERE user_id = ?",
        )
        .bind(user_id.to_string())
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter()
            .map(|row| {
                row.into_entry()
                    .map(|(instance, grant)| (instance, grant.permissions()))
            })
            .collect()
    }

    /// Creates or replaces the grant of `user_id` on `instance_id`.
    pub async fn set(&self, user_id: UserId, instance_id: InstanceId, grant: Grant) -> Result<(), RepoError> {
        let permissions_json = match grant {
            Grant::Custom(set) => {
                Some(serde_json::to_string(&set).map_err(|e| RepoError::corrupt("instance_grants", e))?)
            }
            _ => None,
        };
        sqlx::query(
            "INSERT INTO instance_grants (user_id, instance_id, preset, permissions_json) VALUES (?, ?, ?, ?)
             ON CONFLICT (user_id, instance_id) DO UPDATE SET preset = excluded.preset,
                                                              permissions_json = excluded.permissions_json",
        )
        .bind(user_id.to_string())
        .bind(instance_id.to_string())
        .bind(grant.preset_name())
        .bind(permissions_json)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn remove(&self, user_id: UserId, instance_id: InstanceId) -> Result<(), RepoError> {
        sqlx::query("DELETE FROM instance_grants WHERE user_id = ? AND instance_id = ?")
            .bind(user_id.to_string())
            .bind(instance_id.to_string())
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}
