//! The scheduler: run actions (console commands, restarts with warnings, backups, …) on a
//! timetable or when something happens to a server.
//!
//! - `model.rs`: schedules, triggers, conditions, actions, runs (also the JSON wire format).
//! - `validate.rs`: checking a plan before it is saved; `trigger.rs`: when a trigger fires next.
//! - `conditions.rs`: whether the conditions hold when it fires.
//! - `repo.rs`: the `schedules` and `schedule_runs` tables.
//! - `service.rs`: the API; `engine.rs`: the task that fires schedules; `run.rs`: one run
//!   (permission and condition checks, then the actions in a job); `actions.rs`: each action;
//!   `activity.rs`: who is online, from server events.
//!
//! See `claude/plan/features/scheduler.md` and decision D-036.

mod actions;
mod activity;
mod conditions;
mod engine;
mod model;
mod repo;
mod run;
mod service;
mod trigger;
mod validate;

pub use conditions::{Situation, unmet};
pub use model::{
    Action, Condition, OnError, RunCause, RunOutcome, Schedule, SchedulePlan, ScheduleRun, ServerEvent,
    Trigger,
};
pub use repo::ScheduleRepo;
pub use service::{ScheduleError, Scheduler};
pub use trigger::{TriggerError, describe, next_time};
pub use validate::{PlanError, validate};
