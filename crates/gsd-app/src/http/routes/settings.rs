//! An instance's settings: name, memory, JVM tuning, Java runtime and behaviour.

use axum::{Json, Router, extract::State, routing::get};
use gsd_core::{
    audit::{AuditEntry, actions},
    auth::{GlobalPermission, Permission},
    instances::{
        Instance, SettingsChange,
        definition::{Behavior, JavaSettings},
    },
    java::JavaRuntime,
};
use serde::Serialize;
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
    Router::new().route("/instances/{instance_id}/settings", get(show).patch(update))
}

#[derive(Debug, Serialize)]
struct SettingsView {
    name: String,
    java: JavaSettings,
    behavior: Behavior,
    /// Java installations to choose from (`managed`: installed by the app).
    runtimes: Vec<JavaRuntime>,
    can_edit: bool,
    /// May pick a runtime the app didn't install.
    can_use_custom_runtime: bool,
    /// The server is running: changes apply after a restart.
    restart_required: bool,
}

async fn show(
    State(state): State<AppState>,
    auth: Authorized<perm::InstanceView>,
) -> Result<Json<SettingsView>, ApiError> {
    let instance = state.instances.get(auth.instance_id).await?;
    Ok(Json(view(&state, &auth, &instance).await))
}

async fn update(
    State(state): State<AppState>,
    auth: Authorized<perm::InstanceSettings>,
    ApiJson(mut change): ApiJson<SettingsChange>,
) -> Result<Json<SettingsView>, ApiError> {
    let allow_custom = auth.actor.can_global(GlobalPermission::CustomRuntime);
    if is_minecraft(&state.instances.get(auth.instance_id).await?)
        && let Some(user_args) = change.extra_jvm_args.take()
    {
        // The console needs the default flags: keep them whatever the form sends.
        change.extra_jvm_args = Some(gsd_minecraft::with_default_jvm_args(&user_args));
    }
    let instance = state
        .instances
        .update_settings(auth.instance_id, &change, allow_custom)
        .await?;
    let entry = AuditEntry::on_instance(auth.actor.id(), auth.instance_id, actions::SETTINGS_CHANGED)
        .with_details(json!({ "change": changed_fields(&change) }));
    state.audit.record(entry).await;
    Ok(Json(view(&state, &auth, &instance).await))
}

async fn view<P>(state: &AppState, auth: &Authorized<P>, instance: &Instance) -> SettingsView {
    let definition = &instance.definition;
    let mut java = definition.java.clone();
    if is_minecraft(instance) {
        java.extra_jvm_args = gsd_minecraft::user_jvm_args(&java.extra_jvm_args);
    }
    SettingsView {
        name: definition.name.clone(),
        java,
        behavior: definition.behavior.clone(),
        runtimes: state.java.list(false).await,
        can_edit: auth.actor.can(Permission::InstanceSettings, auth.instance_id),
        can_use_custom_runtime: auth.actor.can_global(GlobalPermission::CustomRuntime),
        restart_required: state.instances.state(auth.instance_id).is_active(),
    }
}

/// Which fields a change touched (for the audit log; values like JVM arguments stay out).
fn changed_fields(change: &SettingsChange) -> Vec<&'static str> {
    [
        ("name", change.name.is_some()),
        ("min_memory_mb", change.min_memory_mb.is_some()),
        ("max_memory_mb", change.max_memory_mb.is_some()),
        ("jvm_preset", change.jvm_preset.is_some()),
        ("extra_jvm_args", change.extra_jvm_args.is_some()),
        ("java_runtime", change.java_runtime.is_some()),
        ("auto_start", change.auto_start.is_some()),
        ("auto_restart", change.auto_restart.is_some()),
        ("crash_limit", change.crash_limit.is_some()),
        ("stop_timeout_secs", change.stop_timeout_secs.is_some()),
    ]
    .into_iter()
    .filter_map(|(field, changed)| changed.then_some(field))
    .collect()
}

fn is_minecraft(instance: &Instance) -> bool {
    instance.definition.game == gsd_minecraft::GAME_ID
}
