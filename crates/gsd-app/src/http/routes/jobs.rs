//! Background jobs: list, inspect (with log), cancel.

use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::{get, post},
};
use gsd_core::{
    auth::{Actor, Permission},
    ids::JobId,
    jobs::JobSnapshot,
};
use serde::Serialize;

use crate::{
    http::{error::ApiError, session::CurrentActor},
    state::AppState,
};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/jobs", get(list))
        .route("/jobs/{job_id}", get(show))
        .route("/jobs/{job_id}/cancel", post(cancel))
}

/// Instance jobs are visible to whoever can see the instance; app-wide jobs (e.g. a Java
/// download) to owners and admins.
pub fn can_view(actor: &Actor, job: &JobSnapshot) -> bool {
    job.instance_id.map_or_else(
        || actor.role().can_access_all_instances(),
        |id| actor.can(Permission::InstanceView, id),
    )
}

fn can_cancel(actor: &Actor, job: &JobSnapshot) -> bool {
    job.instance_id.map_or_else(
        || actor.role().can_access_all_instances(),
        |id| actor.can(Permission::InstanceUpdate, id),
    )
}

async fn list(State(state): State<AppState>, CurrentActor(actor): CurrentActor) -> Json<Vec<JobSnapshot>> {
    Json(
        state
            .jobs
            .list()
            .into_iter()
            .filter(|job| can_view(&actor, job))
            .collect(),
    )
}

#[derive(Debug, Serialize)]
struct JobDetail {
    #[serde(flatten)]
    job: JobSnapshot,
    log: Vec<String>,
}

async fn show(
    State(state): State<AppState>,
    CurrentActor(actor): CurrentActor,
    Path(job_id): Path<JobId>,
) -> Result<Json<JobDetail>, ApiError> {
    let job = state
        .jobs
        .get(job_id)
        .filter(|job| can_view(&actor, job))
        .ok_or_else(job_not_found)?;
    let log = state.jobs.log(job_id).unwrap_or_default();
    Ok(Json(JobDetail { job, log }))
}

async fn cancel(
    State(state): State<AppState>,
    CurrentActor(actor): CurrentActor,
    Path(job_id): Path<JobId>,
) -> Result<StatusCode, ApiError> {
    let job = state
        .jobs
        .get(job_id)
        .filter(|job| can_view(&actor, job))
        .ok_or_else(job_not_found)?;
    if !can_cancel(&actor, &job) {
        return Err(ApiError::forbidden(
            "missing_permission",
            "You can't cancel this job.",
        ));
    }
    if state.jobs.cancel(job_id) {
        Ok(StatusCode::ACCEPTED)
    } else {
        Err(ApiError::new(
            StatusCode::CONFLICT,
            "job_finished",
            "The job has already finished.",
        ))
    }
}

fn job_not_found() -> ApiError {
    ApiError::new(StatusCode::NOT_FOUND, "job_not_found", "No such job.")
}
