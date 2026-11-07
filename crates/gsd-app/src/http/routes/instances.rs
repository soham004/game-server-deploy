//! Server instances: list, create, inspect, delete, lifecycle, console.

use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use chrono::{DateTime, Utc};
use gsd_core::{
    audit::{AuditEntry, actions},
    auth::{Actor, Permission, PermissionSet},
    ids::{InstanceId, JobId},
    instances::Instance,
    supervisor::{ServerState, Snapshot},
};
use gsd_minecraft::{CreateRequest, LoaderKind};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::{
    http::{
        authorize::{Authorized, AuthorizedGlobal, perm},
        dto::ApiJson,
        error::ApiError,
        session::CurrentActor,
    },
    state::AppState,
};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/instances", get(list).post(create))
        .route("/instances/{instance_id}", get(show).delete(delete))
        .route("/instances/{instance_id}/start", post(start))
        .route("/instances/{instance_id}/stop", post(stop))
        .route("/instances/{instance_id}/restart", post(restart))
        .route("/instances/{instance_id}/kill", post(kill))
        .route("/instances/{instance_id}/command", post(command))
        .route("/instances/{instance_id}/console", get(console))
        .route("/instances/{instance_id}/install", post(reinstall))
}

/// An instance as the UI sees it.
#[derive(Debug, Serialize)]
pub struct InstanceDto {
    id: InstanceId,
    name: String,
    game: String,
    slug: String,
    created_at: DateTime<Utc>,
    installed: bool,
    /// A job (install, …) is running on it.
    busy: bool,
    state: ServerState,
    port: u16,
    /// Game-specific settings (Minecraft version, loader, …).
    server: serde_json::Value,
    java_major: u32,
    max_memory_mb: u32,
    /// What the current user may do on it.
    permissions: PermissionSet,
}

impl InstanceDto {
    fn new(state: &AppState, instance: &Instance, actor: &Actor) -> Self {
        let definition = &instance.definition;
        Self {
            id: definition.id,
            name: definition.name.clone(),
            game: definition.game.clone(),
            slug: instance.layout.slug(),
            created_at: definition.created_at,
            installed: definition.installed_at.is_some(),
            busy: state.instances.is_busy(definition.id),
            state: state.instances.state(definition.id),
            port: definition.ports.game,
            server: serde_json::to_value(&definition.server).unwrap_or_default(),
            java_major: definition.java.major,
            max_memory_mb: definition.java.max_memory_mb,
            permissions: actor.permissions_for(definition.id),
        }
    }
}

async fn list(State(state): State<AppState>, CurrentActor(actor): CurrentActor) -> Json<Vec<InstanceDto>> {
    let visible = state
        .instances
        .registry()
        .list()
        .await
        .into_iter()
        .filter(|instance| actor.can(Permission::InstanceView, instance.id()))
        .map(|instance| InstanceDto::new(&state, &instance, &actor))
        .collect();
    Json(visible)
}

#[derive(Debug, Deserialize)]
struct CreateBody {
    name: String,
    mc_version: String,
    loader: LoaderKind,
    #[serde(default)]
    loader_version: Option<String>,
    max_memory_mb: u32,
    #[serde(default)]
    eula_accepted: bool,
}

#[derive(Debug, Serialize)]
struct CreatedResponse {
    instance: InstanceDto,
    job_id: JobId,
}

