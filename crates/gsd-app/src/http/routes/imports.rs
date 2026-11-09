//! Importing existing server folders from `<data>/imports/` (owners and admins). A zip uploaded to
//! `routes::modpacks` lands there too.

use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
};
use gsd_core::{
    audit::{AuditEntry, actions},
    ids::{InstanceId, JobId},
    jobs::JobStatus,
};
use gsd_minecraft::{ImportCandidate, ImportRequest, LoaderKind};
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

use super::instances::{CreatedResponse, InstanceDto};

pub fn routes() -> Router<AppState> {
    Router::new().route("/imports", get(list).post(import))
}

#[derive(Debug, Serialize)]
struct ImportsView {
    /// Where to put a folder so it shows up here (a path on the app's host or container).
    folder: String,
    candidates: Vec<ImportCandidate>,
}

async fn list(
    State(state): State<AppState>,
    _auth: AuthorizedGlobal<perm::InstanceCreate>,
) -> Json<ImportsView> {
    Json(ImportsView {
        folder: state.config.imports_dir().display().to_string(),
        candidates: state.minecraft.import_candidates().await,
    })
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ImportBody {
    folder: String,
    name: String,
    mc_version: String,
    loader: LoaderKind,
    #[serde(default)]
    loader_version: Option<String>,
    #[serde(default)]
    jar: Option<String>,
    max_memory_mb: u32,
    #[serde(default)]
    eula_accepted: bool,
}

/// Once the import has finished, looks up the jars it brought, so they can be tracked and updated.
/// Only a convenience: if it fails, the Mods tab still offers the same lookup.
fn identify_when_done(state: AppState, id: InstanceId, job: JobId) {
    tokio::spawn(async move {
        let finished = state.jobs.wait(job).await;
        if finished.is_some_and(|job| job.status == JobStatus::Succeeded)
            && let Err(error) = state.content.identify(id).await
            && !matches!(error, gsd_mods::ContentError::NoContent)
        {
            tracing::warn!(%error, "could not look up what an import brought");
        }
    });
}

/// 202: the instance exists; the folder is copied in by the returned job.
async fn import(
    State(state): State<AppState>,
    auth: AuthorizedGlobal<perm::InstanceCreate>,
    ApiJson(body): ApiJson<ImportBody>,
) -> Result<Response, ApiError> {
    let folder = body.folder.clone();
    let request = ImportRequest {
        folder: body.folder,
        name: body.name,
        mc_version: body.mc_version,
        loader: body.loader,
        loader_version: body.loader_version,
        jar: body.jar.filter(|jar| !jar.trim().is_empty()),
        max_memory_mb: body.max_memory_mb,
        eula_accepted: body.eula_accepted,
    };
    let (instance, job_id) = state.minecraft.import(request).await?;
    identify_when_done(state.clone(), instance.id(), job_id);
    let details = json!({ "name": instance.definition.name, "imported_from": folder });
    state
        .audit
        .record(
            AuditEntry::on_instance(auth.actor.id(), instance.id(), actions::INSTANCE_CREATED)
                .with_details(details),
        )
        .await;
    let body = CreatedResponse {
        instance: InstanceDto::new(&state, &instance, &auth.actor),
        job_id,
    };
    Ok((StatusCode::ACCEPTED, Json(body)).into_response())
}
