//! A server's log history and crash reports (`logs.read`). Only files that are logs can be reached,
//! by their plain name; `files.read` is a separate, bigger right.

use axum::{
    Json, Router,
    body::Body,
    extract::{Path, Query, State},
    response::Response,
    routing::get,
};
use gsd_core::{files::ServerFiles, ids::InstanceId};
use gsd_minecraft::logs::{self, LogFile, LogPage, LogQuery, MAX_PAGE};
use serde::Deserialize;

use super::files::attachment;
use crate::{
    http::{
        authorize::{Authorized, perm},
        error::ApiError,
    },
    state::AppState,
};

/// Lines per page unless asked otherwise.
const DEFAULT_PAGE: usize = 500;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/instances/{instance_id}/logs", get(list))
        .route("/instances/{instance_id}/logs/{name}", get(read))
        .route("/instances/{instance_id}/logs/{name}/download", get(download))
}

async fn server_files(state: &AppState, id: InstanceId) -> Result<ServerFiles, ApiError> {
    let instance = state.instances.get(id).await?;
    Ok(ServerFiles::open(&instance).await?)
}

async fn list(
    State(state): State<AppState>,
    auth: Authorized<perm::LogsRead>,
) -> Result<Json<Vec<LogFile>>, ApiError> {
    let files = server_files(&state, auth.instance_id).await?;
    Ok(Json(logs::list(&files).await?))
}

#[derive(Debug, Deserialize)]
struct ReadParams {
    #[serde(default)]
    before: Option<u64>,
    #[serde(default)]
    limit: Option<usize>,
    #[serde(default)]
    q: Option<String>,
    #[serde(default)]
    regex: bool,
    #[serde(default)]
    level: Option<String>,
}

/// The last lines that match, in file order; `earlier` asks for the page before.
async fn read(
    State(state): State<AppState>,
    auth: Authorized<perm::LogsRead>,
    Path((_, name)): Path<(InstanceId, String)>,
    Query(params): Query<ReadParams>,
) -> Result<Json<LogPage>, ApiError> {
    let files = server_files(&state, auth.instance_id).await?;
    let log = logs::find(&files, &name).await?;
    let query = LogQuery {
        before: params.before,
        limit: params.limit.unwrap_or(DEFAULT_PAGE).min(MAX_PAGE),
        search: params.q,
        regex: params.regex,
        level: params.level,
    };
    Ok(Json(logs::read(&files, &log, query).await?))
}

/// The file as it is, compressed if it is.
async fn download(
    State(state): State<AppState>,
    auth: Authorized<perm::LogsRead>,
    Path((_, name)): Path<(InstanceId, String)>,
) -> Result<Response, ApiError> {
    let files = server_files(&state, auth.instance_id).await?;
    let log = logs::find(&files, &name).await?;
    let (file, _) = files.open_file(log.path()).await?;
    let body = Body::from_stream(tokio_util::io::ReaderStream::new(file));
    Ok(attachment(body, &log.name))
}
