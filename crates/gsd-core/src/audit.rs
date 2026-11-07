//! Audit trail: who did what, when, to which instance.
//!
//! Recording is best effort: a failure is logged loudly but never fails the user's action.

use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::SqlitePool;

use crate::{
    ids::{InstanceId, UserId},
    store::RepoError,
};

/// Stable action names stored in `audit_log.action`.
pub mod actions {
    pub const SETUP_COMPLETED: &str = "auth.setup_completed";
    pub const LOGIN: &str = "auth.login";
    pub const LOGIN_FAILED: &str = "auth.login_failed";
    pub const LOGOUT: &str = "auth.logout";
    pub const PASSWORD_CHANGED: &str = "auth.password_changed";
    pub const PASSWORD_RESET: &str = "auth.password_reset";
    pub const INSTANCE_CREATED: &str = "instance.created";
    pub const INSTANCE_DELETED: &str = "instance.deleted";
    pub const INSTANCE_REINSTALLED: &str = "instance.reinstalled";
    pub const INSTANCE_STARTED: &str = "instance.started";
    pub const INSTANCE_STOPPED: &str = "instance.stopped";
    pub const INSTANCE_RESTARTED: &str = "instance.restarted";
    pub const INSTANCE_KILLED: &str = "instance.killed";
    pub const CONSOLE_COMMAND: &str = "console.command";
    pub const PROPERTIES_CHANGED: &str = "properties.changed";
    pub const FILES_CHANGED: &str = "files.changed";
    pub const SETTINGS_CHANGED: &str = "instance.settings_changed";
    pub const USER_CREATED: &str = "user.created";
    pub const USER_UPDATED: &str = "user.updated";
    pub const USER_DELETED: &str = "user.deleted";
    pub const USER_SIGNED_OUT: &str = "user.signed_out";
    pub const GRANT_CHANGED: &str = "user.grant_changed";
    pub const JAVA_INSTALLED: &str = "java.install_requested";
}

/// One event to record.
#[derive(Debug)]
pub struct AuditEntry {
    pub user_id: Option<UserId>,
    pub instance_id: Option<InstanceId>,
    pub action: &'static str,
    pub details: Value,
}

impl AuditEntry {
    pub fn by_user(user_id: UserId, action: &'static str) -> Self {
        Self {
            user_id: Some(user_id),
            instance_id: None,
            action,
            details: Value::Null,
        }
    }

    pub fn anonymous(action: &'static str) -> Self {
        Self {
            user_id: None,
            instance_id: None,
            action,
            details: Value::Null,
        }
    }

    /// An action by a user on one instance.
    pub fn on_instance(user_id: UserId, instance_id: InstanceId, action: &'static str) -> Self {
        Self {
            user_id: Some(user_id),
            instance_id: Some(instance_id),
            action,
            details: Value::Null,
        }
    }

    #[must_use]
    pub fn with_details(mut self, details: Value) -> Self {
        self.details = details;
        self
    }
}

/// A stored audit event.
#[derive(Debug, sqlx::FromRow)]
pub struct AuditRecord {
    pub id: i64,
    pub ts: DateTime<Utc>,
    pub user_id: Option<String>,
    pub instance_id: Option<String>,
    pub action: String,
    pub details_json: String,
}

#[derive(Debug, Clone)]
pub struct AuditLog {
    pool: SqlitePool,
}

impl AuditLog {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// Records an event. Never fails; errors are logged.
    pub async fn record(&self, entry: AuditEntry) {
        if let Err(error) = self.insert(&entry).await {
            tracing::error!(%error, action = entry.action, "failed to write audit log entry");
        }
    }

    /// Most recent events first.
    pub async fn recent(&self, limit: u32) -> Result<Vec<AuditRecord>, RepoError> {
        Ok(sqlx::query_as(
            "SELECT id, ts, user_id, instance_id, action, details_json FROM audit_log ORDER BY id DESC LIMIT ?",
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await?)
    }

    async fn insert(&self, entry: &AuditEntry) -> Result<(), RepoError> {
        let details = if entry.details.is_null() {
            "{}".to_owned()
        } else {
            entry.details.to_string()
        };
        sqlx::query(
            "INSERT INTO audit_log (ts, user_id, instance_id, action, details_json) VALUES (?, ?, ?, ?, ?)",
        )
        .bind(Utc::now())
        .bind(entry.user_id.map(|id| id.to_string()))
        .bind(entry.instance_id.map(|id| id.to_string()))
        .bind(entry.action)
        .bind(details)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store;

    #[tokio::test]
    async fn records_and_lists_newest_first() {
        let log = AuditLog::new(store::open_in_memory().await.unwrap());
        log.record(AuditEntry::anonymous(actions::LOGIN_FAILED)).await;
        log.record(AuditEntry::by_user(UserId::generate(), actions::LOGIN))
            .await;

        let recent = log.recent(10).await.unwrap();
        assert_eq!(recent.len(), 2);
        assert_eq!(recent[0].action, actions::LOGIN);
    }
}
