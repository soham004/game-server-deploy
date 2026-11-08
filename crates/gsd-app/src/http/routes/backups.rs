//! An instance's backups: list, back up now, restore, pin or annotate, delete, download.
//!
//! Who may do what: anyone who can see the server sees its backups; `backups.create` backs up
//! and pins; `backups.restore` restores, deletes and downloads (a download holds every world).

use axum::{
    Json, Router,
    body::Body,
    extract::{Path, State},
    http::{HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, patch, post},
};
use chrono::{DateTime, Utc};
use gsd_core::{
    audit::{AuditEntry, actions},
    backups::{BackupChange, BackupKind, BackupRecord, BackupRequest, BackupScope, RestoreOptions},
    ids::{BackupId, InstanceId, JobId},
    instances::BackupPolicy,
};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::{
    http::{
        authorize::{Authorized, perm},
        dto::ApiJson,
        error::ApiError,
        routes::files::attachment,
    },
    state::AppState,
};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/instances/{instance_id}/backups", get(list).post(create))
        .route(
            "/instances/{instance_id}/backups/{backup_id}",
            patch(update).delete(delete),
        )
        .route(
            "/instances/{instance_id}/backups/{backup_id}/restore",
            post(restore),
        )
        .route(
            "/instances/{instance_id}/backups/{backup_id}/download",
            get(download),
        )
}

#[derive(Debug, Serialize)]
struct BackupDto {
    id: BackupId,
    kind: BackupKind,
    scope: BackupScope,
    file_name: String,
    size_bytes: u64,
    /// What the server ran when the backup was taken, e.g. "Paper 26.3".
    software: Option<String>,
    note: Option<String>,
    pinned: bool,
    created_at: DateTime<Utc>,
}

impl From<BackupRecord> for BackupDto {
    fn from(backup: BackupRecord) -> Self {
        Self {
            id: backup.id,
            kind: backup.kind,
            scope: backup.scope,
            file_name: backup.file_name,
            size_bytes: backup.size_bytes,
            software: backup.software,
            note: backup.note,
            pinned: backup.pinned,
            created_at: backup.created_at,
        }
    }
}

#[derive(Debug, Serialize)]
struct BackupList {
    backups: Vec<BackupDto>,
    /// The instance's retention policy.
    policy: BackupPolicy,
    total_bytes: u64,
}

async fn list(
    State(state): State<AppState>,
    auth: Authorized<perm::InstanceView>,
) -> Result<Json<BackupList>, ApiError> {
    let instance = state.instances.get(auth.instance_id).await?;
    let backups = state.backups.list(auth.instance_id).await?;
    Ok(Json(BackupList {
        total_bytes: backups.iter().map(|backup| backup.size_bytes).sum(),
        backups: backups.into_iter().map(BackupDto::from).collect(),
        policy: instance.definition.backups,
    }))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateBody {
    scope: BackupScope,
    #[serde(default)]
    note: Option<String>,
}

#[derive(Debug, Serialize)]
struct JobStarted {
    job_id: JobId,
}

async fn create(
    State(state): State<AppState>,
    auth: Authorized<perm::BackupsCreate>,
    ApiJson(body): ApiJson<CreateBody>,
) -> Result<Response, ApiError> {
    let request = BackupRequest {
        scope: body.scope,
        kind: BackupKind::Manual,
        note: body.note,
        by: Some(auth.actor.id()),
    };
    let job_id = state.backups.start_backup(auth.instance_id, request).await?;
    let entry = AuditEntry::on_instance(auth.actor.id(), auth.instance_id, actions::BACKUP_REQUESTED)
        .with_details(json!({ "scope": body.scope }));
    state.audit.record(entry).await;
    Ok((StatusCode::ACCEPTED, Json(JobStarted { job_id })).into_response())
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RestoreBody {
    /// Back up the current files first (default), so the restore can be undone.
    #[serde(default = "yes")]
    safety_backup: bool,
}

fn yes() -> bool {
    true
}

async fn restore(
    State(state): State<AppState>,
    auth: Authorized<perm::BackupsRestore>,
    Path((_, backup_id)): Path<(InstanceId, BackupId)>,
    ApiJson(body): ApiJson<RestoreBody>,
) -> Result<Response, ApiError> {
    let options = RestoreOptions {
        safety_backup: body.safety_backup,
    };
    let job_id = state
        .backups
        .start_restore(auth.instance_id, backup_id, options, Some(auth.actor.id()))
        .await?;
    let entry = AuditEntry::on_instance(
        auth.actor.id(),
        auth.instance_id,
        actions::BACKUP_RESTORE_REQUESTED,
    )
    .with_details(json!({ "backup": backup_id, "safety_backup": body.safety_backup }));
    state.audit.record(entry).await;
    Ok((StatusCode::ACCEPTED, Json(JobStarted { job_id })).into_response())
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct UpdateBody {
    pinned: Option<bool>,
    /// An empty note removes it.
    note: Option<String>,
}

async fn update(
    State(state): State<AppState>,
    auth: Authorized<perm::BackupsCreate>,
    Path((_, backup_id)): Path<(InstanceId, BackupId)>,
    ApiJson(body): ApiJson<UpdateBody>,
) -> Result<Json<BackupDto>, ApiError> {
    let change = BackupChange {
        pinned: body.pinned,
        note: body.note.map(Some),
    };
    let backup = state.backups.update(auth.instance_id, backup_id, change).await?;
    Ok(Json(backup.into()))
}

async fn delete(
    State(state): State<AppState>,
    auth: Authorized<perm::BackupsRestore>,
    Path((_, backup_id)): Path<(InstanceId, BackupId)>,
) -> Result<StatusCode, ApiError> {
    state.backups.delete(auth.instance_id, backup_id).await?;
    let entry = AuditEntry::on_instance(auth.actor.id(), auth.instance_id, actions::BACKUP_DELETED)
        .with_details(json!({ "backup": backup_id }));
    state.audit.record(entry).await;
    Ok(StatusCode::NO_CONTENT)
}

async fn download(
    State(state): State<AppState>,
    auth: Authorized<perm::BackupsRestore>,
    Path((_, backup_id)): Path<(InstanceId, BackupId)>,
) -> Result<Response, ApiError> {
    let instance = state.instances.get(auth.instance_id).await?;
    let (backup, path) = state.backups.archive(auth.instance_id, backup_id).await?;
    let file = tokio::fs::File::open(&path)
        .await
        .map_err(|error| ApiError::internal(&error))?;
    let body = Body::from_stream(tokio_util::io::ReaderStream::new(file));
    let name = format!("{}-{}", instance.layout.slug(), backup.file_name);
    let mut response = attachment(body, &name);
    response
        .headers_mut()
        .insert(header::CONTENT_LENGTH, HeaderValue::from(backup.size_bytes));
    Ok(response)
}
