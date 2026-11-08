//! A server's mods or plugins: what is installed, searching Modrinth for what fits, planning and
//! installing (with dependencies), turning files on and off, removing them, and keeping them up
//! to date (checking, naming hand-added files, updating after a full backup).
//!
//! Anyone who can see the server sees its content and may search; `content.manage` changes it.

use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{delete, get, post},
};
use gsd_core::{
    audit::{AuditEntry, actions},
    ids::{InstanceId, JobId},
};
use gsd_mods::{
    AvailableUpdate, ContentList, Identified, UpdateCheck,
    model::{ContentVersion, Project, SearchPage, SearchQuery, SearchSort},
    resolver::InstallPlan,
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
    Ok(Json(state.content.search(auth.instance_id, &query).await?))
}

async fn project(
    State(state): State<AppState>,
    _auth: Authorized<perm::InstanceView>,
    Path((_, project_id)): Path<(InstanceId, String)>,
) -> Result<Json<Project>, ApiError> {
    Ok(Json(state.content.project(&project_id).await?))
}

async fn versions(
    State(state): State<AppState>,
    auth: Authorized<perm::InstanceView>,
    Path((_, project_id)): Path<(InstanceId, String)>,
) -> Result<Json<Vec<ContentVersion>>, ApiError> {
    Ok(Json(state.content.versions(auth.instance_id, &project_id).await?))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct InstallBody {
    project_id: String,
    /// A specific version; the best fitting one when absent.
    #[serde(default)]
    version_id: Option<String>,
}

async fn plan(
    State(state): State<AppState>,
    auth: Authorized<perm::ContentManage>,
    ApiJson(body): ApiJson<InstallBody>,
) -> Result<Json<InstallPlan>, ApiError> {
    Ok(Json(
        state
            .content
            .plan(auth.instance_id, &body.project_id, body.version_id.as_deref())
            .await?,
    ))
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
    let (plan, job_id) = state
        .content
        .start_install(auth.instance_id, &body.project_id, body.version_id.as_deref())
        .await?;
    let files: Vec<&str> = plan
        .steps
        .iter()
        .map(|step| step.version.file.filename.as_str())
        .collect();
    let entry = AuditEntry::on_instance(
        auth.actor.id(),
        auth.instance_id,
        actions::CONTENT_INSTALL_REQUESTED,
    )
    .with_details(json!({ "project": body.project_id, "files": files }));
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
