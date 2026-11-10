//! API errors: an HTTP status plus a stable machine-readable `code` the UI can switch on.
//!
//! Wire format: `{"error": {"code": "invalid_credentials", "message": "…"}}`, plus `"details"`
//! where there is more to say (e.g. which fields were invalid).

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
    details: Option<serde_json::Value>,
}

impl ApiError {
    pub fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
            details: None,
        }
    }

    /// Adds machine-readable details (serialised as `error.details`).
    #[must_use]
    pub fn with_details(mut self, details: impl serde::Serialize) -> Self {
        self.details = serde_json::to_value(details).ok();
        self
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
        let mut error = json!({ "code": self.code, "message": self.message });
        if let Some(details) = self.details {
            error["details"] = details;
        }
        let body = json!({ "error": error });
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
            AuthError::OwnAccount => Self::new(StatusCode::CONFLICT, "own_account", message),
            AuthError::LastOwner => Self::new(StatusCode::CONFLICT, "last_owner", message),
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
            InstanceError::SandboxUnusable(_) => {
                Self::new(StatusCode::SERVICE_UNAVAILABLE, "sandbox_unusable", message)
            }
            InstanceError::Settings(_) => {
                Self::new(StatusCode::UNPROCESSABLE_ENTITY, "invalid_settings", message)
            }
            InstanceError::UnknownRuntime(_) => {
                Self::new(StatusCode::UNPROCESSABLE_ENTITY, "unknown_runtime", message)
            }
            InstanceError::RuntimeTooOld { .. } => {
                Self::new(StatusCode::UNPROCESSABLE_ENTITY, "runtime_too_old", message)
            }
            InstanceError::CustomRuntimeNotAllowed => Self::forbidden("custom_runtime_not_allowed", message),
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
            CreateError::SourceNotInstalled | CreateError::NotInstalled => {
                Self::new(StatusCode::CONFLICT, "not_installed", message)
            }
            CreateError::VersionUnchanged(_) => invalid("version_unchanged"),
            CreateError::DowngradeNotConfirmed { .. } => {
                Self::new(StatusCode::CONFLICT, "downgrade_not_confirmed", message)
            }
            CreateError::ImportNotFound(_) => Self::new(StatusCode::NOT_FOUND, "import_not_found", message),
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
            CatalogError::LoaderUnavailable { .. } => {
                Self::new(StatusCode::UNPROCESSABLE_ENTITY, "loader_unavailable", message)
            }
            CatalogError::UnknownLoaderVersion { .. } => Self::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                "unknown_loader_version",
                message,
            ),
            CatalogError::BrokenBuild { .. } => {
                Self::new(StatusCode::UNPROCESSABLE_ENTITY, "broken_build", message)
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

impl From<gsd_minecraft::properties::PropertiesError> for ApiError {
    fn from(error: gsd_minecraft::properties::PropertiesError) -> Self {
        use gsd_minecraft::properties::PropertiesError;
        let message = error.to_string();
        match error {
            PropertiesError::Conflict => Self::new(StatusCode::CONFLICT, "file_changed", message),
            PropertiesError::Invalid(fields) => {
                Self::new(StatusCode::UNPROCESSABLE_ENTITY, "invalid_properties", message)
                    .with_details(fields)
            }
            PropertiesError::TooLarge => Self::new(StatusCode::PAYLOAD_TOO_LARGE, "file_too_large", message),
            PropertiesError::Io { .. } => Self::internal(&error),
        }
    }
}

impl From<gsd_core::files::FilesError> for ApiError {
    fn from(error: gsd_core::files::FilesError) -> Self {
        use gsd_core::files::FilesError;
        let message = error.to_string();
        let status = |status, code| Self::new(status, code, message.clone());
        match error {
            FilesError::Path(_) => status(StatusCode::UNPROCESSABLE_ENTITY, "invalid_path"),
            FilesError::NotFound(_) => status(StatusCode::NOT_FOUND, "file_not_found"),
            FilesError::AlreadyExists(_) => status(StatusCode::CONFLICT, "file_exists"),
            FilesError::NotADirectory(_) => status(StatusCode::UNPROCESSABLE_ENTITY, "not_a_folder"),
            FilesError::IsADirectory(_) => status(StatusCode::UNPROCESSABLE_ENTITY, "is_a_folder"),
            FilesError::NotText(_) => status(StatusCode::UNPROCESSABLE_ENTITY, "not_text"),
            FilesError::TooLarge { .. } => status(StatusCode::PAYLOAD_TOO_LARGE, "file_too_large"),
            FilesError::Conflict(_) => status(StatusCode::CONFLICT, "file_changed"),
            FilesError::InvalidOperation(_) => status(StatusCode::UNPROCESSABLE_ENTITY, "invalid_operation"),
            FilesError::Forbidden(_) => status(StatusCode::FORBIDDEN, "file_forbidden"),
            FilesError::BadArchive(..) => status(StatusCode::UNPROCESSABLE_ENTITY, "bad_archive"),
            FilesError::Io { .. } => Self::internal(&error),
        }
    }
}

impl From<gsd_core::backups::BackupError> for ApiError {
    fn from(error: gsd_core::backups::BackupError) -> Self {
        use gsd_core::backups::BackupError;
        let message = error.to_string();
        let status = |status, code| Self::new(status, code, message.clone());
        match error {
            BackupError::NotFound => status(StatusCode::NOT_FOUND, "backup_not_found"),
            BackupError::FileMissing => status(StatusCode::NOT_FOUND, "backup_file_missing"),
            BackupError::Instance(error) => error.into(),
            BackupError::Job(error) => error.into(),
            BackupError::ServerBusy(_) => status(StatusCode::CONFLICT, "server_busy"),
            BackupError::NoSpace { .. } => status(StatusCode::INSUFFICIENT_STORAGE, "insufficient_storage"),
            BackupError::InvalidNote => status(StatusCode::UNPROCESSABLE_ENTITY, "invalid_note"),
            BackupError::Damaged(_) => status(StatusCode::UNPROCESSABLE_ENTITY, "backup_damaged"),
            // These happen inside the backup and restore jobs, which report them themselves.
            BackupError::Quiesce(_)
            | BackupError::ServerChanged
            | BackupError::Io { .. }
            | BackupError::Unpack(_)
            | BackupError::Repo(_) => Self::internal(&error),
        }
    }
}

impl From<gsd_core::scheduler::ScheduleError> for ApiError {
    fn from(error: gsd_core::scheduler::ScheduleError) -> Self {
        use gsd_core::scheduler::ScheduleError;
        let message = error.to_string();
        let status = |status, code| Self::new(status, code, message.clone());
        match error {
            ScheduleError::NotFound => status(StatusCode::NOT_FOUND, "schedule_not_found"),
            ScheduleError::Plan(_) | ScheduleError::Trigger(_) => {
                status(StatusCode::UNPROCESSABLE_ENTITY, "invalid_schedule")
            }
            ScheduleError::MissingPermission(_) => Self::forbidden("missing_permission", message.clone()),
            ScheduleError::Instance(error) => error.into(),
            ScheduleError::Repo(_) => Self::internal(&error),
        }
    }
}

impl From<gsd_mods::ContentError> for ApiError {
    fn from(error: gsd_mods::ContentError) -> Self {
        use gsd_mods::{ContentError, resolver::ResolveError};
        let message = error.to_string();
        let status = |status, code| Self::new(status, code, message.clone());
        match error {
            ContentError::NoContent => status(StatusCode::CONFLICT, "no_content_for_loader"),
            ContentError::NotInstalled(_) => status(StatusCode::NOT_FOUND, "content_not_installed"),
            ContentError::NotAJar(_) => status(StatusCode::UNPROCESSABLE_ENTITY, "not_a_jar"),
            ContentError::Resolve(ResolveError::Source(error)) | ContentError::Source(error) => error.into(),
            ContentError::Resolve(_) => status(StatusCode::CONFLICT, "cannot_install"),
            ContentError::Files(error) => error.into(),
            ContentError::Instance(error) => error.into(),
            ContentError::Job(error) => error.into(),
            ContentError::NothingToUpdate => status(StatusCode::CONFLICT, "up_to_date"),
            ContentError::Backup(error) => error.into(),
            ContentError::NoChecksum(_)
            | ContentError::Cancelled
            | ContentError::Cache { .. }
            | ContentError::Download(_) => Self::internal(&error),
        }
    }
}

impl From<gsd_mods::modpack::ModpackError> for ApiError {
    fn from(error: gsd_mods::modpack::ModpackError) -> Self {
        use gsd_mods::modpack::ModpackError;
        let message = error.to_string();
        let status = |status, code| Self::new(status, code, message.clone());
        match error {
            ModpackError::TooBig => status(StatusCode::PAYLOAD_TOO_LARGE, "upload_too_big"),
            ModpackError::UnknownUpload => status(StatusCode::NOT_FOUND, "upload_not_found"),
            ModpackError::NoPacksThere => status(StatusCode::UNPROCESSABLE_ENTITY, "source_has_no_packs"),
            ModpackError::ByHand { .. } => status(StatusCode::CONFLICT, "modpack_by_hand"),
            ModpackError::NotAPackServer => status(StatusCode::NOT_FOUND, "not_a_modpack_server"),
            ModpackError::NoLock => status(StatusCode::CONFLICT, "pack_update_unavailable"),
            ModpackError::NoSourceToAsk => status(StatusCode::CONFLICT, "pack_has_no_source"),
            ModpackError::ChangesLoader { .. } => {
                status(StatusCode::UNPROCESSABLE_ENTITY, "pack_changes_loader")
            }
            ModpackError::Pack(_) => status(StatusCode::UNPROCESSABLE_ENTITY, "invalid_modpack"),
            ModpackError::Create(error) => error.into(),
            ModpackError::Content(error) => error.into(),
            ModpackError::Io(_) => Self::internal(&error),
        }
    }
}

impl From<gsd_minecraft::logs::LogError> for ApiError {
    fn from(error: gsd_minecraft::logs::LogError) -> Self {
        use gsd_minecraft::logs::LogError;
        let message = error.to_string();
        match error {
            LogError::NotFound => Self::new(StatusCode::NOT_FOUND, "log_not_found", message),
            LogError::BadSearch(_) => Self::new(StatusCode::UNPROCESSABLE_ENTITY, "bad_log_search", message),
            LogError::Files(error) => error.into(),
            LogError::Io(_) => Self::internal(&error),
        }
    }
}

impl From<gsd_minecraft::players::PlayersError> for ApiError {
    fn from(error: gsd_minecraft::players::PlayersError) -> Self {
        use gsd_minecraft::players::PlayersError;
        let message = error.to_string();
        let conflict = |code| Self::new(StatusCode::CONFLICT, code, message.clone());
        let invalid = |code| Self::new(StatusCode::UNPROCESSABLE_ENTITY, code, message.clone());
        match error {
            PlayersError::Invalid(_) => invalid("invalid_player_action"),
            PlayersError::Busy => conflict("instance_busy"),
            PlayersError::NotRunning => conflict("not_running"),
            PlayersError::NotOnline(_) => conflict("player_not_online"),
            PlayersError::NeedsManagement(_) => conflict("needs_management_protocol"),
            PlayersError::LevelNotAllowed => Self::forbidden("missing_permission", message),
            PlayersError::NoAccount(_) => Self::new(StatusCode::NOT_FOUND, "no_such_account", message),
            PlayersError::Refused(_) => invalid("refused_by_server"),
            PlayersError::NotConfirmed => Self::new(StatusCode::BAD_GATEWAY, "not_confirmed", message),
            PlayersError::Mojang(_) => {
                tracing::warn!(%error, "Mojang lookup failed");
                Self::new(StatusCode::BAD_GATEWAY, "mojang_unavailable", message)
            }
            PlayersError::Control(_) => {
                tracing::warn!(%error, "control channel failed");
                Self::new(StatusCode::BAD_GATEWAY, "control_unavailable", message)
            }
            PlayersError::Files(error) => error.into(),
            PlayersError::Instance(error) => error.into(),
            PlayersError::History(_) => Self::internal(&error),
        }
    }
}

impl From<gsd_minecraft::gamerules::GameruleError> for ApiError {
    fn from(error: gsd_minecraft::gamerules::GameruleError) -> Self {
        use gsd_minecraft::gamerules::GameruleError;
        let message = error.to_string();
        let invalid = |code| Self::new(StatusCode::UNPROCESSABLE_ENTITY, code, message.clone());
        match error {
            GameruleError::Busy => Self::new(StatusCode::CONFLICT, "instance_busy", message),
            GameruleError::NoWorld => Self::new(StatusCode::CONFLICT, "no_world", message),
            GameruleError::UnknownRule(_) => invalid("unknown_gamerule"),
            GameruleError::WrongType { .. } => invalid("wrong_gamerule_type"),
            GameruleError::NothingToChange => invalid("nothing_to_change"),
            GameruleError::Refused(_) => invalid("refused_by_server"),
            GameruleError::NotConfirmed | GameruleError::Save(_) => {
                Self::new(StatusCode::BAD_GATEWAY, "not_confirmed", message)
            }
            GameruleError::Nbt(_) => {
                tracing::warn!(%error, "a world file can't be read");
                Self::new(StatusCode::UNPROCESSABLE_ENTITY, "unreadable_world", message)
            }
            GameruleError::Control(_) => {
                tracing::warn!(%error, "control channel failed");
                Self::new(StatusCode::BAD_GATEWAY, "control_unavailable", message)
            }
            GameruleError::Files(error) => error.into(),
            GameruleError::Instance(error) => error.into(),
        }
    }
}

impl From<gsd_minecraft::worlds::service::WorldError> for ApiError {
    fn from(error: gsd_minecraft::worlds::service::WorldError) -> Self {
        use gsd_minecraft::worlds::service::WorldError;
        let message = error.to_string();
        let conflict = |code| Self::new(StatusCode::CONFLICT, code, message.clone());
        let invalid = |code| Self::new(StatusCode::UNPROCESSABLE_ENTITY, code, message.clone());
        match error {
            WorldError::NotFound(_) => Self::new(StatusCode::NOT_FOUND, "world_not_found", message),
            WorldError::InvalidName(_) => invalid("invalid_world_name"),
            WorldError::Exists(_) => conflict("world_exists"),
            WorldError::Running => conflict("instance_running"),
            WorldError::Busy => conflict("instance_busy"),
            WorldError::Active => conflict("world_active"),
            WorldError::NotActive => conflict("world_not_active"),
            WorldError::NoLevelDat => invalid("not_a_world"),
            WorldError::BadSeed => invalid("invalid_seed"),
            WorldError::Files(error) => error.into(),
            WorldError::Instance(error) => error.into(),
            WorldError::Job(error) => error.into(),
        }
    }
}

impl From<gsd_minecraft::commands::CommandsError> for ApiError {
    fn from(error: gsd_minecraft::commands::CommandsError) -> Self {
        use gsd_minecraft::commands::CommandsError;
        match error {
            CommandsError::Instance(error) => error.into(),
            CommandsError::Files(error) => error.into(),
            other => {
                tracing::warn!(error = %other, "autocomplete failed");
                Self::internal(&other)
            }
        }
    }
}

impl From<gsd_mods::SourceError> for ApiError {
    fn from(error: gsd_mods::SourceError) -> Self {
        use gsd_mods::SourceError;
        let message = error.to_string();
        match error {
            SourceError::NotFound => Self::new(StatusCode::NOT_FOUND, "project_not_found", message),
            SourceError::RateLimited => {
                Self::new(StatusCode::TOO_MANY_REQUESTS, "source_rate_limited", message)
            }
            SourceError::NoKey => Self::new(StatusCode::CONFLICT, "source_key_missing", message),
            SourceError::KeyRefused => Self::new(StatusCode::CONFLICT, "source_key_refused", message),
            SourceError::Request(_)
            | SourceError::Status { .. }
            | SourceError::Decode { .. }
            | SourceError::ApiRetired => {
                tracing::warn!(%error, "content source failed");
                Self::new(StatusCode::BAD_GATEWAY, "source_unavailable", message)
            }
        }
    }
}
