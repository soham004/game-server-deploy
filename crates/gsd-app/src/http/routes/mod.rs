//! API route groups, one module per feature. Each exposes `routes() -> Router<AppState>`.

pub mod auth;
pub mod backups;
pub mod catalog;
pub mod content;
pub mod files;
pub mod imports;
pub mod instances;
pub mod java;
pub mod jobs;
pub mod me;
pub mod properties;
pub mod schedules;
pub mod settings;
pub mod setup;
pub mod system;
pub mod users;

use axum::{
    Json,
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use gsd_core::auth::LoginOutcome;

use crate::{
    http::{
        client::ClientContext,
        dto::{SessionResponse, UserDto},
        error::ApiError,
        session::session_cookie,
    },
    state::AppState,
};

/// Fallback for unknown `/api/v1/*` paths: JSON, not the web UI.
pub async fn not_found() -> ApiError {
    ApiError::new(StatusCode::NOT_FOUND, "not_found", "No such API endpoint.")
}

/// A known `/api/v1/*` path with the wrong method: JSON like every other API error.
pub async fn method_not_allowed() -> ApiError {
    ApiError::new(
        StatusCode::METHOD_NOT_ALLOWED,
        "method_not_allowed",
        "This endpoint doesn't take that method.",
    )
}

/// Response for any endpoint that signs a user in: sets the session cookie, returns the user.
fn signed_in(
    status: StatusCode,
    outcome: &LoginOutcome,
    state: &AppState,
    client: &ClientContext,
) -> Response {
    let cookie = session_cookie(&outcome.session_token, state.auth.session_ttl(), client.is_https);
    let body = SessionResponse {
        user: UserDto::from(&outcome.user),
    };
    (status, [(header::SET_COOKIE, cookie)], Json(body)).into_response()
}
