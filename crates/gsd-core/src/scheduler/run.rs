//! One run of a schedule: record it, check it may run (the server exists, the user it runs as
//! still holds every permission its actions need, the conditions hold), then do the actions in
//! a job that holds the server's lock, and record how it ended.
//!
//! The lock keeps runs from overlapping each other or an install, backup or restore: a run that
//! finds the server busy is recorded as skipped and does nothing.

use std::sync::Arc;

use chrono::{DateTime, Utc};

use super::{
    Scheduler,
    actions::{ActionContext, ActionFailed},
    conditions::{Situation, unmet},
    model::{Action, OnError, RunCause, RunOutcome, Schedule, permission_name},
};
use crate::{
    game::GameAdapter,
    ids::UserId,
    instances::Instance,
    jobs::{JobFailure, JobSpec},
    supervisor::ServerState,
};

/// Why a run stopped before doing anything.
enum Halt {
    Skipped(String),
    Failed(String),
}

/// What a run needs once it may go ahead.
struct Ready {
    instance: Instance,
    adapter: Arc<dyn GameAdapter>,
    run_as: UserId,
}

impl Scheduler {
    /// Runs `schedule` (its trigger fired, or someone pressed "Run now"). `previous_run` is when
    /// it last ran before this, for "somebody played since the last run".
    pub(super) async fn dispatch(
        &self,
        schedule: Schedule,
        previous_run: Option<DateTime<Utc>>,
        cause: RunCause,
    ) {
        let repo = &self.inner.repo;
        let run = match repo.start_run(schedule.id, cause, Utc::now()).await {
            Ok(run) => run,
            Err(error) => {
                tracing::error!(%error, schedule = %schedule.id, "could not record a schedule run");
                return;
            }
        };
        match self.check(&schedule, previous_run, cause).await {
            Ok(ready) => self.start_job(schedule, ready, run).await,
            Err(Halt::Skipped(reason)) => {
                self.finish(&schedule, run, RunOutcome::Skipped, Some(reason))
                    .await;
            }
            Err(Halt::Failed(reason)) => {
                self.finish(&schedule, run, RunOutcome::Failed, Some(reason))
                    .await;
            }
        }
    }

    async fn check(
        &self,
        schedule: &Schedule,
        previous_run: Option<DateTime<Utc>>,
        cause: RunCause,
    ) -> Result<Ready, Halt> {
        let instances = &self.inner.instances;
        let instance = instances
            .get(schedule.instance_id)
            .await
            .map_err(|_| Halt::Failed("the server no longer exists".into()))?;
        let run_as = self.check_permissions(schedule).await?;
        if cause != RunCause::Manual {
            let situation = self.situation(&instance, previous_run);
            if let Some(reason) = unmet(&schedule.plan.conditions, &situation) {
                return Err(Halt::Skipped(format!("Skipped: {reason}.")));
            }
        }
        let adapter = instances
            .adapter(&instance)
            .map_err(|error| Halt::Failed(error.to_string()))?;
        Ok(Ready {
            instance,
            adapter,
            run_as,
        })
    }

    /// The user it runs as must still exist, be enabled, and hold what every action needs.
    async fn check_permissions(&self, schedule: &Schedule) -> Result<UserId, Halt> {
        let gone = || {
            Halt::Failed(
                "The user who saved this schedule was deleted or disabled. Save it again to run it as yourself.".into(),
            )
        };
        let run_as = schedule.run_as.ok_or_else(gone)?;
        let actor = self
            .inner
            .auth
            .actor_for(run_as)
            .await
            .map_err(|error| Halt::Failed(error.to_string()))?
            .ok_or_else(gone)?;
        let missing = schedule
            .plan
            .actions
            .iter()
            .map(Action::permission)
            .find(|permission| !actor.can(*permission, schedule.instance_id));
        if let Some(permission) = missing {
            return Err(Halt::Failed(format!(
                "{} no longer has the \"{}\" permission this schedule needs.",
                actor.user().username,
                permission_name(permission)
            )));
        }
        Ok(run_as)
    }

    fn situation(&self, instance: &Instance, previous_run: Option<DateTime<Utc>>) -> Situation {
        let id = instance.id();
        let running = matches!(self.inner.instances.state(id), ServerState::Running { .. });
        let activity = self
            .inner
            .activity
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Situation {
            running,
            players_online: activity.players_online(id),
            played_since_last_run: activity.played_since(id, previous_run),
            now: Utc::now(),
        }
    }

    async fn start_job(&self, schedule: Schedule, ready: Ready, run: i64) {
        let spec = JobSpec {
            kind: "schedule.run",
            title: schedule.plan.name.clone(),
            instance_id: Some(schedule.instance_id),
        };
        let scheduler = self.clone();
        let finished = schedule.clone();
        let spawned = self.inner.jobs.spawn(spec, move |job| async move {
            let context = ActionContext {
                instances: &scheduler.inner.instances,
                backups: &scheduler.inner.backups,
                instance: &ready.instance,
                adapter: ready.adapter.clone(),
                author: format!("schedule {}", schedule.plan.name),
                run_as: ready.run_as,
                job: &job,
            };
            let failures = perform_all(&context, &schedule).await;
            let (outcome, message) = match failures.as_slice() {
                [] => (RunOutcome::Succeeded, None),
                failures => (RunOutcome::Failed, Some(failures.join("; "))),
            };
            scheduler.finish(&schedule, run, outcome, message.clone()).await;
            message.map_or(Ok(()), |message| Err(JobFailure::msg(message)))
        });
        if spawned.is_err() {
            let reason = "Skipped: another operation (an install, backup or other schedule) was running on the server.";
            self.finish(&finished, run, RunOutcome::Skipped, Some(reason.into()))
                .await;
        }
    }

    /// Records how the run ended and tells open pages.
    async fn finish(&self, schedule: &Schedule, run: i64, outcome: RunOutcome, message: Option<String>) {
        if let Err(error) = self
            .inner
            .repo
            .finish_run(run, outcome, message.as_deref(), Utc::now())
            .await
        {
            tracing::error!(%error, run, "could not record how a schedule run ended");
        }
        self.inner.events.publish(
            Some(schedule.instance_id),
            crate::events::EventKind::SchedulesChanged,
        );
    }
}

/// Does the actions in order; returns what failed. With `on_error: stop`, the first failure
/// ends the run.
async fn perform_all(context: &ActionContext<'_>, schedule: &Schedule) -> Vec<String> {
    let mut failures = Vec::new();
    let total = schedule.plan.actions.len();
    for (index, action) in schedule.plan.actions.iter().enumerate() {
        context
            .job
            .step(format!("{} ({} of {total})", action_label(action), index + 1));
        if let Err(ActionFailed(reason)) = context.perform(action).await {
            context.job.log(format!("Failed: {reason}"));
            failures.push(format!(
                "action {} ({}): {reason}",
                index + 1,
                action_label(action)
            ));
            if schedule.plan.on_error == OnError::Stop || context.job.is_cancelled() {
                break;
            }
        }
    }
    failures
}

fn action_label(action: &Action) -> &'static str {
    match action {
        Action::Command { .. } => "Command",
        Action::Broadcast { .. } => "Message",
        Action::Restart { .. } => "Restart",
        Action::Stop { .. } => "Stop",
        Action::Start => "Start",
        Action::Backup { .. } => "Backup",
        Action::Wait { .. } => "Wait",
    }
}
