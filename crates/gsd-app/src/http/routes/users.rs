//! Managing accounts and their per-instance access (owners: `users.manage`).

use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::{get, post, put},
};
use chrono::{DateTime, Utc};
use gsd_core::{
    audit::{AuditEntry, actions},
    auth::{AccountChange, GlobalRole, Grant, PermissionSet, UserSummary},
    ids::{InstanceId, UserId},
};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::{
    http::{
        authorize::{AuthorizedGlobal, perm},
        dto::ApiJson,
        error::ApiError,
    },
    state::AppState,
};

type Owner = AuthorizedGlobal<perm::UsersManage>;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/users", get(list).post(create))
        .route("/users/{user_id}", axum::routing::patch(update).delete(delete))
        .route("/users/{user_id}/reset-password", post(reset_password))
        .route("/users/{user_id}/sign-out", post(sign_out))
        .route("/users/{user_id}/grants", get(grants))
        .route(
            "/users/{user_id}/grants/{instance_id}",
            put(set_grant).delete(remove_grant),
        )
}

#[derive(Debug, Serialize)]
struct UserRow {
    id: UserId,
    username: String,
    role: GlobalRole,
    disabled: bool,
    must_change_password: bool,
    created_at: DateTime<Utc>,
    active_sessions: i64,
}

impl From<UserSummary> for UserRow {
    fn from(summary: UserSummary) -> Self {
        let user = summary.user;
        Self {
            id: user.id,
            username: user.username.to_string(),
            role: user.role,
            disabled: user.disabled,
            must_change_password: user.must_change_password,
            created_at: user.created_at,
            active_sessions: summary.active_sessions,
        }
    }
}

async fn list(State(state): State<AppState>, _auth: Owner) -> Result<Json<Vec<UserRow>>, ApiError> {
    let users = state.auth.list_users().await?;
    Ok(Json(users.into_iter().map(UserRow::from).collect()))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct NewAccount {
    username: String,
    role: GlobalRole,
}

#[derive(Debug, Serialize)]
struct TemporaryPassword {
    /// Shown once: the user must change it at first sign-in.
    temporary_password: String,
}

#[derive(Debug, Serialize)]
struct Created {
    user: UserRow,
    temporary_password: String,
}

async fn create(
    State(state): State<AppState>,
    auth: Owner,
    ApiJson(body): ApiJson<NewAccount>,
) -> Result<(StatusCode, Json<Created>), ApiError> {
    let (user, temporary_password) = state.auth.create_user(&body.username, body.role).await?;
    let details = json!({ "username": user.username.to_string(), "role": user.role });
    record(&state, &auth, user.id, actions::USER_CREATED, details).await;
    let user = UserRow::from(UserSummary {
        user,
        active_sessions: 0,
    });
    Ok((
        StatusCode::CREATED,
        Json(Created {
            user,
            temporary_password,
        }),
    ))
}

async fn update(
    State(state): State<AppState>,
    auth: Owner,
    Path(user_id): Path<UserId>,
    ApiJson(change): ApiJson<AccountChange>,
) -> Result<Json<UserRow>, ApiError> {
    let user = state.auth.update_user(&auth.actor, user_id, &change).await?;
    let details = json!({ "role": change.role, "disabled": change.disabled });
    record(&state, &auth, user_id, actions::USER_UPDATED, details).await;
    Ok(Json(UserRow::from(UserSummary {
        user,
        active_sessions: 0,
    })))
}

async fn delete(
    State(state): State<AppState>,
    auth: Owner,
    Path(user_id): Path<UserId>,
) -> Result<StatusCode, ApiError> {
    state.auth.delete_user(&auth.actor, user_id).await?;
    record(&state, &auth, user_id, actions::USER_DELETED, json!({})).await;
    Ok(StatusCode::NO_CONTENT)
}

async fn reset_password(
    State(state): State<AppState>,
    auth: Owner,
    Path(user_id): Path<UserId>,
) -> Result<Json<TemporaryPassword>, ApiError> {
    let temporary_password = state.auth.reset_user_password(user_id).await?;
    record(&state, &auth, user_id, actions::PASSWORD_RESET, json!({})).await;
    Ok(Json(TemporaryPassword { temporary_password }))
}

async fn sign_out(
    State(state): State<AppState>,
    auth: Owner,
    Path(user_id): Path<UserId>,
) -> Result<StatusCode, ApiError> {
    state.auth.sign_out_everywhere(user_id).await?;
    record(&state, &auth, user_id, actions::USER_SIGNED_OUT, json!({})).await;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Serialize)]
struct GrantRow {
    instance_id: InstanceId,
    /// `None` if the instance was deleted since.
    instance_name: Option<String>,
    grant: &'static str,
    permissions: PermissionSet,
}

async fn grants(
    State(state): State<AppState>,
    _auth: Owner,
    Path(user_id): Path<UserId>,
) -> Result<Json<Vec<GrantRow>>, ApiError> {
    let mut rows = Vec::new();
    for (instance_id, grant) in state.auth.user_grants(user_id).await? {
        let instance_name = state
            .instances
            .registry()
            .get(instance_id)
            .await
            .map(|instance| instance.definition.name);
        rows.push(GrantRow {
            instance_id,
            instance_name,
            grant: grant.preset_name(),
            permissions: grant.permissions(),
        });
    }
    Ok(Json(rows))
}

/// A preset (`viewer`, `moderator`, `operator`, `manager`) or `custom` with a permission list.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct GrantBody {
    grant: String,
    #[serde(default)]
    permissions: Option<PermissionSet>,
}

async fn set_grant(
    State(state): State<AppState>,
    auth: Owner,
    Path((user_id, instance_id)): Path<(UserId, InstanceId)>,
    ApiJson(body): ApiJson<GrantBody>,
) -> Result<StatusCode, ApiError> {
    state.instances.get(instance_id).await?;
    let grant = Grant::from_stored(&body.grant, body.permissions).ok_or_else(|| {
        ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_grant",
            "Use viewer, moderator, operator, manager, or custom with a permission list.",
        )
    })?;
    state.auth.user_grants(user_id).await?; // the user must exist
    state
        .auth
        .grants()
        .set(user_id, instance_id, grant)
        .await
        .map_err(|error| ApiError::internal(&error))?;
    let details = json!({ "instance_id": instance_id, "grant": grant.preset_name() });
    record(&state, &auth, user_id, actions::GRANT_CHANGED, details).await;
    Ok(StatusCode::NO_CONTENT)
}

async fn remove_grant(
    State(state): State<AppState>,
    auth: Owner,
    Path((user_id, instance_id)): Path<(UserId, InstanceId)>,
) -> Result<StatusCode, ApiError> {
    state.auth.user_grants(user_id).await?;
    state
        .auth
        .grants()
        .remove(user_id, instance_id)
        .await
        .map_err(|error| ApiError::internal(&error))?;
    let details = json!({ "instance_id": instance_id, "grant": null });
    record(&state, &auth, user_id, actions::GRANT_CHANGED, details).await;
    Ok(StatusCode::NO_CONTENT)
}

async fn record(
    state: &AppState,
    auth: &Owner,
    target: UserId,
    action: &'static str,
    mut details: serde_json::Value,
) {
    details["user_id"] = json!(target);
    let entry = AuditEntry::by_user(auth.actor.id(), action).with_details(details);
    state.audit.record(entry).await;
}
