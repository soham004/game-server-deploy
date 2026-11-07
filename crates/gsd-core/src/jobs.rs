//! Background jobs: anything slow (installs, Java downloads, later backups and updates).
//!
//! A job is an async task with an id, a title, progress, a log and a cancellation token. Jobs
//! that touch an instance take that instance's lock, so two conflicting operations (say, a
//! reinstall and a start) can't run at once. Every change is published on the event bus.

use std::{
    collections::{HashMap, HashSet, VecDeque},
    error::Error as StdError,
    fmt,
    future::Future,
    panic::AssertUnwindSafe,
    sync::{Arc, Mutex, MutexGuard},
    time::{Duration, Instant},
};

use chrono::{DateTime, Utc};
use futures_util::FutureExt;
use serde::Serialize;
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

use crate::{
    events::{EventBus, EventKind},
    ids::{InstanceId, JobId},
};

/// Finished jobs kept for the UI's job list.
const HISTORY_LIMIT: usize = 50;
/// Lines kept per job log.
const LOG_LIMIT: usize = 500;
/// Progress events are throttled to at most one per interval (step changes always go out).
const PROGRESS_INTERVAL: Duration = Duration::from_millis(200);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum JobStatus {
    Running,
    Succeeded,
    Failed,
    Cancelled,
}

impl JobStatus {
    pub fn is_finished(self) -> bool {
        !matches!(self, Self::Running)
    }
}

/// The public view of a job.
#[derive(Debug, Clone, Serialize)]
pub struct JobSnapshot {
    pub id: JobId,
    pub instance_id: Option<InstanceId>,
    /// Machine-readable kind, e.g. `"instance.create"`.
    pub kind: &'static str,
    pub title: String,
    pub status: JobStatus,
    /// 0.0–1.0 when known.
    pub progress: Option<f32>,
    pub step: Option<String>,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub error: Option<String>,
}

/// What to run.
#[derive(Debug, Clone)]
pub struct JobSpec {
    pub kind: &'static str,
    pub title: String,
    /// When set, the job holds this instance's lock for its whole run.
    pub instance_id: Option<InstanceId>,
}

/// Why a job failed: a message with its cause chain, e.g. "download failed: connection reset".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobFailure(String);

