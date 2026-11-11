//! Sign in and sign out.

use std::time::{Duration, Instant};

use axum::{
    Router,
    extract::State,
    http::{HeaderMap, HeaderValue, StatusCode, header},
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
    // Before the password is looked at: an attempt that has to wait costs nothing to refuse.
    if let Err(wait) = state
        .login_throttle
        .check(client.ip, &body.username, Instant::now())
    {
        return Ok(too_many_attempts(wait));
    }
    match state
        .auth
        .login(&body.username, &body.password, &client.info())
        .await
    {
        Ok(outcome) => {
            state.login_throttle.record_success(client.ip, &body.username);
            let entry =
                AuditEntry::by_user(outcome.user.id, actions::LOGIN).with_details(json!({ "ip": ip }));
            state.audit.record(entry).await;
            Ok(signed_in(StatusCode::OK, &outcome, &state, &client))
        }
        Err(error) => {
            if matches!(error, AuthError::InvalidCredentials) {
                state
                    .login_throttle
                    .record_failure(client.ip, &body.username, Instant::now());
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

/// `429` with `Retry-After`: too many wrong passwords from this address (see `throttle.rs`).
fn too_many_attempts(wait: Duration) -> Response {
    let seconds = wait.as_secs().max(1);
    let when = if seconds < 90 {
        format!("{seconds} seconds")
    } else {
        format!("{} minutes", seconds.div_ceil(60))
    };
    let error = ApiError::new(
        StatusCode::TOO_MANY_REQUESTS,
        "too_many_attempts",
        format!("Too many failed sign-ins. Try again in {when}."),
    );
    let mut response = error.into_response();
    response
        .headers_mut()
        .insert(header::RETRY_AFTER, HeaderValue::from(seconds));
    response
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
