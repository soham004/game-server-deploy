//! Settings of the app itself, kept in the database. That file is readable only by the app (see
//! `store`) and no server's sandbox can reach it, which is why a user's API key can live here.

use chrono::Utc;
use sqlx::SqlitePool;

use crate::store::RepoError;

/// The user's `CurseForge` API key. Never sent to the browser or written to a log.
pub const CURSEFORGE_API_KEY: &str = "curseforge_api_key";

#[derive(Debug, Clone)]
pub struct AppSettings {
    pool: SqlitePool,
}

impl AppSettings {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    pub async fn get(&self, key: &str) -> Result<Option<String>, RepoError> {
        Ok(sqlx::query_scalar("SELECT value FROM app_settings WHERE key = ?")
            .bind(key)
            .fetch_optional(&self.pool)
            .await?)
    }

    pub async fn set(&self, key: &str, value: &str) -> Result<(), RepoError> {
        sqlx::query(
            "INSERT INTO app_settings (key, value, updated_at) VALUES (?, ?, ?) \
             ON CONFLICT (key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
        )
        .bind(key)
        .bind(value)
        .bind(Utc::now().to_rfc3339())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn remove(&self, key: &str) -> Result<(), RepoError> {
        sqlx::query("DELETE FROM app_settings WHERE key = ?")
            .bind(key)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use crate::store;

    #[tokio::test]
    async fn values_are_set_replaced_and_removed() {
        let settings = AppSettings::new(store::open_in_memory().await.unwrap());
        assert_eq!(settings.get(CURSEFORGE_API_KEY).await.unwrap(), None);
        settings.set(CURSEFORGE_API_KEY, "one").await.unwrap();
        settings.set(CURSEFORGE_API_KEY, "two").await.unwrap();
        assert_eq!(
            settings.get(CURSEFORGE_API_KEY).await.unwrap().as_deref(),
            Some("two")
        );
        settings.remove(CURSEFORGE_API_KEY).await.unwrap();
        settings.remove(CURSEFORGE_API_KEY).await.unwrap();
        assert_eq!(settings.get(CURSEFORGE_API_KEY).await.unwrap(), None);
    }
}
