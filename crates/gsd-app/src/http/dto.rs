//! Request and response bodies shared by several routes. Domain types are mapped explicitly so
//! the wire format doesn't change by accident when internals do.

use axum::extract::FromRequest;
use chrono::{DateTime, Utc};
use gsd_core::auth::{GlobalRole, User};
use serde::Serialize;

use crate::http::error::ApiError;

/// `axum::Json`, but malformed bodies produce our JSON error format.
#[derive(Debug, FromRequest)]
#[from_request(via(axum::Json), rejection(ApiError))]
pub struct ApiJson<T>(pub T);

#[derive(Debug, Serialize)]
pub struct UserDto {
    pub id: String,
    pub username: String,
    pub role: GlobalRole,
    pub must_change_password: bool,
    pub created_at: DateTime<Utc>,
}

impl From<&User> for UserDto {
    fn from(user: &User) -> Self {
        Self {
            id: user.id.to_string(),
            username: user.username.to_string(),
            role: user.role,
            must_change_password: user.must_change_password,
            created_at: user.created_at,
        }
    }
}

/// Body of responses that sign a user in.
#[derive(Debug, Serialize)]
pub struct SessionResponse {
    pub user: UserDto,
}
