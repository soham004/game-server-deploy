//! The `server.properties` editor: the file, its values and the schema to render them with.

use std::collections::BTreeMap;

use axum::{Json, Router, extract::State, http::StatusCode, routing::get};
use gsd_core::{
    audit::{AuditEntry, actions},
    auth::Permission,
    instances::Instance,
};
use gsd_minecraft::{
    GAME_ID,
    properties::{self, PropertiesEdit, PropertiesSnapshot, PropertySchema},
};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::{
    http::{
        authorize::{Authorized, perm},
        dto::ApiJson,
        error::ApiError,
    },
    state::AppState,
};

pub fn routes() -> Router<AppState> {
    Router::new().route("/instances/{instance_id}/properties", get(show).put(update))
}

#[derive(Debug, Serialize)]
struct PropertiesResponse {
    #[serde(flatten)]
    snapshot: PropertiesSnapshot,
    /// What every known key means (labels, types, defaults, categories).
    schema: &'static [PropertySchema],
    /// The current user may change the file (otherwise secrets are blanked).
    can_edit: bool,
    /// The server is running, so changes apply after a restart.
    restart_required: bool,
}

async fn show(
    State(state): State<AppState>,
    auth: Authorized<perm::InstanceView>,
) -> Result<Json<PropertiesResponse>, ApiError> {
    let instance = minecraft_instance(&state, &auth).await?;
    let snapshot = properties::read(&instance).await?;
    let can_edit = auth.actor.can(Permission::PropertiesWrite, auth.instance_id);
    let snapshot = if can_edit { snapshot } else { snapshot.redacted() };
    Ok(Json(response(&state, &auth, snapshot, can_edit)))
}

/// Exactly one of `changes` (key → value, `null` removes) or `text` (the whole file).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct UpdateBody {
    revision: String,
    #[serde(default)]
    changes: Option<BTreeMap<String, Option<String>>>,
    #[serde(default)]
    text: Option<String>,
}

async fn update(
    State(state): State<AppState>,
    auth: Authorized<perm::PropertiesWrite>,
    ApiJson(body): ApiJson<UpdateBody>,
) -> Result<Json<PropertiesResponse>, ApiError> {
    let instance = minecraft_instance(&state, &auth).await?;
    let (edit, changed_keys) = match (body.changes, body.text) {
        (Some(changes), None) => {
            let keys: Vec<String> = changes.keys().cloned().collect();
            (PropertiesEdit::Changes(changes), json!(keys))
        }
        (None, Some(text)) => (PropertiesEdit::Replace(text), json!("whole file")),
        _ => {
            return Err(ApiError::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                "invalid_request_body",
                "Send either `changes` or `text`.",
            ));
        }
    };
    let snapshot = properties::apply(&instance, &edit, &body.revision).await?;
    let entry = AuditEntry::on_instance(auth.actor.id(), auth.instance_id, actions::PROPERTIES_CHANGED)
        .with_details(json!({ "keys": changed_keys }));
    state.audit.record(entry).await;
    Ok(Json(response(&state, &auth, snapshot, true)))
}

fn response<P>(
    state: &AppState,
    auth: &Authorized<P>,
    snapshot: PropertiesSnapshot,
    can_edit: bool,
) -> PropertiesResponse {
    PropertiesResponse {
        snapshot,
        schema: properties::schema::all(),
        can_edit,
        restart_required: state.instances.state(auth.instance_id).is_active(),
    }
}

async fn minecraft_instance<P>(state: &AppState, auth: &Authorized<P>) -> Result<Instance, ApiError> {
    let instance = state.instances.get(auth.instance_id).await?;
    if instance.definition.game != GAME_ID {
        return Err(ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "not_minecraft",
            "This server has no server.properties.",
        ));
    }
    Ok(instance)
}
