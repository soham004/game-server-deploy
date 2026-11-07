//! API errors: an HTTP status plus a stable machine-readable `code` the UI can switch on.
//!
//! Wire format: `{"error": {"code": "invalid_credentials", "message": "…"}}`.

use std::fmt::Display;

use axum::{
    Json,
    extract::rejection::JsonRejection,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use gsd_core::auth::AuthError;
use serde_json::json;

#[derive(Debug)]
pub struct ApiError {
    status: StatusCode,
    code: &'static str,
    message: String,
}

impl ApiError {
    pub fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
        }
    }

    pub fn unauthenticated() -> Self {
        Self::new(
            StatusCode::UNAUTHORIZED,
            "unauthenticated",
            "Sign in to continue.",
        )
    }

    pub fn not_found() -> Self {
        Self::new(StatusCode::NOT_FOUND, "not_found", "Not found.")
    }

    pub fn forbidden(code: &'static str, message: impl Into<String>) -> Self {
        Self::new(StatusCode::FORBIDDEN, code, message)
    }

    /// A server-side failure: the details are logged, the client gets a generic message.
    pub fn internal(error: &dyn Display) -> Self {
        tracing::error!(%error, "request failed");
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal_error",
            "Something went wrong on the server.",
        )
    }

    pub fn status(&self) -> StatusCode {
        self.status
    }

    pub fn code(&self) -> &'static str {
        self.code
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = json!({ "error": { "code": self.code, "message": self.message } });
        (self.status, Json(body)).into_response()
    }
}

impl From<AuthError> for ApiError {
    fn from(error: AuthError) -> Self {
        let message = error.to_string();
        match error {
            AuthError::InvalidCredentials => {
                Self::new(StatusCode::UNAUTHORIZED, "invalid_credentials", message)
            }
            AuthError::SessionInvalid => Self::unauthenticated(),
            AuthError::AlreadySetUp => Self::new(StatusCode::CONFLICT, "already_set_up", message),
            AuthError::UsernameTaken => Self::new(StatusCode::CONFLICT, "username_taken", message),
            AuthError::InvalidUsername(_) => {
                Self::new(StatusCode::UNPROCESSABLE_ENTITY, "invalid_username", message)
            }
            AuthError::WeakPassword(_) => {
                Self::new(StatusCode::UNPROCESSABLE_ENTITY, "weak_password", message)
            }
            AuthError::UserNotFound => Self::new(StatusCode::NOT_FOUND, "user_not_found", message),
            AuthError::Repo(_)
            | AuthError::Password(_)
            | AuthError::Random(_)
            | AuthError::BackgroundTask => Self::internal(&error),
        }
    }
}

impl From<JsonRejection> for ApiError {
    fn from(rejection: JsonRejection) -> Self {
        Self::new(rejection.status(), "invalid_request_body", rejection.body_text())
    }
}
