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
use gsd_core::{
    auth::AuthError,
    instances::{InstanceError, registry::RegistryError},
    jobs::JobError,
    supervisor::{CommandError, SupervisorError},
};
use gsd_minecraft::{CatalogError, CreateError};
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

impl From<InstanceError> for ApiError {
    fn from(error: InstanceError) -> Self {
        let message = error.to_string();
        let conflict = |code| Self::new(StatusCode::CONFLICT, code, message.clone());
        match error {
            InstanceError::NotFound => {
                Self::new(StatusCode::NOT_FOUND, "instance_not_found", "No such instance.")
            }
            InstanceError::Busy => conflict("instance_busy"),
            InstanceError::JavaMissing { .. } => conflict("java_missing"),
            InstanceError::StillRunning => conflict("instance_running"),
            InstanceError::NotInstalled => conflict("not_installed"),
            InstanceError::Supervisor(error) => error.into(),
            InstanceError::Registry(error) => error.into(),
            InstanceError::UnknownGame(_) => Self::internal(&error),
        }
    }
}

impl From<SupervisorError> for ApiError {
    fn from(error: SupervisorError) -> Self {
        let message = error.to_string();
        match error {
            SupervisorError::Command(CommandError::AlreadyRunning) => {
                Self::new(StatusCode::CONFLICT, "already_running", message)
            }
            SupervisorError::Command(CommandError::NotRunning) | SupervisorError::NotAcceptingInput => {
                Self::new(StatusCode::CONFLICT, "not_running", message)
            }
            SupervisorError::Write(_) => Self::new(StatusCode::BAD_REQUEST, "invalid_command", message),
            SupervisorError::Gone => Self::internal(&error),
        }
    }
}

impl From<RegistryError> for ApiError {
    fn from(error: RegistryError) -> Self {
        let message = error.to_string();
        match error {
            RegistryError::NotFound => {
                Self::new(StatusCode::NOT_FOUND, "instance_not_found", "No such instance.")
            }
            RegistryError::NoFreePort(_) | RegistryError::NoFreeUid => {
                Self::new(StatusCode::CONFLICT, "no_capacity", message)
            }
            RegistryError::Io { .. } | RegistryError::Definition(_) => Self::internal(&error),
        }
    }
}

impl From<CreateError> for ApiError {
    fn from(error: CreateError) -> Self {
        let message = error.to_string();
        let invalid = |code| Self::new(StatusCode::UNPROCESSABLE_ENTITY, code, message.clone());
        match error {
            CreateError::EulaRequired => invalid("eula_required"),
            CreateError::InvalidName => invalid("invalid_name"),
            CreateError::InvalidMemory => invalid("invalid_memory"),
            CreateError::UnknownVersion(_) => invalid("unknown_version"),
            CreateError::LoaderUnavailable { .. } => invalid("loader_unavailable"),
            CreateError::UnknownLoaderVersion { .. } => invalid("unknown_loader_version"),
            CreateError::Catalog(error) => error.into(),
            CreateError::Registry(error) => error.into(),
            CreateError::Job(error) => error.into(),
            CreateError::NotMinecraft(_) => Self::internal(&error),
        }
    }
}

impl From<CatalogError> for ApiError {
    fn from(error: CatalogError) -> Self {
        let message = error.to_string();
        match error {
            CatalogError::UnknownVersion(_) => Self::new(StatusCode::NOT_FOUND, "unknown_version", message),
            CatalogError::NoServerDownload(_) => {
                Self::new(StatusCode::UNPROCESSABLE_ENTITY, "no_server_download", message)
            }
            CatalogError::Unreachable { .. }
            | CatalogError::Status { .. }
            | CatalogError::Malformed { .. } => {
                tracing::warn!(%error, "upstream catalog unavailable");
                Self::new(
                    StatusCode::BAD_GATEWAY,
                    "upstream_unavailable",
                    "A version server could not be reached. Try again shortly.",
                )
            }
        }
    }
}

impl From<JobError> for ApiError {
    fn from(error: JobError) -> Self {
        Self::new(StatusCode::CONFLICT, "instance_busy", error.to_string())
    }
}

impl From<JsonRejection> for ApiError {
    fn from(rejection: JsonRejection) -> Self {
        Self::new(rejection.status(), "invalid_request_body", rejection.body_text())
    }
}
