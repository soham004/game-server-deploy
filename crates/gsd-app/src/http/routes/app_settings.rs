//! Settings of the app itself (`settings.manage`, owners). So far the `CurseForge` API key, which is
//! write-only: the API says whether one is set and never returns it, and it stays out of the audit
//! log.

use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    routing::{get, put},
};
use gsd_core::{
    audit::{AuditEntry, actions},
    settings::CURSEFORGE_API_KEY,
};
use gsd_mods::{ContentError, SourceError};
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

type Manager = AuthorizedGlobal<perm::SettingsManage>;

/// Far longer than any real key; a paste of something else is refused.
const MAX_KEY_CHARS: usize = 256;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/settings", get(show))
        .route("/settings/curseforge-key", put(set_key).delete(remove_key))
}

#[derive(Debug, Serialize)]
struct SettingsView {
    curseforge_key_set: bool,
}

async fn show(State(state): State<AppState>, _auth: Manager) -> Json<SettingsView> {
    Json(SettingsView {
        curseforge_key_set: state.content.has_curseforge_key(),
    })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct KeyBody {
    key: String,
}

fn refused() -> ApiError {
    ApiError::new(
        StatusCode::UNPROCESSABLE_ENTITY,
        "curseforge_key_refused",
        "CurseForge didn't accept that API key. Nothing was saved.",
    )
}

/// Tries the key on `CurseForge` first, so a typo shows now rather than at the next search. If
/// `CurseForge` can't be reached nothing is saved either, and the error says so.
async fn set_key(
    State(state): State<AppState>,
    auth: Manager,
    ApiJson(body): ApiJson<KeyBody>,
) -> Result<Json<SettingsView>, ApiError> {
    let key = body.key.trim();
    if key.is_empty() || key.chars().count() > MAX_KEY_CHARS {
        return Err(refused());
    }
    match state.content.check_curseforge_key(key).await {
        Ok(()) => {}
        Err(ContentError::Source(SourceError::KeyRefused)) => return Err(refused()),
        Err(error) => return Err(error.into()),
    }
    // Usable first, then saved: a key that can't be used never reaches the database.
    state
        .content
        .set_curseforge_key(Some(key))
        .map_err(|_| refused())?;
    state
        .settings
        .set(CURSEFORGE_API_KEY, key)
        .await
        .map_err(|error| ApiError::internal(&error))?;
    audit(&state, &auth, true).await;
    Ok(Json(SettingsView {
        curseforge_key_set: true,
    }))
}

async fn remove_key(State(state): State<AppState>, auth: Manager) -> Result<StatusCode, ApiError> {
    state
        .settings
        .remove(CURSEFORGE_API_KEY)
        .await
        .map_err(|error| ApiError::internal(&error))?;
    state.content.set_curseforge_key(None).map_err(|_| refused())?;
    audit(&state, &auth, false).await;
    Ok(StatusCode::NO_CONTENT)
}

async fn audit(state: &AppState, auth: &Manager, set: bool) {
    let entry = AuditEntry::by_user(auth.actor.id(), actions::CURSEFORGE_KEY_CHANGED)
        .with_details(json!({ "set": set }));
    state.audit.record(entry).await;
}
