//! Creating a server from an archive (`instance.create`): upload it, see what it is, confirm. A
//! modpack then becomes a server in one job; a plain server zip lands in the imports folder, where
//! the usual import takes over.

use axum::{
    Json, Router,
    body::Body,
    extract::{DefaultBodyLimit, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{post, put},
};
use gsd_core::audit::{AuditEntry, actions};
use gsd_mods::modpack::{InstallPack, Uploaded};
use serde::Deserialize;
use serde_json::json;

use crate::{
    http::{
        authorize::{AuthorizedGlobal, perm},
        dto::ApiJson,
        error::ApiError,
    },
    state::AppState,
};

use super::instances::{CreatedResponse, InstanceDto};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/modpacks/upload", put(upload).layer(DefaultBodyLimit::disable()))
        .route("/modpacks/install", post(install))
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
