//! The signed-in user: profile, effective permissions, own password.

use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    response::Response,
    routing::{get, post},
};
use gsd_core::{
    audit::{AuditEntry, actions},
    auth::{AuthError, GlobalPermission, PermissionSet},
};
use serde::{Deserialize, Serialize};

use crate::{
    http::{
        client::ClientContext, dto::ApiJson, dto::UserDto, error::ApiError, routes::signed_in,
        session::SignedIn,
    },
    state::AppState,
};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/me", get(me))
        .route("/me/password", post(change_password))
}

#[derive(Debug, Serialize)]
struct MeResponse {
    user: UserDto,
    global_permissions: &'static [GlobalPermission],
    /// Owners and admins implicitly hold every permission on every instance.
    all_instances: bool,
    instance_grants: Vec<InstanceGrantDto>,
}

#[derive(Debug, Serialize)]
struct InstanceGrantDto {
    instance_id: String,
    permissions: PermissionSet,
}

async fn me(SignedIn(actor): SignedIn) -> Json<MeResponse> {
    let instance_grants = actor
        .instance_grants()
        .map(|(id, permissions)| InstanceGrantDto {
            instance_id: id.to_string(),
            permissions,
        })
        .collect();
    Json(MeResponse {
        user: UserDto::from(actor.user()),
        global_permissions: actor.role().global_permissions(),
        all_instances: actor.role().can_access_all_instances(),
        instance_grants,
    })
}

#[derive(Debug, Deserialize)]
struct ChangePasswordRequest {
    current_password: String,
    new_password: String,
}

/// Changes the password, signs out every other session, and returns a fresh session cookie.
async fn change_password(
    State(state): State<AppState>,
    client: ClientContext,
    SignedIn(actor): SignedIn,
    ApiJson(body): ApiJson<ChangePasswordRequest>,
) -> Result<Response, ApiError> {
    let outcome = state
        .auth
        .change_password(&actor, &body.current_password, &body.new_password, &client.info())
        .await
        .map_err(|error| match error {
            // Not 401: the user *is* signed in, their current password is just wrong.
            AuthError::InvalidCredentials => {
                ApiError::forbidden("wrong_password", "Your current password is incorrect.")
            }
            other => other.into(),
        })?;

    state
        .audit
        .record(AuditEntry::by_user(actor.id(), actions::PASSWORD_CHANGED))
        .await;
    Ok(signed_in(StatusCode::OK, &outcome, &state, &client))
}