/// 202: the instance exists; its files are installed by the returned job.
async fn create(
    State(state): State<AppState>,
    auth: AuthorizedGlobal<perm::InstanceCreate>,
    ApiJson(body): ApiJson<CreateBody>,
) -> Result<Response, ApiError> {
    let request = CreateRequest {
        name: body.name,
        mc_version: body.mc_version,
        loader: body.loader,
        loader_version: body.loader_version.filter(|version| !version.trim().is_empty()),
        max_memory_mb: body.max_memory_mb,
        eula_accepted: body.eula_accepted,
    };
    let (instance, job_id) = state.minecraft.create(request).await?;
    let details = json!({ "name": instance.definition.name, "server": instance.definition.server });
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

async fn show(
    State(state): State<AppState>,
    auth: Authorized<perm::InstanceView>,
) -> Result<Json<InstanceDto>, ApiError> {
    let instance = state.instances.get(auth.instance_id).await?;
    Ok(Json(InstanceDto::new(&state, &instance, &auth.actor)))
}

async fn delete(
    State(state): State<AppState>,
    auth: AuthorizedGlobal<perm::InstanceDelete>,
    Path(instance_id): Path<InstanceId>,
) -> Result<StatusCode, ApiError> {
    let instance = state.instances.get(instance_id).await?;
    state.instances.delete(instance_id).await?;
    let details = json!({ "name": instance.definition.name });
    state
        .audit
        .record(
            AuditEntry::on_instance(auth.actor.id(), instance_id, actions::INSTANCE_DELETED)
                .with_details(details),
        )
        .await;
    Ok(StatusCode::NO_CONTENT)
}

async fn start(
    State(state): State<AppState>,
    auth: Authorized<perm::Lifecycle>,
) -> Result<StatusCode, ApiError> {
    state.instances.start(auth.instance_id).await?;
    audit(&state, &auth.actor, auth.instance_id, actions::INSTANCE_STARTED).await;
    Ok(StatusCode::ACCEPTED)
}

async fn stop(
    State(state): State<AppState>,
    auth: Authorized<perm::Lifecycle>,
) -> Result<StatusCode, ApiError> {
    state.instances.stop(auth.instance_id).await?;
    audit(&state, &auth.actor, auth.instance_id, actions::INSTANCE_STOPPED).await;
    Ok(StatusCode::ACCEPTED)
}

async fn restart(
    State(state): State<AppState>,
    auth: Authorized<perm::Lifecycle>,
) -> Result<StatusCode, ApiError> {
    state.instances.restart(auth.instance_id).await?;
    audit(&state, &auth.actor, auth.instance_id, actions::INSTANCE_RESTARTED).await;
    Ok(StatusCode::ACCEPTED)
}

async fn kill(
    State(state): State<AppState>,
    auth: Authorized<perm::Lifecycle>,
) -> Result<StatusCode, ApiError> {
    state.instances.kill(auth.instance_id).await?;
    audit(&state, &auth.actor, auth.instance_id, actions::INSTANCE_KILLED).await;
    Ok(StatusCode::ACCEPTED)
}

#[derive(Debug, Deserialize)]
struct CommandBody {
    command: String,
}

async fn command(
    State(state): State<AppState>,
    auth: Authorized<perm::ConsoleWrite>,
    ApiJson(body): ApiJson<CommandBody>,
) -> Result<StatusCode, ApiError> {
    let author = auth.actor.user().username.to_string();
    state
        .instances
        .send_command(auth.instance_id, &body.command, Some(author))
        .await?;
    let entry = AuditEntry::on_instance(auth.actor.id(), auth.instance_id, actions::CONSOLE_COMMAND)
        .with_details(json!({ "command": body.command.trim() }));
    state.audit.record(entry).await;
    Ok(StatusCode::NO_CONTENT)
}

/// State, online players, diagnostics and the recent console lines.
async fn console(
    State(state): State<AppState>,
    auth: Authorized<perm::ConsoleRead>,
) -> Result<Json<Snapshot>, ApiError> {
    Ok(Json(state.instances.snapshot(auth.instance_id).await?))
}

#[derive(Debug, Serialize)]
struct JobResponse {
    job_id: JobId,
}

/// Re-runs the install job (e.g. after a failed download).
async fn reinstall(
    State(state): State<AppState>,
    auth: Authorized<perm::InstanceUpdate>,
) -> Result<Response, ApiError> {
    if state.instances.state(auth.instance_id).is_active() {
        return Err(gsd_core::instances::InstanceError::StillRunning.into());
    }
    let instance = state.instances.get(auth.instance_id).await?;
    let job_id = state.minecraft.install(instance)?;
    audit(
        &state,
        &auth.actor,
        auth.instance_id,
        actions::INSTANCE_REINSTALLED,
    )
    .await;
    Ok((StatusCode::ACCEPTED, Json(JobResponse { job_id })).into_response())
}

async fn audit(state: &AppState, actor: &Actor, instance: InstanceId, action: &'static str) {
    state
        .audit
        .record(AuditEntry::on_instance(actor.id(), instance, action))
        .await;
}
