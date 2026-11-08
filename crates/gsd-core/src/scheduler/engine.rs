//! The engine: one task that sleeps until a schedule is due, the schedules change, or a server
//! event arrives, then starts the runs that are due (each in its own task, see `run.rs`).
//!
//! - Time triggers fire when `next_run_at` passes; the following time is saved before the run.
//! - Event triggers fire from the event bus; "nobody online for …" from [`Activity`], once per
//!   empty stretch.
//! - At startup, runs a stopped app left open are closed, and schedules that came due while the
//!   app was off either run once (`catch_up`) or are recorded as skipped.
//! - When a server is deleted, its schedules go too.

use std::{collections::HashMap, time::Duration};

use chrono::{DateTime, Utc};
use tokio::{sync::broadcast::error::RecvError, task::JoinHandle};
use tokio_util::sync::CancellationToken;

use super::{
    Scheduler,
    model::{RunCause, RunOutcome, Schedule, ServerEvent, Trigger},
    trigger::next_time,
};
use crate::{
    events::{Event, EventKind},
    game::GameSignal,
    ids::ScheduleId,
    supervisor::ServerState,
};

/// The longest the engine sleeps without looking again (the clock may jump).
const MAX_SLEEP: Duration = Duration::from_secs(60);

/// "Nobody online" schedules that already ran for the empty stretch that began at this time.
type EmptyRuns = HashMap<ScheduleId, DateTime<Utc>>;

impl Scheduler {
    /// Starts the engine; it stops when `shutdown` is cancelled.
    pub fn start(&self, shutdown: CancellationToken) -> JoinHandle<()> {
        let scheduler = self.clone();
        tokio::spawn(async move { scheduler.run_engine(shutdown).await })
    }

    async fn run_engine(self, shutdown: CancellationToken) {
        // Subscribe before anything else, so no event between startup and the loop is lost.
        let mut events = self.inner.events.subscribe();
        self.recover(Utc::now()).await;
        let mut schedules = self.load_enabled().await;
        let mut empty_runs = EmptyRuns::new();
        loop {
            let wait = self.time_until_due(&schedules, &empty_runs, Utc::now());
            tokio::select! {
                () = shutdown.cancelled() => return,
                () = self.inner.wake.notified() => schedules = self.load_enabled().await,
                () = tokio::time::sleep(wait) => {
                    let now = Utc::now();
                    self.fire_due(&schedules, now).await;
                    self.fire_empty(&schedules, &mut empty_runs, now).await;
                    schedules = self.load_enabled().await;
                }
                event = events.recv() => match event {
                    Ok(event) => {
                        if self.on_event(&event, &schedules).await {
                            schedules = self.load_enabled().await;
                        }
                    }
                    // Missed some events (heavy console output); later ones still count.
                    Err(RecvError::Lagged(_)) => {}
                    Err(RecvError::Closed) => return,
                },
            }
        }
    }

    async fn load_enabled(&self) -> Vec<Schedule> {
        match self.inner.repo.list(None).await {
            Ok(schedules) => schedules.into_iter().filter(|s| s.plan.enabled).collect(),
            Err(error) => {
                tracing::error!(%error, "could not load the schedules");
                Vec::new()
            }
        }
    }

    /// After (re)starting the app: close runs it left open; handle times missed while it was off.
    async fn recover(&self, now: DateTime<Utc>) {
        if let Err(error) = self.inner.repo.close_interrupted_runs(now).await {
            tracing::error!(%error, "could not close interrupted schedule runs");
        }
        for schedule in self.load_enabled().await {
            let missed = schedule.next_run_at.is_some_and(|due| due <= now);
            if !missed {
                continue;
            }
            let next = next_after(&schedule, now);
            if schedule.plan.catch_up {
                self.save_times(&schedule, Some(now), next).await;
                self.spawn_run(schedule, RunCause::CatchUp);
            } else {
                self.save_times(&schedule, schedule.last_run_at, next).await;
                self.record_missed(&schedule, now).await;
            }
        }
    }

