//! Console autocomplete, for those who may type commands (`console.write`): where a server's commands
//! stand, preparing them, and completing what's typed.

use axum::{
    Json, Router,
    extract::{Query, State},
    http::StatusCode,
    routing::{get, post},
};
use gsd_minecraft::commands::{CommandsStatus, Completion};
use serde::Deserialize;

use crate::{
    http::{
        authorize::{Authorized, perm},
        dto::ApiJson,
        error::ApiError,
    },
    state::AppState,
};

/// Longer than any command a console takes (the game's limit is 32,500 characters).
const MAX_INPUT: usize = 32_768;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/instances/{instance_id}/commands", get(status))
        .route("/instances/{instance_id}/commands/prepare", post(prepare))
        .route("/instances/{instance_id}/commands/complete", get(complete))
}

async fn status(
    State(state): State<AppState>,
    auth: Authorized<perm::ConsoleWrite>,
) -> Result<Json<CommandsStatus>, ApiError> {
    Ok(Json(state.commands.status(auth.instance_id).await?))
}

#[derive(Debug, Default, Deserialize)]
struct PrepareBody {
    /// Try again after a failure.
    #[serde(default)]
    retry: bool,
}

/// Starts what's missing (the data generator's job, the server's `help`); idempotent.
async fn prepare(
    State(state): State<AppState>,
    auth: Authorized<perm::ConsoleWrite>,
    ApiJson(body): ApiJson<PrepareBody>,
) -> Result<Json<CommandsStatus>, ApiError> {
    Ok(Json(state.commands.prepare(auth.instance_id, body.retry).await?))
}

#[derive(Debug, Deserialize)]
struct CompleteQuery {
    input: String,
    /// In UTF-16 code units, as the browser counts; the end of the input when left out.
    cursor: Option<usize>,
}

async fn complete(
    State(state): State<AppState>,
    auth: Authorized<perm::ConsoleWrite>,
    Query(query): Query<CompleteQuery>,
) -> Result<Json<Completion>, ApiError> {
    if query.input.len() > MAX_INPUT {
        return Err(ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "input_too_long",
            "That is longer than any command.",
        ));
    }
    let cursor = query.cursor.unwrap_or(usize::MAX);
    Ok(Json(
        state
            .commands
            .complete(auth.instance_id, &query.input, cursor)
            .await?,
    ))
}
