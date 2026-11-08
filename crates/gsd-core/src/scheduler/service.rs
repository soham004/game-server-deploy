//! The scheduler's API: list, create, edit, delete, run now, and the run history. Changes wake
//! the engine (`engine.rs`) so it picks up new times at once; a run is carried out by
//! `run.rs`.

use std::sync::{Arc, Mutex};

use chrono::Utc;
use sqlx::SqlitePool;
use tokio::sync::Notify;

use super::{
    activity::Activity,
    model::{RunCause, Schedule, SchedulePlan, ScheduleRun},
    repo::ScheduleRepo,
    trigger::{TriggerError, next_time},
    validate::{PlanError, validate},
};
use crate::{
    auth::{Actor, AuthService},
    backups::BackupService,
    events::{EventBus, EventKind},
    ids::{InstanceId, ScheduleId},
    instances::{InstanceError, InstanceService},
    jobs::JobRunner,
    store::RepoError,
};

/// Runs shown in a schedule's history.
const HISTORY_LENGTH: i64 = 20;

#[derive(Debug, thiserror::Error)]
pub enum ScheduleError {
    #[error("schedule not found")]
    NotFound,
    #[error(transparent)]
    Plan(#[from] PlanError),
    #[error(transparent)]
    Trigger(#[from] TriggerError),
    #[error(transparent)]
    Instance(#[from] InstanceError),
    #[error(transparent)]
    Repo(#[from] RepoError),
    /// The author can't do one of the actions themselves (it would run with their permissions).
    #[error("you need the \"{0}\" permission for one of the actions")]
    MissingPermission(String),
}

#[derive(Debug, Clone)]
pub struct Scheduler {
    pub(super) inner: Arc<Inner>,
}

#[derive(Debug)]
pub(super) struct Inner {
    pub(super) repo: ScheduleRepo,
    pub(super) instances: InstanceService,
    pub(super) backups: BackupService,
    pub(super) auth: AuthService,
    pub(super) jobs: JobRunner,
    pub(super) events: EventBus,
    /// Tells the engine the schedules changed.
    pub(super) wake: Notify,
    pub(super) activity: Mutex<Activity>,
}

impl Scheduler {
    pub fn new(
        pool: SqlitePool,
        instances: InstanceService,
        backups: BackupService,
        auth: AuthService,
        jobs: JobRunner,
        events: EventBus,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                repo: ScheduleRepo::new(pool),
                instances,
                backups,
                auth,
                jobs,
                events,
                wake: Notify::new(),
                activity: Mutex::default(),
            }),
        }
    }

    /// An instance's schedules, oldest first.
    pub async fn list(&self, instance: InstanceId) -> Result<Vec<Schedule>, ScheduleError> {
        self.inner.instances.get(instance).await?;
        Ok(self.inner.repo.list(Some(instance)).await?)
    }

    /// One schedule of `instance`.
    pub async fn get(&self, instance: InstanceId, id: ScheduleId) -> Result<Schedule, ScheduleError> {
        self.inner
            .repo
            .get(id)
            .await?
            .filter(|schedule| schedule.instance_id == instance)
            .ok_or(ScheduleError::NotFound)
    }

    /// Checks and saves a new schedule that runs as `by`.
    pub async fn create(
        &self,
        instance: InstanceId,
        plan: SchedulePlan,
        by: &Actor,
    ) -> Result<Schedule, ScheduleError> {
        self.inner.instances.get(instance).await?;
        let now = Utc::now();
        let plan = tidy(plan);
        validate(&plan, now)?;
        check_author(&plan, by, instance)?;
        let schedule = Schedule {
            id: ScheduleId::generate(),
            instance_id: instance,
            next_run_at: if plan.enabled {
                next_time(&plan.trigger, now)?
            } else {
                None
            },
            plan,
            run_as: Some(by.id()),
            created_at: now,
            last_run_at: None,
        };
        self.inner.repo.insert(&schedule).await?;
        self.changed(instance);
        Ok(schedule)
    }

    /// Replaces the plan. The schedule then runs as `by`, so nobody can borrow someone else's
    /// permissions by editing their schedule. An unchanged trigger keeps its next time.
    pub async fn update(
        &self,
        instance: InstanceId,
        id: ScheduleId,
        plan: SchedulePlan,
        by: &Actor,
    ) -> Result<Schedule, ScheduleError> {
        let current = self.get(instance, id).await?;
        let now = Utc::now();
        let plan = tidy(plan);
        validate(&plan, now)?;
        check_author(&plan, by, instance)?;
        let next_run_at = match (
            plan.enabled,
            current.plan.enabled && current.plan.trigger == plan.trigger,
        ) {
            (false, _) => None,
            (true, true) => current.next_run_at,
            (true, false) => next_time(&plan.trigger, now)?,
        };
        self.inner
            .repo
            .update_plan(id, &plan, by.id(), next_run_at)
            .await?;
        self.changed(instance);
        self.get(instance, id).await
    }

    pub async fn delete(&self, instance: InstanceId, id: ScheduleId) -> Result<(), ScheduleError> {
        self.get(instance, id).await?;
        self.inner.repo.delete(id).await?;
        self.changed(instance);
        Ok(())
    }

    /// Runs the actions now, whatever the conditions say. Returns at once; the run shows up in
    /// the history and the job tray.
    pub async fn run_now(&self, instance: InstanceId, id: ScheduleId) -> Result<(), ScheduleError> {
        let schedule = self.get(instance, id).await?;
        let scheduler = self.clone();
        tokio::spawn(async move {
            let previous = schedule.last_run_at;
            scheduler.dispatch(schedule, previous, RunCause::Manual).await;
        });
        Ok(())
    }

    /// The most recent runs of a schedule, newest first.
    pub async fn runs(
        &self,
        instance: InstanceId,
        id: ScheduleId,
    ) -> Result<Vec<ScheduleRun>, ScheduleError> {
        self.get(instance, id).await?;
        Ok(self.inner.repo.runs(id, HISTORY_LENGTH).await?)
    }
}

impl Scheduler {
    /// Wakes the engine (times may have changed) and tells open pages.
    fn changed(&self, instance: InstanceId) {
        self.inner.wake.notify_one();
        self.inner
            .events
            .publish(Some(instance), EventKind::SchedulesChanged);
    }
}

/// The author must be able to do every action themselves: the schedule runs as them.
fn check_author(plan: &SchedulePlan, author: &Actor, instance: InstanceId) -> Result<(), ScheduleError> {
    match plan
        .actions
        .iter()
        .map(super::model::Action::permission)
        .find(|permission| !author.can(*permission, instance))
    {
        Some(permission) => Err(ScheduleError::MissingPermission(super::model::permission_name(
            permission,
        ))),
        None => Ok(()),
    }
}

/// Trims what people type.
fn tidy(mut plan: SchedulePlan) -> SchedulePlan {
    plan.name = plan.name.trim().to_owned();
    plan
}
