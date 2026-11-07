//! Login sessions.
//!
//! The browser holds a random 256-bit token in an `HttpOnly` cookie; the database only stores
//! its SHA-256, so a leaked database cannot be used to hijack sessions.

use chrono::{DateTime, Utc};
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;

use crate::{ids::UserId, store::RepoError};

/// Hex SHA-256 of a session token, as stored in `sessions.token_hash`.
pub fn hash_token(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}

/// An active session.
#[derive(Debug)]
pub struct SessionRecord {
    pub user_id: UserId,
    pub expires_at: DateTime<Utc>,
    pub last_seen_at: DateTime<Utc>,
}

/// Data for a new session.
#[derive(Debug)]
pub struct NewSession<'a> {
    pub token_hash: &'a str,
    pub user_id: UserId,
    pub now: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub user_agent: Option<&'a str>,
    pub ip: Option<String>,
}

#[derive(sqlx::FromRow)]
struct SessionRow {
    user_id: String,
    expires_at: DateTime<Utc>,
    last_seen_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct SessionRepo {
    pool: SqlitePool,
}

impl SessionRepo {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    pub async fn insert(&self, session: &NewSession<'_>) -> Result<(), RepoError> {
        sqlx::query(
            "INSERT INTO sessions (token_hash, user_id, created_at, expires_at, last_seen_at, user_agent, ip)
             VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(session.token_hash)
        .bind(session.user_id.to_string())
        .bind(session.now)
        .bind(session.expires_at)
        .bind(session.now)
        .bind(session.user_agent)
        .bind(session.ip.as_deref())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// The session for this token hash, if it exists and has not expired at `now`.
    pub async fn find_active(
        &self,
        token_hash: &str,
        now: DateTime<Utc>,
    ) -> Result<Option<SessionRecord>, RepoError> {
        let row: Option<SessionRow> = sqlx::query_as(
            "SELECT user_id, expires_at, last_seen_at FROM sessions WHERE token_hash = ? AND expires_at > ?",
        )
        .bind(token_hash)
        .bind(now)
        .fetch_optional(&self.pool)
        .await?;

        row.map(|row| {
            Ok(SessionRecord {
                user_id: row
                    .user_id
                    .parse()
                    .map_err(|e| RepoError::corrupt("sessions", format!("{e}")))?,
                expires_at: row.expires_at,
                last_seen_at: row.last_seen_at,
            })
        })
        .transpose()
    }

    /// Sliding expiry: record activity and push the expiry forward.
    pub async fn extend(
        &self,
        token_hash: &str,
        now: DateTime<Utc>,
        expires_at: DateTime<Utc>,
    ) -> Result<(), RepoError> {
        sqlx::query("UPDATE sessions SET last_seen_at = ?, expires_at = ? WHERE token_hash = ?")
            .bind(now)
            .bind(expires_at)
            .bind(token_hash)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn delete(&self, token_hash: &str) -> Result<(), RepoError> {
        sqlx::query("DELETE FROM sessions WHERE token_hash = ?")
            .bind(token_hash)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn delete_for_user(&self, user_id: UserId) -> Result<(), RepoError> {
        sqlx::query("DELETE FROM sessions WHERE user_id = ?")
            .bind(user_id.to_string())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Removes expired sessions; returns how many were deleted.
    pub async fn delete_expired(&self, now: DateTime<Utc>) -> Result<u64, RepoError> {
        let result = sqlx::query("DELETE FROM sessions WHERE expires_at <= ?")
            .bind(now)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_hash_is_stable_hex() {
        let hash = hash_token("abc");
        assert_eq!(hash.len(), 64);
        assert_eq!(hash, hash_token("abc"));
        assert_ne!(hash, hash_token("abd"));
    }
}
