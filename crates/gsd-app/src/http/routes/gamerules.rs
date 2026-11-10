//! A server's game rules (`instance.view`), and changes to them (`properties.write`, which covers game
//! rules in the permission matrix).

use std::collections::BTreeMap;

use axum::{Json, Router, extract::State, routing::get};
use gsd_core::audit::{AuditEntry, actions};
use gsd_minecraft::gamerules::{GamerulesView, RuleValue, Updated};
use serde::Deserialize;
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
    Router::new().route("/instances/{instance_id}/gamerules", get(view).patch(update))
}

async fn view(
    State(state): State<AppState>,
    auth: Authorized<perm::InstanceView>,
) -> Result<Json<GamerulesView>, ApiError> {
    Ok(Json(state.gamerules.view(auth.instance_id).await?))
}

#[derive(Debug, Deserialize)]
struct UpdateBody {
    /// Rule name to its new value.
    changes: BTreeMap<String, RuleValue>,
}

async fn update(
    State(state): State<AppState>,
    auth: Authorized<perm::PropertiesWrite>,
    ApiJson(body): ApiJson<UpdateBody>,
) -> Result<Json<Updated>, ApiError> {
    let author = auth.actor.user().username.to_string();
    let details = json!({ "changes": body.changes });
    let updated = state
        .gamerules
        .update(auth.instance_id, body.changes, &author)
        .await?;
    let entry = AuditEntry::on_instance(auth.actor.id(), auth.instance_id, actions::GAMERULES_CHANGED)
        .with_details(details);
    state.audit.record(entry).await;
    Ok(Json(updated))
}
