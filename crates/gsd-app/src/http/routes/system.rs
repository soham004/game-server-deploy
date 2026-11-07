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
    /// Set when servers can't start because the sandbox can't work here (explains the fix).
    sandbox_problem: Option<String>,
    /// Servers get a hard memory and task ceiling (cgroup v2).
    hard_limits: bool,
    /// Why they don't, if they don't.
    hard_limits_problem: Option<String>,
    game_ports: String,
}

async fn system(State(state): State<AppState>, _actor: CurrentActor) -> Json<SystemInfo> {
    let host = state.instances.supervisors().host();
    let support = host.support();
    Json(SystemInfo {
        version: gsd_brand::VERSION,
        os: std::env::consts::OS,
        arch: std::env::consts::ARCH,
        isolation: support,
        isolation_level: support.level(),
        sandbox_problem: host.problem().map(str::to_owned),
        hard_limits: host.cgroups().is_some(),
        hard_limits_problem: host.cgroups_problem().map(str::to_owned),
        game_ports: state.config.game_ports.to_string(),
    })
}

/// Runs the sandbox self-test on this host (spawns short-lived probe processes).
async fn isolation_check(
    State(state): State<AppState>,
    _auth: AuthorizedGlobal<perm::InstanceCreate>,
) -> Result<Json<IsolationReport>, ApiError> {
    let probe = std::env::current_exe().map_err(|error| ApiError::internal(&error))?;
    // In the data dir, not /tmp: its filesystem and permissions are what servers actually get.
    // (Not `cache/` either: it is private, so a sandbox user couldn't reach a scratch dir there.)
    let scratch = gsd_core::supervisor::sandbox::check::scratch_in(&state.config.sandbox_check_dir())
        .map_err(|error| ApiError::internal(&error))?;
    let host = state.instances.supervisors().host();
    let report = gsd_core::supervisor::sandbox::check::run(&probe, scratch.path(), host)
        .await
        .map_err(|error| ApiError::internal(&error))?;
    Ok(Json(report))
}
