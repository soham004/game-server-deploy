//! Sign in and sign out.

use axum::{
    Router,
    extract::State,
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::post,
};
use gsd_core::{
    audit::{AuditEntry, actions},
    auth::AuthError,
};
use serde::Deserialize;
use serde_json::json;

use crate::{
    http::{
        client::ClientContext,
        dto::ApiJson,
        error::ApiError,
        routes::signed_in,
        session::{cleared_session_cookie, session_token},
    },
    state::AppState,
};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/auth/login", post(login))
        .route("/auth/logout", post(logout))
}

#[derive(Debug, Deserialize)]
struct LoginRequest {
    username: String,
    password: String,
}

async fn login(
    State(state): State<AppState>,
    client: ClientContext,
    ApiJson(body): ApiJson<LoginRequest>,
) -> Result<Response, ApiError> {
    let ip = client.ip.map(|ip| ip.to_string());
    match state
        .auth
        .login(&body.username, &body.password, &client.info())
        .await
    {
        Ok(outcome) => {
            let entry =
                AuditEntry::by_user(outcome.user.id, actions::LOGIN).with_details(json!({ "ip": ip }));
            state.audit.record(entry).await;
            Ok(signed_in(StatusCode::OK, &outcome, &state, &client))
        }
        Err(error) => {
            if matches!(error, AuthError::InvalidCredentials) {
                let details = json!({ "username": body.username.trim(), "ip": ip });
                state
                    .audit
                    .record(AuditEntry::anonymous(actions::LOGIN_FAILED).with_details(details))
                    .await;
            }
            Err(error.into())
        }
    }
}

/// Always succeeds and clears the cookie, even if the session was already gone.
async fn logout(
    State(state): State<AppState>,
    client: ClientContext,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    if let Some(token) = session_token(&headers) {
        if let Ok(actor) = state.auth.authenticate(&token).await {
            state
                .audit
                .record(AuditEntry::by_user(actor.id(), actions::LOGOUT))
                .await;
        }
        state.auth.logout(&token).await?;
    }
    Ok((
        StatusCode::NO_CONTENT,
        [(header::SET_COOKIE, cleared_session_cookie(client.is_https))],
    )
        .into_response())
}
