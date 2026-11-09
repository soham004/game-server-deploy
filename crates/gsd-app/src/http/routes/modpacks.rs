//! Creating a server from a modpack or an archive (`instance.create`): find a pack on a source and
//! fetch it, or upload an archive; see what it is; confirm. A modpack then becomes a server in one
//! job; a plain server zip lands in the imports folder, where the usual import takes over.

use axum::{
    Json, Router,
    body::Body,
    extract::{DefaultBodyLimit, Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post, put},
};
use gsd_core::audit::{AuditEntry, actions};
use gsd_mods::{
    model::{ContentVersion, SearchPage, SearchQuery, SearchSort, SourceKind},
    modpack::{
        InstallPack, Uploaded,
        browse::{FetchPack, Fetching},
    },
};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{
    http::{
        authorize::{AuthorizedGlobal, perm},
        dto::ApiJson,
        error::ApiError,
    },
    state::AppState,
};

use super::instances::{CreatedResponse, InstanceDto};

/// Results per page.
const PAGE_SIZE: u32 = 20;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/modpacks/sources", get(sources))
        .route("/modpacks/search", get(search))
        .route("/modpacks/projects/{project_id}/versions", get(versions))
        .route("/modpacks/fetch", post(fetch))
        .route("/modpacks/uploads/{upload_id}", get(fetched))
        .route("/modpacks/upload", put(upload).layer(DefaultBodyLimit::disable()))
        .route("/modpacks/install", post(install))
}

/// Where packs can be searched now.
async fn sources(
    State(state): State<AppState>,
    _auth: AuthorizedGlobal<perm::InstanceCreate>,
) -> Json<Value> {
    Json(json!({ "sources": state.content.pack_sources() }))
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
    /// Only packs for this Minecraft version.
    #[serde(default)]
    mc: Option<String>,
}

async fn search(
    State(state): State<AppState>,
    _auth: AuthorizedGlobal<perm::InstanceCreate>,
    Query(params): Query<SearchParams>,
) -> Result<Json<SearchPage>, ApiError> {
    let query = SearchQuery {
        text: params.q,
        sort: params.sort,
        offset: params.offset.min(10_000),
        limit: PAGE_SIZE,
    };
    let mc_version = params.mc.as_deref().filter(|version| !version.is_empty());
    Ok(Json(
        state
            .content
            .search_modpacks(params.source, &query, mc_version)
            .await?,
    ))
}

#[derive(Debug, Deserialize)]
struct VersionsParams {
    #[serde(default)]
    source: SourceKind,
}

async fn versions(
    State(state): State<AppState>,
    _auth: AuthorizedGlobal<perm::InstanceCreate>,
    Path(project_id): Path<String>,
    Query(params): Query<VersionsParams>,
) -> Result<Json<Vec<ContentVersion>>, ApiError> {
    Ok(Json(
        state.content.modpack_versions(params.source, &project_id).await?,
    ))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FetchBody {
    source: SourceKind,
    project_id: String,
    #[serde(default)]
    version_id: Option<String>,
}

/// 202: the returned job downloads it; then `GET /modpacks/uploads/{upload_id}` says what it is.
async fn fetch(
    State(state): State<AppState>,
    _auth: AuthorizedGlobal<perm::InstanceCreate>,
    ApiJson(body): ApiJson<FetchBody>,
) -> Result<(StatusCode, Json<Fetching>), ApiError> {
    let request = FetchPack {
        source: body.source,
        project_id: body.project_id,
        version_id: body.version_id,
    };
    let fetching = state
        .content
        .fetch_modpack(request, state.config.imports_dir())
        .await?;
    Ok((StatusCode::ACCEPTED, Json(fetching)))
}

async fn fetched(
    State(state): State<AppState>,
    _auth: AuthorizedGlobal<perm::InstanceCreate>,
    Path(upload_id): Path<String>,
) -> Result<Json<Uploaded>, ApiError> {
    Ok(Json(state.content.fetched_modpack(&upload_id).await?))
}

#[derive(Debug, Deserialize)]
struct UploadQuery {
    file: String,
}

/// The request body is the archive (streamed to disk, never held in memory).
async fn upload(
    State(state): State<AppState>,
    _auth: AuthorizedGlobal<perm::InstanceCreate>,
    Query(query): Query<UploadQuery>,
    body: Body,
) -> Result<(StatusCode, Json<Uploaded>), ApiError> {
    let uploaded = state
        .content
        .upload_modpack(&query.file, body.into_data_stream(), &state.config.imports_dir())
        .await?;
    Ok((StatusCode::CREATED, Json(uploaded)))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct InstallBody {
    upload_id: String,
    name: String,
    max_memory_mb: u32,
    #[serde(default)]
    eula_accepted: bool,
}

/// 202: the instance exists; the returned job installs the server and then the pack.
async fn install(
    State(state): State<AppState>,
    auth: AuthorizedGlobal<perm::InstanceCreate>,
    ApiJson(body): ApiJson<InstallBody>,
) -> Result<Response, ApiError> {
    let request = InstallPack {
        upload_id: body.upload_id,
        name: body.name,
        max_memory_mb: body.max_memory_mb,
        eula_accepted: body.eula_accepted,
    };
    let (instance, job_id) = state.content.install_modpack(request, &state.minecraft).await?;
    let details = json!({ "name": instance.definition.name, "from": "modpack" });
    state
        .audit
        .record(
            AuditEntry::on_instance(auth.actor.id(), instance.id(), actions::INSTANCE_CREATED)
                .with_details(details),
        )
        .await;
    let body = CreatedResponse {
        instance: InstanceDto::new(&state, &instance, &auth.actor),
        job_id,
    };
    Ok((StatusCode::ACCEPTED, Json(body)).into_response())
}