impl JobFailure {
    pub fn msg(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for JobFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl<E: StdError> From<E> for JobFailure {
    fn from(error: E) -> Self {
        Self(error_chain(&error))
    }
}

/// `error: cause: root cause`.
pub fn error_chain(error: &dyn StdError) -> String {
    let mut message = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        let text = cause.to_string();
        if !message.contains(&text) {
            message.push_str(": ");
            message.push_str(&text);
        }
        source = cause.source();
    }
    message
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum JobError {
    #[error("another operation is already running on this instance")]
    InstanceBusy,
}

/// Handed to the running task.
#[derive(Debug, Clone)]
pub struct JobContext {
    id: JobId,
    cancel: CancellationToken,
    runner: JobRunner,
    last_progress: Arc<Mutex<Option<Instant>>>,
}

impl JobContext {
    pub fn id(&self) -> JobId {
        self.id
    }

    pub fn cancel_token(&self) -> &CancellationToken {
        &self.cancel
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancel.is_cancelled()
    }

    /// Returns `Err` when the job was cancelled, for use with `?` between steps.
    pub fn check_cancelled(&self) -> Result<(), JobFailure> {
        if self.is_cancelled() {
            Err(JobFailure::msg("cancelled"))
        } else {
            Ok(())
        }
    }

    /// Starts a new named step (resets progress) and logs it.
    pub fn step(&self, step: impl Into<String>) {
        let step = step.into();
        self.log(&step);
        self.runner.update(self.id, |snapshot| {
            snapshot.step = Some(step);
            snapshot.progress = None;
        });
    }

    /// Reports progress of the current step (0.0–1.0). Throttled.
    pub fn progress(&self, fraction: f32) {
        let mut last = lock(&self.last_progress);
        let due = last.is_none_or(|at| at.elapsed() >= PROGRESS_INTERVAL) || fraction >= 1.0;
        if !due {
            return;
        }
        *last = Some(Instant::now());
        drop(last);
        self.runner.update(self.id, |snapshot| {
            snapshot.progress = Some(fraction.clamp(0.0, 1.0));
        });
    }

    /// Appends a line to the job log (visible in the UI).
    pub fn log(&self, line: impl AsRef<str>) {
        self.runner.append_log(self.id, line.as_ref());
    }
}

/// Runs and tracks jobs. Cheap to clone.
#[derive(Debug, Clone)]
pub struct JobRunner {
    inner: Arc<Inner>,
}

#[derive(Debug)]
struct Inner {
    events: EventBus,
    state: Mutex<State>,
}

#[derive(Debug, Default)]
struct State {
    jobs: HashMap<JobId, Entry>,
    finished_order: VecDeque<JobId>,
    busy_instances: HashSet<InstanceId>,
}

#[derive(Debug)]
struct Entry {
    snapshot: JobSnapshot,
    cancel: CancellationToken,
    log: VecDeque<String>,
    done: watch::Sender<bool>,
}

impl JobRunner {
    pub fn new(events: EventBus) -> Self {
        Self {
            inner: Arc::new(Inner {
                events,
                state: Mutex::new(State::default()),
            }),
        }
    }

    /// Starts `task` in the background. Fails immediately if the instance is busy.
    pub fn spawn<F, Fut>(&self, spec: JobSpec, task: F) -> Result<JobId, JobError>
    where
        F: FnOnce(JobContext) -> Fut + Send + 'static,
        Fut: Future<Output = Result<(), JobFailure>> + Send + 'static,
    {
        let id = JobId::generate();
        let cancel = CancellationToken::new();
        let snapshot = JobSnapshot {
            id,
            instance_id: spec.instance_id,
            kind: spec.kind,
            title: spec.title,
            status: JobStatus::Running,
            progress: None,
            step: None,
            started_at: Utc::now(),
            finished_at: None,
            error: None,
        };

        {
            let mut state = self.state();
            if let Some(instance) = spec.instance_id
                && !state.busy_instances.insert(instance)
            {
                return Err(JobError::InstanceBusy);
            }
            let (done, _) = watch::channel(false);
            let entry = Entry {
                snapshot: snapshot.clone(),
                cancel: cancel.clone(),
                log: VecDeque::new(),
                done,
            };
            state.jobs.insert(id, entry);
        }
        self.publish(&snapshot);

        let context = JobContext {
            id,
            cancel,
            runner: self.clone(),
            last_progress: Arc::default(),
        };
        let runner = self.clone();
        tokio::spawn(async move {
            let outcome = AssertUnwindSafe(task(context)).catch_unwind().await;
            let result = outcome.unwrap_or_else(|_| Err(JobFailure::msg("the job crashed unexpectedly")));
            runner.finish(id, result);
        });
        Ok(id)
    }

    /// True while a job holds this instance's lock.
    pub fn is_instance_busy(&self, instance: InstanceId) -> bool {
        self.state().busy_instances.contains(&instance)
    }

    /// Requests cancellation. Returns false if the job is unknown or already finished.
    pub fn cancel(&self, id: JobId) -> bool {
        let state = self.state();
        match state.jobs.get(&id) {
            Some(entry) if !entry.snapshot.status.is_finished() => {
                entry.cancel.cancel();
                true
            }
            _ => false,
        }
    }

    pub fn get(&self, id: JobId) -> Option<JobSnapshot> {
        self.state().jobs.get(&id).map(|entry| entry.snapshot.clone())
    }

    pub fn log(&self, id: JobId) -> Option<Vec<String>> {
        self.state()
            .jobs
            .get(&id)
            .map(|entry| entry.log.iter().cloned().collect())
    }

    /// Running jobs first, then recent history, newest first.
    pub fn list(&self) -> Vec<JobSnapshot> {
        let mut jobs: Vec<JobSnapshot> = self.state().jobs.values().map(|e| e.snapshot.clone()).collect();
        jobs.sort_by(|a, b| {
            a.status
                .is_finished()
                .cmp(&b.status.is_finished())
                .then(b.started_at.cmp(&a.started_at))
        });
        jobs
    }

    /// Waits until the job finishes and returns its final snapshot.
    pub async fn wait(&self, id: JobId) -> Option<JobSnapshot> {
        let mut done = self.state().jobs.get(&id)?.done.subscribe();
        let _ = done.wait_for(|finished| *finished).await;
        self.get(id)
    }

    fn finish(&self, id: JobId, result: Result<(), JobFailure>) {
        let (snapshot, evicted) = {
            let mut state = self.state();
            let Some(entry) = state.jobs.get_mut(&id) else {
                return;
            };
            entry.snapshot.finished_at = Some(Utc::now());
            match result {
                Ok(()) => {
                    entry.snapshot.status = JobStatus::Succeeded;
                    entry.snapshot.progress = Some(1.0);
                }
                Err(_) if entry.cancel.is_cancelled() => entry.snapshot.status = JobStatus::Cancelled,
                Err(failure) => {
                    entry.snapshot.status = JobStatus::Failed;
                    push_log(&mut entry.log, &format!("failed: {failure}"));
                    entry.snapshot.error = Some(failure.0);
                }
            }
            let snapshot = entry.snapshot.clone();
            let _ = entry.done.send(true);
            if let Some(instance) = snapshot.instance_id {
                state.busy_instances.remove(&instance);
            }
            state.finished_order.push_back(id);
            (snapshot, Self::trim_history(&mut state))
        };
        self.publish(&snapshot);
        for old in evicted {
            self.inner.events.publish(None, EventKind::JobRemoved(old));
        }
    }

    fn trim_history(state: &mut MutexGuard<'_, State>) -> Vec<JobId> {
        let mut evicted = Vec::new();
        while state.finished_order.len() > HISTORY_LIMIT {
            if let Some(old) = state.finished_order.pop_front() {
                state.jobs.remove(&old);
                evicted.push(old);
            }
        }
        evicted
    }

    fn update(&self, id: JobId, change: impl FnOnce(&mut JobSnapshot)) {
        let snapshot = {
            let mut state = self.state();
            let Some(entry) = state.jobs.get_mut(&id) else {
                return;
            };
            change(&mut entry.snapshot);
            entry.snapshot.clone()
        };
        self.publish(&snapshot);
    }

    fn append_log(&self, id: JobId, line: &str) {
        if let Some(entry) = self.state().jobs.get_mut(&id) {
            push_log(&mut entry.log, line);
        }
    }

    fn publish(&self, snapshot: &JobSnapshot) {
        self.inner
            .events
            .publish(snapshot.instance_id, EventKind::Job(snapshot.clone()));
    }

    fn state(&self) -> MutexGuard<'_, State> {
        lock(&self.inner.state)
    }
}

fn push_log(log: &mut VecDeque<String>, line: &str) {
    if log.len() == LOG_LIMIT {
        log.pop_front();
    }
    log.push_back(format!("[{}] {line}", Utc::now().format("%H:%M:%S")));
}

/// Locks a std mutex, recovering from poisoning (a panicked holder can't corrupt this state
/// in a way that matters more than losing the whole job list).
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(instance: Option<InstanceId>) -> JobSpec {
        JobSpec {
            kind: "test",
            title: "Test job".into(),
            instance_id: instance,
        }
    }

