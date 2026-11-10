//! A server's worlds. Seeing them takes `instance.view` (their seeds `files.read`); making one active
//! `properties.write`; downloading `files.read`; uploading, deleting and resetting `files.write`.

use axum::{
    Json, Router,
    body::Body,
    extract::{DefaultBodyLimit, Path, Query, State},
    http::StatusCode,
    response::Response,
    routing::{get, post},
};
use gsd_core::{
    audit::{AuditEntry, actions},
    auth::Permission,
    ids::InstanceId,
};
use gsd_minecraft::worlds::service::{NewSeed, World, WorldsView};
use serde::Deserialize;
use serde_json::{Value, json};

use super::files::attachment;
use crate::{
    http::{
        authorize::{Authorized, perm},
        dto::ApiJson,
        error::ApiError,
    },
    state::AppState,
};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/instances/{instance_id}/worlds", get(list))
        .route(
            "/instances/{instance_id}/worlds/{folder}",
            axum::routing::put(upload)
                .layer(DefaultBodyLimit::disable())
                .delete(delete),
        )
        .route("/instances/{instance_id}/worlds/{folder}/download", get(download))
        .route(
            "/instances/{instance_id}/worlds/{folder}/activate",
            post(activate),
        )
        .route("/instances/{instance_id}/worlds/{folder}/reset", post(reset))
}

async fn record(state: &AppState, actor: gsd_core::ids::UserId, id: InstanceId, details: Value) {
    let entry = AuditEntry::on_instance(actor, id, actions::WORLDS_CHANGED).with_details(details);
    state.audit.record(entry).await;
}

async fn list(
    State(state): State<AppState>,
    auth: Authorized<perm::InstanceView>,
) -> Result<Json<WorldsView>, ApiError> {
    let mut view = state.worlds.list(auth.instance_id).await?;
    // A seed lets players find what the world hides; it is for those who may read the server's files.
    if !auth.actor.can(Permission::FilesRead, auth.instance_id) {
        for world in &mut view.worlds {
            world.facts.seed = None;
        }
    }
    Ok(Json(view))
}

async fn activate(
    State(state): State<AppState>,
    auth: Authorized<perm::PropertiesWrite>,
    Path((_, folder)): Path<(InstanceId, String)>,
) -> Result<Json<Value>, ApiError> {
    let restart_needed = state.worlds.activate(auth.instance_id, &folder).await?;
    record(
        &state,
        auth.actor.id(),
        auth.instance_id,
        json!({ "action": "activate", "world": folder }),
    )
    .await;
    Ok(Json(json!({ "restart_needed": restart_needed })))
}

async fn download(
    State(state): State<AppState>,
    auth: Authorized<perm::FilesRead>,
    Path((_, folder)): Path<(InstanceId, String)>,
) -> Result<Response, ApiError> {
    let stream = state.worlds.download(auth.instance_id, &folder).await?;
    Ok(attachment(Body::from_stream(stream), &format!("{folder}.zip")))
}

#[derive(Debug, Deserialize)]
struct UploadParams {
    #[serde(default)]
    activate: bool,
}

async fn upload(
    State(state): State<AppState>,
    auth: Authorized<perm::FilesWrite>,
    Path((_, folder)): Path<(InstanceId, String)>,
    Query(params): Query<UploadParams>,
    body: Body,
) -> Result<(StatusCode, Json<World>), ApiError> {
    let world = state
        .worlds
        .upload(
            auth.instance_id,
            &folder,
            body.into_data_stream(),
            params.activate,
        )
        .await?;
    record(
        &state,
        auth.actor.id(),
        auth.instance_id,
        json!({ "action": "upload", "world": folder, "activate": params.activate }),
    )
    .await;
    Ok((StatusCode::CREATED, Json(world)))
}

async fn delete(
    State(state): State<AppState>,
    auth: Authorized<perm::FilesWrite>,
    Path((_, folder)): Path<(InstanceId, String)>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let job = state
        .worlds
        .delete(auth.instance_id, &folder, Some(auth.actor.id()))
        .await?;
    record(
        &state,
        auth.actor.id(),
        auth.instance_id,
        json!({ "action": "delete", "world": folder }),
    )
    .await;
    Ok((StatusCode::ACCEPTED, Json(json!({ "job_id": job }))))
}

#[derive(Debug, Default, Deserialize)]
struct ResetBody {
    /// None keeps `level-seed`; empty makes it random; anything else is the new seed.
    #[serde(default)]
    seed: Option<String>,
}

async fn reset(
    State(state): State<AppState>,
    auth: Authorized<perm::FilesWrite>,
    Path((_, folder)): Path<(InstanceId, String)>,
    ApiJson(body): ApiJson<ResetBody>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let seed = NewSeed::from_request(body.seed)?;
    let job = state
        .worlds
        .reset(auth.instance_id, &folder, seed, Some(auth.actor.id()))
        .await?;
    record(
        &state,
        auth.actor.id(),
        auth.instance_id,
        json!({ "action": "reset", "world": folder }),
    )
    .await;
    Ok((StatusCode::ACCEPTED, Json(json!({ "job_id": job }))))
}