    /// How long until the next time trigger or "nobody online" deadline.
    fn time_until_due(&self, schedules: &[Schedule], empty_runs: &EmptyRuns, now: DateTime<Utc>) -> Duration {
        let time_deadlines = schedules.iter().filter_map(|s| s.next_run_at);
        let empty_deadlines = schedules.iter().filter_map(|schedule| {
            self.empty_deadline(schedule, empty_runs)
                .map(|(_, deadline)| deadline)
        });
        time_deadlines
            .chain(empty_deadlines)
            .min()
            .map_or(MAX_SLEEP, |deadline| {
                (deadline - now).to_std().unwrap_or(Duration::ZERO)
            })
            .min(MAX_SLEEP)
    }

    async fn fire_due(&self, schedules: &[Schedule], now: DateTime<Utc>) {
        for schedule in schedules {
            if schedule.next_run_at.is_some_and(|due| due <= now) {
                let next = next_after(schedule, now);
                self.save_times(schedule, Some(now), next).await;
                self.spawn_run(schedule.clone(), RunCause::Trigger);
            }
        }
    }

    async fn fire_empty(&self, schedules: &[Schedule], empty_runs: &mut EmptyRuns, now: DateTime<Utc>) {
        for schedule in schedules {
            let Some((since, deadline)) = self.empty_deadline(schedule, empty_runs) else {
                continue;
            };
            if deadline <= now {
                empty_runs.insert(schedule.id, since);
                self.save_times(schedule, Some(now), None).await;
                self.spawn_run(schedule.clone(), RunCause::Trigger);
            }
        }
    }

    /// For a "nobody online for …" schedule whose server is empty and that hasn't run for this
    /// empty stretch yet: when the stretch began, and when it fires.
    fn empty_deadline(
        &self,
        schedule: &Schedule,
        empty_runs: &EmptyRuns,
    ) -> Option<(DateTime<Utc>, DateTime<Utc>)> {
        let empty_since = self
            .inner
            .activity
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .empty_since(schedule.instance_id);
        empty_deadline(
            &schedule.plan.trigger,
            empty_since,
            empty_runs.get(&schedule.id).copied(),
        )
    }

    /// Learns from the event and starts the schedules it triggers. Returns whether the
    /// schedules must be reloaded (a server was deleted).
    async fn on_event(&self, event: &Event, schedules: &[Schedule]) -> bool {
        let now = Utc::now();
        self.inner
            .activity
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .observe(event, now);
        let Some(instance) = event.instance_id else {
            return false;
        };
        if matches!(event.kind, EventKind::InstanceDeleted) {
            if let Err(error) = self.inner.repo.delete_for_instance(instance).await {
                tracing::error!(%error, %instance, "could not delete the schedules of a deleted server");
            }
            return true;
        }
        let Some(happened) = server_event(&event.kind) else {
            return false;
        };
        for schedule in schedules.iter().filter(|s| s.instance_id == instance) {
            if matches!(&schedule.plan.trigger, Trigger::Event { event } if *event == happened) {
                self.save_times(schedule, Some(now), None).await;
                self.spawn_run(schedule.clone(), RunCause::Trigger);
            }
        }
        false
    }

    async fn save_times(
        &self,
        schedule: &Schedule,
        last: Option<DateTime<Utc>>,
        next: Option<DateTime<Utc>>,
    ) {
        if let Err(error) = self.inner.repo.set_times(schedule.id, last, next).await {
            tracing::error!(%error, schedule = %schedule.id, "could not save a schedule's times");
        }
    }

    /// Starts a run in its own task: runs can take minutes (restart warnings, backups) and the
    /// engine must keep watching. The run records its own outcome and logs its own errors.
    fn spawn_run(&self, schedule: Schedule, cause: RunCause) {
        let scheduler = self.clone();
        let previous = schedule.last_run_at;
        tokio::spawn(async move { scheduler.dispatch(schedule, previous, cause).await });
    }