    #[tokio::test]
    async fn successful_job_reports_progress_and_finishes() {
        let runner = JobRunner::new(EventBus::new());
        let id = runner
            .spawn(spec(None), |ctx| async move {
                ctx.step("Downloading");
                ctx.progress(1.0);
                ctx.log("done downloading");
                Ok(())
            })
            .unwrap();

        let finished = runner.wait(id).await.unwrap();
        assert_eq!(finished.status, JobStatus::Succeeded);
        assert_eq!(finished.step.as_deref(), Some("Downloading"));
        assert!(finished.finished_at.is_some());
        let log = runner.log(id).unwrap();
        assert!(log.iter().any(|line| line.ends_with("done downloading")));
    }

    #[tokio::test]
    async fn failures_keep_the_cause_chain() {
        let runner = JobRunner::new(EventBus::new());
        let id = runner
            .spawn(spec(None), |_| async {
                let io = std::io::Error::new(std::io::ErrorKind::NotFound, "server.jar missing");
                Err(JobFailure::from(io))
            })
            .unwrap();
        let finished = runner.wait(id).await.unwrap();
        assert_eq!(finished.status, JobStatus::Failed);
        assert_eq!(finished.error.as_deref(), Some("server.jar missing"));
    }

    #[tokio::test]
    async fn panics_become_failures() {
        let runner = JobRunner::new(EventBus::new());
        let id = runner.spawn(spec(None), |_| async { panic!("boom") }).unwrap();
        let finished = runner.wait(id).await.unwrap();
        assert_eq!(finished.status, JobStatus::Failed);
        assert!(finished.error.unwrap().contains("crashed"));
    }

