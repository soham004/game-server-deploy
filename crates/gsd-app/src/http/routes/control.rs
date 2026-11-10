//! Whether the app can reach a running server through its control channel (`instance.view`). Secrets
//! stay in the server's own settings: nothing here returns one.

use axum::{Json, Router, extract::State, routing::get};
use gsd_minecraft::control::{self, Channel, ControlStatus};
use gsd_minecraft::settings::MinecraftServer;

use crate::{
    http::{
        authorize::{Authorized, perm},
        error::ApiError,
    },
    state::AppState,
};

pub fn routes() -> Router<AppState> {
    Router::new().route("/instances/{instance_id}/control", get(status))
}

async fn status(
    State(state): State<AppState>,
    auth: Authorized<perm::InstanceView>,
) -> Result<Json<ControlStatus>, ApiError> {
    let instance = state.instances.get(auth.instance_id).await?;
    if state.instances.state(auth.instance_id).is_active() {
        return Ok(Json(control::status(&instance).await));
    }
    // Stopped: which channel it has, without trying what can't work.
    let channel = MinecraftServer::from_table(&instance.definition.server)
        .map_or(Channel::Rcon, |server| control::channel_for(&server.mc_version));
    Ok(Json(ControlStatus {
        channel,
        connected: false,
        problem: None,
        api_version: None,
        methods: Vec::new(),
    }))
}