    async fn record_missed(&self, schedule: &Schedule, now: DateTime<Utc>) {
        let repo = &self.inner.repo;
        let recorded = async {
            let run = repo.start_run(schedule.id, RunCause::CatchUp, now).await?;
            let message = "Skipped: it was due while the app was off (catching up is off for this schedule).";
            repo.finish_run(run, RunOutcome::Skipped, Some(message), now)
                .await
        };
        if let Err(error) = recorded.await {
            tracing::error!(%error, schedule = %schedule.id, "could not record a missed run");
        }
    }
}

/// When the schedule's trigger fires after `now` (none for a trigger saved before it became invalid).
fn next_after(schedule: &Schedule, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
    next_time(&schedule.plan.trigger, now).unwrap_or_else(|error| {
        tracing::error!(%error, schedule = %schedule.id, "a saved schedule has an invalid trigger");
        None
    })
}

/// The server event a bus event means, if any.
fn server_event(kind: &EventKind) -> Option<ServerEvent> {
    match kind {
        EventKind::State(ServerState::Running { .. }) => Some(ServerEvent::Started),
        EventKind::State(ServerState::Stopped { .. }) => Some(ServerEvent::Stopped),
        EventKind::State(ServerState::Crashed { .. }) => Some(ServerEvent::Crashed),
        EventKind::Signal(GameSignal::PlayerJoined { .. }) => Some(ServerEvent::PlayerJoined),
        _ => None,
    }
}

/// When a "nobody online for …" trigger fires: `secs` after the server became empty, once per
/// empty stretch (`ran_for` is the start of the stretch it last ran for). Returns the stretch's
/// start and the deadline; `None` for other triggers, a server with players, or one already run.
fn empty_deadline(
    trigger: &Trigger,
    empty_since: Option<DateTime<Utc>>,
    ran_for: Option<DateTime<Utc>>,
) -> Option<(DateTime<Utc>, DateTime<Utc>)> {
    let Trigger::Event {
        event: ServerEvent::EmptyFor { secs },
    } = trigger
    else {
        return None;
    };
    let since = empty_since?;
    if ran_for == Some(since) {
        return None;
    }
    let deadline = since + chrono::TimeDelta::seconds(i64::try_from(*secs).unwrap_or(i64::MAX));
    Some((since, deadline))
}

#[cfg(test)]
mod tests {
    use chrono::TimeDelta;

    use super::*;

    #[test]
    fn nobody_online_fires_once_per_empty_stretch() {
        let quarter_hour = Trigger::Event {
            event: ServerEvent::EmptyFor { secs: 900 },
        };
        let emptied = Utc::now();
        let due = emptied + TimeDelta::minutes(15);

        assert_eq!(
            empty_deadline(&quarter_hour, Some(emptied), None),
            Some((emptied, due))
        );
        assert_eq!(empty_deadline(&quarter_hour, None, None), None, "players are on");
        assert_eq!(
            empty_deadline(&quarter_hour, Some(emptied), Some(emptied)),
            None,
            "already ran"
        );
        let emptied_again = emptied + TimeDelta::hours(1);
        assert_eq!(
            empty_deadline(&quarter_hour, Some(emptied_again), Some(emptied)),
            Some((emptied_again, emptied_again + TimeDelta::minutes(15))),
            "a new stretch counts again"
        );
        let started = Trigger::Event {
            event: ServerEvent::Started,
        };
        assert_eq!(empty_deadline(&started, Some(emptied), None), None);
    }

    #[test]
    fn bus_events_map_to_server_events() {
        let now = Utc::now();
        assert_eq!(
            server_event(&EventKind::State(ServerState::Running {
                since: now,
                pid: None
            })),
            Some(ServerEvent::Started)
        );
        assert_eq!(
            server_event(&EventKind::State(ServerState::initial())),
            Some(ServerEvent::Stopped)
        );
        assert_eq!(
            server_event(&EventKind::Signal(GameSignal::PlayerJoined {
                name: "Steve".into()
            })),
            Some(ServerEvent::PlayerJoined)
        );
        assert_eq!(server_event(&EventKind::Signal(GameSignal::Saved)), None);
        assert_eq!(
            server_event(&EventKind::State(ServerState::Starting {
                since: now,
                pid: None
            })),
            None
        );
    }
}
