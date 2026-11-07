//! User accounts and their repository.

use chrono::{DateTime, Utc};
use sqlx::SqlitePool;

use crate::{
    auth::{GlobalRole, Username},
    ids::UserId,
    store::RepoError,
};

/// A user account (without the password hash).
#[derive(Debug, Clone)]
pub struct User {
    pub id: UserId,
    pub username: Username,
    pub role: GlobalRole,
    /// Set after an admin reset; the user must choose a new password before doing anything else.
    pub must_change_password: bool,
    pub disabled: bool,
    pub created_at: DateTime<Utc>,
}

/// A user together with their password hash. Only the auth service sees this.
#[derive(Debug)]
pub struct StoredUser {
    pub user: User,
    pub password_hash: String,
}

/// Data for a user that does not exist yet.
#[derive(Debug)]
pub struct NewUser {
    pub username: Username,
    pub password_hash: String,
    pub role: GlobalRole,
    pub must_change_password: bool,
}

#[derive(sqlx::FromRow)]
struct UserRow {
    id: String,
    username: String,
    password_hash: String,
    global_role: String,
    must_change_password: bool,
    disabled: bool,
    created_at: DateTime<Utc>,
}

impl TryFrom<UserRow> for StoredUser {
    type Error = RepoError;

    fn try_from(row: UserRow) -> Result<Self, Self::Error> {
        let corrupt = |detail: String| RepoError::corrupt("users", detail);
        let user = User {
            id: row
                .id
                .parse()
                .map_err(|e: crate::ids::IdParseError| corrupt(e.to_string()))?,
            username: Username::parse(&row.username).map_err(|e| corrupt(e.to_string()))?,
            role: row
                .global_role
                .parse()
                .map_err(|e: super::permissions::UnknownRole| corrupt(e.to_string()))?,
            must_change_password: row.must_change_password,
            disabled: row.disabled,
            created_at: row.created_at,
        };
        Ok(Self {
            user,
            password_hash: row.password_hash,
        })
    }
}

macro_rules! user_columns {
    () => {
        "id, username, password_hash, global_role, must_change_password, disabled, created_at"
    };
}

#[derive(Debug, Clone)]
pub struct UserRepo {
    pool: SqlitePool,
}

impl UserRepo {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    pub async fn count(&self) -> Result<i64, RepoError> {
        Ok(sqlx::query_scalar("SELECT COUNT(*) FROM users")
            .fetch_one(&self.pool)
            .await?)
    }

    /// Inserts a user. Fails with a unique violation if the username is taken.
    pub async fn insert(&self, new: &NewUser) -> Result<User, RepoError> {
        let user = Self::user_from_new(new);
        sqlx::query(
            "INSERT INTO users (id, username, password_hash, global_role, must_change_password, disabled, created_at)
             VALUES (?, ?, ?, ?, ?, 0, ?)",
        )
        .bind(user.id.to_string())
        .bind(user.username.as_str())
        .bind(&new.password_hash)
        .bind(user.role.as_str())
        .bind(user.must_change_password)
        .bind(user.created_at)
        .execute(&self.pool)
        .await?;
        Ok(user)
    }

    /// Inserts the user only if the table is empty, atomically. Returns `None` if a user
    /// already existed, which makes concurrent first-run setups safe.
    pub async fn insert_if_no_users(&self, new: &NewUser) -> Result<Option<User>, RepoError> {
        let user = Self::user_from_new(new);
        let result = sqlx::query(
            "INSERT INTO users (id, username, password_hash, global_role, must_change_password, disabled, created_at)
             SELECT ?, ?, ?, ?, ?, 0, ?
             WHERE NOT EXISTS (SELECT 1 FROM users)",
        )
        .bind(user.id.to_string())
        .bind(user.username.as_str())
        .bind(&new.password_hash)
        .bind(user.role.as_str())
        .bind(user.must_change_password)
        .bind(user.created_at)
        .execute(&self.pool)
        .await?;
        Ok((result.rows_affected() == 1).then_some(user))
    }

    /// Case-insensitive lookup.
    pub async fn find_by_username(&self, username: &str) -> Result<Option<StoredUser>, RepoError> {
        let row: Option<UserRow> = sqlx::query_as(concat!(
            "SELECT ",
            user_columns!(),
            " FROM users WHERE username = ?"
        ))
        .bind(username)
        .fetch_optional(&self.pool)
        .await?;
        row.map(StoredUser::try_from).transpose()
    }

    pub async fn find_by_id(&self, id: UserId) -> Result<Option<StoredUser>, RepoError> {
        let row: Option<UserRow> =
            sqlx::query_as(concat!("SELECT ", user_columns!(), " FROM users WHERE id = ?"))
                .bind(id.to_string())
                .fetch_optional(&self.pool)
                .await?;
        row.map(StoredUser::try_from).transpose()
    }

    pub async fn update_password(
        &self,
        id: UserId,
        password_hash: &str,
        must_change_password: bool,
    ) -> Result<(), RepoError> {
        sqlx::query("UPDATE users SET password_hash = ?, must_change_password = ? WHERE id = ?")
            .bind(password_hash)
            .bind(must_change_password)
            .bind(id.to_string())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    fn user_from_new(new: &NewUser) -> User {
        User {
            id: UserId::generate(),
            username: new.username.clone(),
            role: new.role,
            must_change_password: new.must_change_password,
            disabled: false,
            created_at: Utc::now(),
        }
    }
}