    #[tokio::test]
    async fn cancellation_is_cooperative() {
        let runner = JobRunner::new(EventBus::new());
        let id = runner
            .spawn(spec(None), |ctx| async move {
                ctx.cancel_token().cancelled().await;
                ctx.check_cancelled()
            })
            .unwrap();
        assert!(runner.cancel(id));
        let finished = runner.wait(id).await.unwrap();
        assert_eq!(finished.status, JobStatus::Cancelled);
        assert!(!runner.cancel(id), "finished jobs can't be cancelled");
    }

    #[tokio::test]
    async fn one_job_per_instance_at_a_time() {
        let runner = JobRunner::new(EventBus::new());
        let instance = InstanceId::generate();
        let (release, wait_release) = tokio::sync::oneshot::channel::<()>();
        let first = runner
            .spawn(spec(Some(instance)), |_| async move {
                let _ = wait_release.await;
                Ok(())
            })
            .unwrap();

        assert!(runner.is_instance_busy(instance));
        assert_eq!(
            runner.spawn(spec(Some(instance)), |_| async { Ok(()) }),
            Err(JobError::InstanceBusy)
        );
        // Other instances are unaffected.
        assert!(
            runner
                .spawn(spec(Some(InstanceId::generate())), |_| async { Ok(()) })
                .is_ok()
        );

        release.send(()).unwrap();
        runner.wait(first).await;
        assert!(!runner.is_instance_busy(instance));
        assert!(runner.spawn(spec(Some(instance)), |_| async { Ok(()) }).is_ok());
    }

    #[tokio::test]
    async fn events_are_published() {
        let bus = EventBus::new();
        let mut events = bus.subscribe();
        let runner = JobRunner::new(bus);
        let id = runner.spawn(spec(None), |_| async { Ok(()) }).unwrap();
        runner.wait(id).await;

        let mut statuses = Vec::new();
        while let Ok(event) = events.try_recv() {
            if let EventKind::Job(snapshot) = event.kind {
                statuses.push(snapshot.status);
            }
        }
        assert_eq!(statuses.first(), Some(&JobStatus::Running));
        assert_eq!(statuses.last(), Some(&JobStatus::Succeeded));
    }

    #[tokio::test]
    async fn history_is_bounded() {
        let runner = JobRunner::new(EventBus::new());
        for _ in 0..(HISTORY_LIMIT + 5) {
            let id = runner.spawn(spec(None), |_| async { Ok(()) }).unwrap();
            runner.wait(id).await;
        }
        assert_eq!(runner.list().len(), HISTORY_LIMIT);
    }

    #[test]
    fn error_chain_joins_causes() {
        #[derive(Debug, thiserror::Error)]
        #[error("install failed")]
        struct Outer(#[source] std::io::Error);
        let error = Outer(std::io::Error::other("disk full"));
        assert_eq!(error_chain(&error), "install failed: disk full");
    }
}
