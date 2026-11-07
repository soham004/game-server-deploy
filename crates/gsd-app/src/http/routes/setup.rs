//! First-run setup: create the owner account.

use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    response::Response,
    routing::{get, post},
};
use gsd_core::audit::{AuditEntry, actions};
use serde::{Deserialize, Serialize};

use crate::{
    http::{client::ClientContext, dto::ApiJson, error::ApiError, routes::signed_in},
    state::AppState,
};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/setup/status", get(status))
        .route("/setup", post(create_owner))
}

#[derive(Debug, Serialize)]
struct SetupStatus {
    setup_required: bool,
    /// The server is reachable from the network, so the token from the server log is needed.
    token_required: bool,
}

async fn status(State(state): State<AppState>) -> Result<Json<SetupStatus>, ApiError> {
    let setup_required = state.auth.is_setup_required().await?;
    let token_required = setup_required && state.setup_gate.requires_token();
    Ok(Json(SetupStatus {
        setup_required,
        token_required,
    }))
}

#[derive(Debug, Deserialize)]
struct SetupRequest {
    username: String,
    password: String,
    #[serde(default)]
    setup_token: Option<String>,
}

async fn create_owner(
    State(state): State<AppState>,
    client: ClientContext,
    ApiJson(body): ApiJson<SetupRequest>,
) -> Result<Response, ApiError> {
    if !state.setup_gate.allows(body.setup_token.as_deref()) {
        return Err(ApiError::forbidden(
            "setup_token_invalid",
            "The setup token is missing or wrong. It is printed in the server log.",
        ));
    }

    let outcome = state
        .auth
        .create_owner(&body.username, &body.password, &client.info())
        .await?;
    state
        .audit
        .record(AuditEntry::by_user(outcome.user.id, actions::SETUP_COMPLETED))
        .await;
    Ok(signed_in(StatusCode::CREATED, &outcome, &state, &client))
}
