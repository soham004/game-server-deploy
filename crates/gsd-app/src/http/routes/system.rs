//! System endpoints: health, product metadata, host capabilities and the isolation self-test.

use axum::{
    Json, Router,
    extract::State,
    routing::{get, post},
};
use gsd_core::supervisor::{IsolationLevel, SandboxSupport, sandbox::check::IsolationReport};
use serde::Serialize;

use crate::{
    http::{
        authorize::{AuthorizedGlobal, perm},
        error::ApiError,
        session::CurrentActor,
    },
    state::AppState,
};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/health", get(health))
        .route("/meta", get(meta))
        .route("/system", get(system))
        .route("/system/isolation-check", post(isolation_check))
}

#[derive(Debug, Serialize)]
struct Health {
    status: &'static str,
}

async fn health() -> Json<Health> {
    Json(Health { status: "ok" })
}

/// Branding for the frontend, which never hard-codes the product name.
#[derive(Debug, Serialize)]
struct Meta {
    name: &'static str,
    slug: &'static str,
    version: &'static str,
}

async fn meta() -> Json<Meta> {
    Json(Meta {
        name: gsd_brand::NAME,
        slug: gsd_brand::SLUG,
        version: gsd_brand::VERSION,
    })
}

#[derive(Debug, Serialize)]
struct SystemInfo {
    version: &'static str,
    os: &'static str,
    arch: &'static str,
    isolation: SandboxSupport,
    isolation_level: IsolationLevel,
    game_ports: String,
}

async fn system(State(state): State<AppState>, _actor: CurrentActor) -> Json<SystemInfo> {
    let support = state.instances.supervisors().support();
    Json(SystemInfo {
        version: gsd_brand::VERSION,
        os: std::env::consts::OS,
        arch: std::env::consts::ARCH,
        isolation: support,
        isolation_level: support.level(),
        game_ports: state.config.game_ports.to_string(),
    })
}

/// Runs the sandbox self-test on this host (spawns short-lived probe processes).
async fn isolation_check(
    State(state): State<AppState>,
    _auth: AuthorizedGlobal<perm::InstanceCreate>,
) -> Result<Json<IsolationReport>, ApiError> {
    let probe = std::env::current_exe().map_err(|error| ApiError::internal(&error))?;
    let scratch_parent = state.config.data_dir.join("cache");
    let scratch = tempfile::Builder::new()
        .prefix("isolation-check-")
        .tempdir_in(&scratch_parent)
        .map_err(|error| ApiError::internal(&error))?;
    let support = state.instances.supervisors().support();
    let report = gsd_core::supervisor::sandbox::check::run(&probe, scratch.path(), support)
        .await
        .map_err(|error| ApiError::internal(&error))?;
    Ok(Json(report))
}
