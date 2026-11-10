//! Players of a server: who is on, operators, the allowlist and bans (`instance.view`; banned addresses
//! only with `players.manage`), visits, changes (`players.manage`; operator levels above 2 also need
//! `console.write`, since they let a player run any command), and heads.

use axum::{
    Json, Router,
    extract::{Path, State},
    http::{HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use gsd_core::{
    audit::{AuditEntry, actions},
    auth::Permission,
    ids::InstanceId,
    players::Session,
};
use gsd_minecraft::players::{Applied, PlayerAction, PlayersView};

use crate::{
    http::{
        authorize::{Authorized, perm},
        dto::ApiJson,
        error::ApiError,
        session::CurrentActor,
    },
    state::AppState,
};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/instances/{instance_id}/players", get(view))
        .route("/instances/{instance_id}/players/actions", post(act))
        .route("/instances/{instance_id}/players/{name}/sessions", get(sessions))
        .route("/players/{uuid}/head.png", get(head))
}

async fn view(
    State(state): State<AppState>,
    auth: Authorized<perm::InstanceView>,
) -> Result<Json<PlayersView>, ApiError> {
    let mut players = state.players.view(auth.instance_id).await?;
    if !auth.actor.can(Permission::PlayersManage, auth.instance_id) {
        players.ip_bans.clear();
    }
    Ok(Json(players))
}

async fn act(
    State(state): State<AppState>,
    auth: Authorized<perm::PlayersManage>,
    ApiJson(action): ApiJson<PlayerAction>,
) -> Result<Json<Applied>, ApiError> {
    let author = auth.actor.user().username.to_string();
    let any_level = auth.actor.can(Permission::ConsoleWrite, auth.instance_id);
    let applied = state
        .players
        .apply(auth.instance_id, &action, &author, any_level)
        .await?;
    let entry = AuditEntry::on_instance(auth.actor.id(), auth.instance_id, actions::PLAYER_ACTION)
        .with_details(serde_json::to_value(&action).unwrap_or_default());
    state.audit.record(entry).await;
    Ok(Json(applied))
}

async fn sessions(
    State(state): State<AppState>,
    auth: Authorized<perm::InstanceView>,
    Path((_, name)): Path<(InstanceId, String)>,
) -> Result<Json<Vec<Session>>, ApiError> {
    Ok(Json(state.players.sessions(auth.instance_id, &name).await?))
}

/// The head of an account, from its skin. Any signed-in user: heads are public on Mojang's side.
async fn head(
    State(state): State<AppState>,
    CurrentActor(_): CurrentActor,
    Path(uuid): Path<String>,
) -> Result<Response, ApiError> {
    match state.players.mojang().head(&uuid).await {
        Ok(Some(png)) => {
            let mut response = png.into_response();
            let headers = response.headers_mut();
            headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("image/png"));
            headers.insert(
                header::CACHE_CONTROL,
                HeaderValue::from_static("private, max-age=3600"),
            );
            Ok(response)
        }
        // The page shows its own picture instead.
        Ok(None) => Err(ApiError::new(
            StatusCode::NOT_FOUND,
            "no_head",
            "This player has no skin of their own.",
        )),
        Err(error) => {
            tracing::debug!(%error, "no head");
            Err(ApiError::new(
                StatusCode::BAD_GATEWAY,
                "mojang_unavailable",
                error.to_string(),
            ))
        }
    }
}
