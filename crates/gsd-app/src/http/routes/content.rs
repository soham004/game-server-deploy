//! A server's mods or plugins: what is installed, searching a source for what fits, planning and
//! installing (with dependencies), turning files on and off, removing them, and keeping them up
//! to date (checking, naming hand-added files, updating after a full backup).
//!
//! Anyone who can see the server sees its content and may search; `content.manage` changes it.

use axum::{
    Json, Router,
    body::Body,
    extract::{DefaultBodyLimit, Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{delete, get, post, put},
};
use gsd_core::{
    audit::{AuditEntry, actions},
    ids::{InstanceId, JobId},
};
use gsd_mods::{
    AvailableUpdate, ContentList, Identified, UpdateCheck,
    model::{ContentVersion, Project, SearchPage, SearchQuery, SearchSort, SourceKind},
    resolver::{InstallPlan, Requested},
};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::{
    http::{
        authorize::{Authorized, perm},
        dto::ApiJson,
        error::ApiError,
    },
    state::AppState,
};

/// Search results per page.
const PAGE_SIZE: u32 = 20;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/instances/{instance_id}/content", get(list))
        .route("/instances/{instance_id}/content/search", get(search))
        .route(
            "/instances/{instance_id}/content/projects/{project_id}",
            get(project),
        )
        .route(
            "/instances/{instance_id}/content/projects/{project_id}/versions",
            get(versions),
        )
        .route("/instances/{instance_id}/content/plan", post(plan))
        .route("/instances/{instance_id}/content/install", post(install))
        .route(
            "/instances/{instance_id}/content/files/{file}/enable",
            post(enable),
        )
        .route(
            "/instances/{instance_id}/content/files/{file}/disable",
            post(disable),
        )
        .route("/instances/{instance_id}/content/files/{file}", delete(remove))
        .route("/instances/{instance_id}/content/updates", get(updates))
        .route("/instances/{instance_id}/content/identify", post(identify))
        .route("/instances/{instance_id}/content/update", post(update))
        .route(
            "/instances/{instance_id}/content/upload",
            put(upload).layer(DefaultBodyLimit::disable()),
        )
}

async fn list(
    State(state): State<AppState>,
    auth: Authorized<perm::InstanceView>,
) -> Result<Json<ContentList>, ApiError> {
    Ok(Json(state.content.list(auth.instance_id).await?))
}

#[derive(Debug, Deserialize)]
struct SearchParams {
    #[serde(default)]
    source: SourceKind,
    #[serde(default)]
    q: String,
    #[serde(default)]
    sort: SearchSort,
    #[serde(default)]
    offset: u32,
}

async fn search(
    State(state): State<AppState>,
    auth: Authorized<perm::InstanceView>,
    Query(params): Query<SearchParams>,
) -> Result<Json<SearchPage>, ApiError> {
    let query = SearchQuery {
        text: params.q.trim().chars().take(100).collect(),
        sort: params.sort,
        offset: params.offset.min(10_000),
        limit: PAGE_SIZE,
    };
    Ok(Json(
        state
            .content
            .search(auth.instance_id, params.source, &query)
            .await?,
    ))
}

/// Which source a project id belongs to (ids mean nothing on another).
#[derive(Debug, Deserialize)]
struct SourceParam {
    #[serde(default)]
    source: SourceKind,
}

async fn project(
    State(state): State<AppState>,
    _auth: Authorized<perm::InstanceView>,
    Path((_, project_id)): Path<(InstanceId, String)>,
    Query(params): Query<SourceParam>,
) -> Result<Json<Project>, ApiError> {
    Ok(Json(state.content.project(params.source, &project_id).await?))
}

