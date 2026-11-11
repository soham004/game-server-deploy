//! An instance's schedules: list, create, edit, delete, run now, and each one's run history.
//!
//! Anyone who can see the server sees its schedules; `schedules.manage` changes them. A
//! schedule runs with the permissions of whoever saved it last, so saving one also needs the
//! permissions its actions use (checked by the scheduler).

use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post, put},
};
use chrono::{DateTime, Utc};
use gsd_core::{
    audit::{AuditEntry, actions},
    ids::{InstanceId, ScheduleId},
    scheduler::{Schedule, SchedulePlan, ScheduleRun, describe},
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
    Router::new()
        .route("/instances/{instance_id}/schedules", get(list).post(create))
        .route(
            "/instances/{instance_id}/schedules/{schedule_id}",
            put(update).delete(delete),
        )
        .route(
            "/instances/{instance_id}/schedules/{schedule_id}/run",
            post(run_now),
        )
        .route("/instances/{instance_id}/schedules/{schedule_id}/runs", get(runs))
}

#[derive(Debug, Serialize)]
struct ScheduleDto {
    id: ScheduleId,
    plan: SchedulePlan,
    /// The trigger in words: "At 04:00 (Europe/London)".
    description: String,
    /// Whose permissions it runs with; `None` once that user is gone or disabled.
    run_as: Option<String>,
    created_at: DateTime<Utc>,
    last_run_at: Option<DateTime<Utc>>,
    next_run_at: Option<DateTime<Utc>>,
    last_run: Option<ScheduleRun>,
}

async fn dto(state: &AppState, instance: InstanceId, schedule: Schedule) -> Result<ScheduleDto, ApiError> {
    let run_as = match schedule.run_as {
        Some(id) => state
            .auth
            .actor_for(id)
            .await?
            .map(|actor| actor.user().username.to_string()),
        None => None,
    };
    let last_run = state
        .scheduler
        .runs(instance, schedule.id)
        .await?
        .into_iter()
        .next();
    Ok(ScheduleDto {
        id: schedule.id,
        description: describe(&schedule.plan.trigger),
        plan: schedule.plan,
        run_as,
        created_at: schedule.created_at,
        last_run_at: schedule.last_run_at,
        next_run_at: schedule.next_run_at,
        last_run,
    })
}

#[derive(Debug, Serialize)]
struct ScheduleList {
    schedules: Vec<ScheduleDto>,
}

async fn list(
    State(state): State<AppState>,
    auth: Authorized<perm::InstanceView>,
) -> Result<Json<ScheduleList>, ApiError> {
    let mut schedules = Vec::new();
    for schedule in state.scheduler.list(auth.instance_id).await? {
        schedules.push(dto(&state, auth.instance_id, schedule).await?);
    }
    Ok(Json(ScheduleList { schedules }))
}

async fn create(
    State(state): State<AppState>,
    auth: Authorized<perm::SchedulesManage>,
    ApiJson(plan): ApiJson<SchedulePlan>,
) -> Result<Response, ApiError> {
    let schedule = state
        .scheduler
        .create(auth.instance_id, plan, &auth.actor)
        .await?;
    audit(&state, &auth, actions::SCHEDULE_CREATED, &schedule).await;
    let body = dto(&state, auth.instance_id, schedule).await?;
    Ok((StatusCode::CREATED, Json(body)).into_response())
}

async fn update(
    State(state): State<AppState>,
    auth: Authorized<perm::SchedulesManage>,
    Path((_, schedule_id)): Path<(InstanceId, ScheduleId)>,
    ApiJson(plan): ApiJson<SchedulePlan>,
) -> Result<Json<ScheduleDto>, ApiError> {
    let schedule = state
        .scheduler
        .update(auth.instance_id, schedule_id, plan, &auth.actor)
        .await?;
    audit(&state, &auth, actions::SCHEDULE_UPDATED, &schedule).await;
    Ok(Json(dto(&state, auth.instance_id, schedule).await?))
}

async fn delete(
    State(state): State<AppState>,
    auth: Authorized<perm::SchedulesManage>,
    Path((_, schedule_id)): Path<(InstanceId, ScheduleId)>,
) -> Result<StatusCode, ApiError> {
    let schedule = state.scheduler.get(auth.instance_id, schedule_id).await?;
    state.scheduler.delete(auth.instance_id, schedule_id).await?;
    audit(&state, &auth, actions::SCHEDULE_DELETED, &schedule).await;
    Ok(StatusCode::NO_CONTENT)
}

async fn run_now(
    State(state): State<AppState>,
    auth: Authorized<perm::SchedulesManage>,
    Path((_, schedule_id)): Path<(InstanceId, ScheduleId)>,
) -> Result<StatusCode, ApiError> {
    let schedule = state.scheduler.get(auth.instance_id, schedule_id).await?;
    state
        .scheduler
        .run_now(auth.instance_id, schedule_id, &auth.actor)
        .await?;
    audit(&state, &auth, actions::SCHEDULE_RUN_REQUESTED, &schedule).await;
    Ok(StatusCode::ACCEPTED)
}

async fn runs(
    State(state): State<AppState>,
    auth: Authorized<perm::InstanceView>,
    Path((_, schedule_id)): Path<(InstanceId, ScheduleId)>,
) -> Result<Json<Vec<ScheduleRun>>, ApiError> {
    Ok(Json(state.scheduler.runs(auth.instance_id, schedule_id).await?))
}

async fn audit<P>(state: &AppState, auth: &Authorized<P>, action: &'static str, schedule: &Schedule) {
    let entry = AuditEntry::on_instance(auth.actor.id(), auth.instance_id, action)
        .with_details(json!({ "schedule": schedule.id, "name": schedule.plan.name }));
    state.audit.record(entry).await;
}
