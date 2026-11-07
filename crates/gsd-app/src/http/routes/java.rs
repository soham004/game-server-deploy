//! Java runtimes: list detected/managed ones, download a Temurin release.

use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
};
use gsd_core::{
    audit::{AuditEntry, actions},
    ids::JobId,
    java::JavaRuntime,
    jobs::JobSpec,
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

pub fn routes() -> Router<AppState> {
    Router::new().route("/java", get(list).post(install))
}

async fn list(
    State(state): State<AppState>,
    _auth: AuthorizedGlobal<perm::InstanceCreate>,
) -> Json<Vec<JavaRuntime>> {
    Json(state.java.list(true).await)
}

#[derive(Debug, Deserialize)]
struct InstallBody {
    major: u32,
}

#[derive(Debug, Serialize)]
struct JobResponse {
    job_id: JobId,
}

/// Downloads Eclipse Temurin `major` (no-op if one is already installed).
async fn install(
    State(state): State<AppState>,
    auth: AuthorizedGlobal<perm::InstanceCreate>,
    ApiJson(body): ApiJson<InstallBody>,
) -> Result<Response, ApiError> {
    if !(8..=99).contains(&body.major) {
        return Err(ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_java_version",
            "Unknown Java version.",
        ));
    }
    let java = state.java.clone();
    let major = body.major;
    let spec = JobSpec {
        kind: "java.install",
        title: format!("Installing Java {major}"),
        instance_id: None,
    };
    let job_id = state.jobs.spawn(spec, move |job| async move {
        java.ensure(major, &job).await?;
        Ok(())
    })?;
    let entry =
        AuditEntry::by_user(auth.actor.id(), actions::JAVA_INSTALLED).with_details(json!({ "major": major }));
    state.audit.record(entry).await;
    Ok((StatusCode::ACCEPTED, Json(JobResponse { job_id })).into_response())
}
