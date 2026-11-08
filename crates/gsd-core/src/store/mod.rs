//! SQLite persistence: opening the database, applying migrations, shared repository errors.
//!
//! Repositories live next to their feature (for example `auth::users::UserRepo`). Services
//! never contain SQL. Migrations are embedded from `src/store/migrations/` and run at startup.

use std::{
    path::{Path, PathBuf},
    str::FromStr,
    time::Duration,
};

use sqlx::{
    migrate::Migrator,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions},
};

pub use sqlx::SqlitePool;

static MIGRATOR: Migrator = sqlx::migrate!("src/store/migrations");

/// Failure to open or migrate the database.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("could not open database {path}")]
    Open {
        path: PathBuf,
        #[source]
        source: sqlx::Error,
    },
    #[error("database migration failed")]
    Migrate(#[from] sqlx::migrate::MigrateError),
    #[error("could not restrict permissions of {path}")]
    Permissions {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

/// Failure inside a repository.
#[derive(Debug, thiserror::Error)]
pub enum RepoError {
    #[error(transparent)]
    Db(#[from] sqlx::Error),
    #[error("corrupt row in table {table}: {detail}")]
    Corrupt { table: &'static str, detail: String },
}

impl RepoError {
    pub(crate) fn corrupt(table: &'static str, detail: impl ToString) -> Self {
        Self::Corrupt {
            table,
            detail: detail.to_string(),
        }
    }

    /// True when the error is a UNIQUE constraint violation (e.g. a duplicate username).
    pub fn is_unique_violation(&self) -> bool {
        matches!(self, Self::Db(sqlx::Error::Database(db)) if db.is_unique_violation())
    }
}

/// Opens (creating if needed) the database file and applies pending migrations.
pub async fn open(path: &Path) -> Result<SqlitePool, StoreError> {
    let options = SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .foreign_keys(true)
        .busy_timeout(Duration::from_secs(5));

    let pool = SqlitePoolOptions::new()
        .max_connections(8)
        .connect_with(options)
        .await
        .map_err(|source| StoreError::Open {
            path: path.to_owned(),
            source,
        })?;

    restrict_permissions(path)?;
    MIGRATOR.run(&pool).await?;
    Ok(pool)
}

/// A private in-memory database with migrations applied. Used by tests.
pub async fn open_in_memory() -> Result<SqlitePool, StoreError> {
    let path = PathBuf::from(":memory:");
    let open_error = |source| StoreError::Open {
        path: path.clone(),
        source,
    };

    let options = SqliteConnectOptions::from_str("sqlite::memory:")
        .map_err(open_error)?
        .foreign_keys(true);
    // WHY: every connection to `:memory:` is a separate database, so keep exactly one alive forever.
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .idle_timeout(None)
        .max_lifetime(None)
        .connect_with(options)
        .await
        .map_err(open_error)?;

    MIGRATOR.run(&pool).await?;
    Ok(pool)
}

/// The database holds password hashes and session hashes: owner read/write only.
fn restrict_permissions(path: &Path) -> Result<(), StoreError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).map_err(|source| {
            StoreError::Permissions {
                path: path.to_owned(),
                source,
            }
        })?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use sqlx::Row;

    use super::*;

    /// Migration 0004 rebuilds the backups table: rows made under 0002 must survive it.
    #[tokio::test]
    async fn rebuilding_the_backups_table_keeps_existing_backups() {
        // One connection: each connection to :memory: is its own database.
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        for migration in [
            include_str!("migrations/0001_auth_and_audit.sql"),
            include_str!("migrations/0002_backups.sql"),
        ] {
            sqlx::raw_sql(migration).execute(&pool).await.unwrap();
        }
        sqlx::query(
            "INSERT INTO backups (id, instance_id, kind, scope, file_name, size_bytes, sha256, pinned, created_at) \
             VALUES ('b1', 'i1', 'manual', 'world', 'a.zip', 10, 'abc', 1, '2026-11-08T00:00:00Z')",
        )
        .execute(&pool)
        .await
        .unwrap();

        sqlx::raw_sql(include_str!("migrations/0004_backup_kind_pre_update.sql"))
            .execute(&pool)
            .await
            .unwrap();

        let row = sqlx::query("SELECT kind, file_name, pinned FROM backups WHERE id = 'b1'")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(row.get::<String, _>("kind"), "manual");
        assert_eq!(row.get::<String, _>("file_name"), "a.zip");
        assert!(row.get::<bool, _>("pinned"));
        sqlx::query(
            "INSERT INTO backups (id, instance_id, kind, scope, file_name, size_bytes, sha256, created_at) \
             VALUES ('b2', 'i1', 'pre_update', 'full', 'b.zip', 10, 'abc', '2026-11-08T00:00:00Z')",
        )
        .execute(&pool)
        .await
        .unwrap();
    }
}
