//! What a schedule is: a trigger (when), conditions (whether) and actions (what), plus its runs.
//!
//! These types are the wire format too (`serde`, tagged by `kind`): the UI edits them as JSON and
//! the database stores them as JSON, so a new trigger, condition or action is one variant here.

use chrono::{DateTime, NaiveTime, Utc};
use serde::{Deserialize, Serialize};

use crate::{
    auth::Permission,
    backups::BackupScope,
    ids::{InstanceId, ScheduleId, UserId},
};

/// A schedule of one instance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Schedule {
    pub id: ScheduleId,
    pub instance_id: InstanceId,
    pub plan: SchedulePlan,
    /// Whose permissions the actions run with: whoever saved it last (checked at every run).
    /// `None` once that user is deleted; the schedule then fails until someone saves it again.
    pub run_as: Option<UserId>,
    pub created_at: DateTime<Utc>,
    pub last_run_at: Option<DateTime<Utc>>,
    /// When a time trigger fires next; `None` for event triggers and finished one-off schedules.
    pub next_run_at: Option<DateTime<Utc>>,
}

/// The part of a schedule people edit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SchedulePlan {
    pub name: String,
    pub enabled: bool,
    pub trigger: Trigger,
    #[serde(default)]
    pub conditions: Vec<Condition>,
    pub actions: Vec<Action>,
    /// What to do after a failed action.
    #[serde(default)]
    pub on_error: OnError,
    /// After downtime (the app was off when it was due): run once on startup, or skip to the
    /// next time.
    #[serde(default)]
    pub catch_up: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Trigger {
    /// A cron expression (5 fields, or 6 with seconds) in an IANA time zone, e.g. `0 4 * * *` in
    /// `Europe/London`.
    Cron { expr: String, time_zone: String },
    /// Every this many seconds (at least a minute), counted from the last run.
    Interval { every_secs: u64 },
    /// Once, at this moment.
    Once { at: DateTime<Utc> },
    /// When something happens to the server.
    Event { event: ServerEvent },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ServerEvent {
    Started,
    Stopped,
    Crashed,
    PlayerJoined,
    /// Nobody was online for this long while it ran (counted from the last player leaving, or
    /// from the start).
    EmptyFor {
        secs: u64,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Condition {
    ServerRunning,
    ServerStopped,
    PlayersOnline {
        min: u32,
    },
    /// Someone was online since the schedule last ran ("only back up if somebody played").
    PlayersSinceLastRun,
    /// Only between these times of day in `time_zone`; `from` after `to` spans midnight.
    TimeWindow {
        from: NaiveTime,
        to: NaiveTime,
        time_zone: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Action {
    /// A console command (without `/`).
    Command {
        command: String,
    },
    /// A message to everyone online (`say`).
    Broadcast {
        message: String,
    },
    /// Restart, warning players this many seconds before (e.g. `[600, 300, 60, 10]`).
    Restart {
        #[serde(default)]
        warn_secs: Vec<u64>,
    },
    /// Stop, warning players first.
    Stop {
        #[serde(default)]
        warn_secs: Vec<u64>,
    },
    Start,
    Backup {
        scope: BackupScope,
        #[serde(default)]
        note: Option<String>,
    },
    /// Pause between actions.
    Wait {
        secs: u64,
    },
}

impl Action {
    /// The permission the schedule's creator needs for this action.
    pub fn permission(&self) -> Permission {
        match self {
            Self::Command { .. } | Self::Broadcast { .. } => Permission::ConsoleWrite,
            Self::Restart { .. } | Self::Stop { .. } | Self::Start => Permission::Lifecycle,
            Self::Backup { .. } => Permission::BackupsCreate,
            Self::Wait { .. } => Permission::InstanceView,
        }
    }
}

/// A permission's wire name ("console.write"), for messages.
pub(super) fn permission_name(permission: Permission) -> String {
    serde_json::to_value(permission)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_default()
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OnError {
    /// Skip the remaining actions.
    #[default]
    Stop,
    Continue,
}

/// How a run ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunOutcome {
    Running,
    Succeeded,
    /// An action failed (with `on_error: continue`, the run still went on).
    Failed,
    /// The conditions didn't hold, or the server was busy; nothing ran.
    Skipped,
}

impl RunOutcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Skipped => "skipped",
        }
    }
}

/// Why a run started.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunCause {
    /// Its time came, or its event happened.
    Trigger,
    /// It was due while the app was off.
    CatchUp,
    /// Someone pressed "Run now".
    Manual,
}

impl RunCause {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Trigger => "trigger",
            Self::CatchUp => "catch_up",
            Self::Manual => "manual",
        }
    }
}

/// One run of a schedule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ScheduleRun {
    pub id: i64,
    pub schedule_id: ScheduleId,
    pub cause: RunCause,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub outcome: RunOutcome,
    /// What happened, for people: the reason it was skipped, or the failed action's error.
    pub message: Option<String>,
}