async fn versions(
    State(state): State<AppState>,
    auth: Authorized<perm::InstanceView>,
    Path((_, project_id)): Path<(InstanceId, String)>,
    Query(params): Query<SourceParam>,
) -> Result<Json<Vec<ContentVersion>>, ApiError> {
    Ok(Json(
        state
            .content
            .versions(auth.instance_id, params.source, &project_id)
            .await?,
    ))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct InstallBody {
    /// One project (from Modrinth unless `source` says otherwise)…
    #[serde(default)]
    source: Option<SourceKind>,
    #[serde(default)]
    project_id: Option<String>,
    /// …at a specific version (the best fitting one when absent).
    #[serde(default)]
    version_id: Option<String>,
    /// …or several at once, each with its own source and an optional version.
    #[serde(default)]
    projects: Option<Vec<Requested>>,
}

/// The most projects one install may ask for (each brings its dependencies too).
const MAX_REQUESTED: usize = 32;

impl InstallBody {
    /// What to install: one project or the list, never both.
    fn requested(self) -> Result<Vec<Requested>, ApiError> {
        let requested = match (self.source, self.project_id, self.version_id, self.projects) {
            (source, Some(project_id), version_id, None) => vec![Requested {
                source: source.unwrap_or_default(),
                project_id,
                version_id,
            }],
            (None, None, None, Some(projects)) => projects,
            _ => {
                return Err(invalid_request(
                    "Send either project_id (with source and version_id if needed) or projects.",
                ));
            }
        };
        if requested.is_empty() || requested.len() > MAX_REQUESTED {
            return Err(invalid_request(format!(
                "Install 1 to {MAX_REQUESTED} projects at a time."
            )));
        }
        Ok(requested)
    }
}

fn invalid_request(message: impl Into<String>) -> ApiError {
    ApiError::new(
        StatusCode::UNPROCESSABLE_ENTITY,
        "invalid_install_request",
        message,
    )
}

async fn plan(
    State(state): State<AppState>,
    auth: Authorized<perm::ContentManage>,
    ApiJson(body): ApiJson<InstallBody>,
) -> Result<Json<InstallPlan>, ApiError> {
    let requested = body.requested()?;
    Ok(Json(state.content.plan(auth.instance_id, &requested).await?))
}

#[derive(Debug, Serialize)]
struct InstallStarted {
    job_id: JobId,
    plan: InstallPlan,
}

async fn install(
    State(state): State<AppState>,
    auth: Authorized<perm::ContentManage>,
    ApiJson(body): ApiJson<InstallBody>,
) -> Result<Response, ApiError> {
    let requested = body.requested()?;
    let (plan, job_id) = state.content.start_install(auth.instance_id, &requested).await?;
    let files: Vec<&str> = plan
        .steps
        .iter()
        .map(|step| step.version.file.filename.as_str())
        .collect();
    let projects: Vec<String> = requested
        .iter()
        .map(|request| format!("{}:{}", request.source.as_str(), request.project_id))
        .collect();
    let entry = AuditEntry::on_instance(
        auth.actor.id(),
        auth.instance_id,
        actions::CONTENT_INSTALL_REQUESTED,
    )
    .with_details(json!({ "projects": projects, "files": files }));
    state.audit.record(entry).await;
    Ok((StatusCode::ACCEPTED, Json(InstallStarted { job_id, plan })).into_response())
}

async fn enable(
    State(state): State<AppState>,
    auth: Authorized<perm::ContentManage>,
    Path((_, file)): Path<(InstanceId, String)>,
) -> Result<StatusCode, ApiError> {
    toggle(&state, &auth, &file, true).await
}

async fn disable(
    State(state): State<AppState>,
    auth: Authorized<perm::ContentManage>,
    Path((_, file)): Path<(InstanceId, String)>,
) -> Result<StatusCode, ApiError> {
    toggle(&state, &auth, &file, false).await
}

async fn toggle(
    state: &AppState,
    auth: &Authorized<perm::ContentManage>,
    file: &str,
    enabled: bool,
) -> Result<StatusCode, ApiError> {
    state.content.set_enabled(auth.instance_id, file, enabled).await?;
    let entry = AuditEntry::on_instance(auth.actor.id(), auth.instance_id, actions::CONTENT_TOGGLED)
        .with_details(json!({ "file": file, "enabled": enabled }));
    state.audit.record(entry).await;
    Ok(StatusCode::NO_CONTENT)
}

async fn remove(
    State(state): State<AppState>,
    auth: Authorized<perm::ContentManage>,
    Path((_, file)): Path<(InstanceId, String)>,
) -> Result<StatusCode, ApiError> {
    state.content.remove(auth.instance_id, &file).await?;
    let entry = AuditEntry::on_instance(auth.actor.id(), auth.instance_id, actions::CONTENT_REMOVED)
        .with_details(json!({ "file": file }));
    state.audit.record(entry).await;
    Ok(StatusCode::NO_CONTENT)
}

async fn updates(
    State(state): State<AppState>,
    auth: Authorized<perm::InstanceView>,
) -> Result<Json<UpdateCheck>, ApiError> {
    Ok(Json(state.content.check_updates(auth.instance_id).await?))
}

/// Names hand-added files by their hash (they are tracked, and can be updated, from then on).
async fn identify(
    State(state): State<AppState>,
    auth: Authorized<perm::ContentManage>,
) -> Result<Json<Identified>, ApiError> {
    let identified = state.content.identify(auth.instance_id).await?;
    if !identified.identified.is_empty() {
        let entry = AuditEntry::on_instance(auth.actor.id(), auth.instance_id, actions::CONTENT_IDENTIFIED)
            .with_details(json!({ "projects": identified.identified }));
        state.audit.record(entry).await;
    }
    Ok(Json(identified))
}

#[derive(Debug, Deserialize)]
struct UploadQuery {
    file: String,
}

/// A jar the user brings (one `CurseForge` can't hand over, or any other): the request body is the
/// file. It is looked up right away, so it is tracked if a source knows it.
async fn upload(
    State(state): State<AppState>,
    auth: Authorized<perm::ContentManage>,
    Query(query): Query<UploadQuery>,
    body: Body,
) -> Result<(StatusCode, Json<Identified>), ApiError> {
    let identified = state
        .content
        .upload(auth.instance_id, &query.file, body.into_data_stream())
        .await?;
    let entry = AuditEntry::on_instance(auth.actor.id(), auth.instance_id, actions::CONTENT_UPLOADED)
        .with_details(json!({ "file": query.file, "identified": identified.identified }));
    state.audit.record(entry).await;
    Ok((StatusCode::CREATED, Json(identified)))
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct UpdateBody {
    /// The files to update; every available update when absent.
    #[serde(default)]
    files: Option<Vec<String>>,
}

#[derive(Debug, Serialize)]
struct UpdateStarted {
    job_id: JobId,
    updates: Vec<AvailableUpdate>,
}

async fn update(
    State(state): State<AppState>,
    auth: Authorized<perm::ContentManage>,
    ApiJson(body): ApiJson<UpdateBody>,
) -> Result<Response, ApiError> {
    let (updates, job_id) = state
        .content
        .start_update(
            auth.instance_id,
            body.files.as_deref(),
            &state.backups,
            Some(auth.actor.id()),
        )
        .await?;
    let files: Vec<&str> = updates.iter().map(|update| update.file.as_str()).collect();
    let entry = AuditEntry::on_instance(
        auth.actor.id(),
        auth.instance_id,
        actions::CONTENT_UPDATE_REQUESTED,
    )
    .with_details(json!({ "files": files }));
    state.audit.record(entry).await;
    Ok((StatusCode::ACCEPTED, Json(UpdateStarted { job_id, updates })).into_response())
}
