# Feature: Scheduler (cron + Minecraft commands)

**Built in M3 (D-036).** Differences from this sketch: the event trigger "backup finished", the `CheckUpdates`,
`ApplyServerUpdate` and `Webhook` actions and drag reordering are not built (the roadmap lists them under Later);
overlaps are prevented by the instance's job lock (a busy server records the run as skipped); tests use real time
instead of an injected clock.

## Model
```rust
pub struct Schedule {
    id: ScheduleId, instance: InstanceId, name: String, enabled: bool,
    trigger: Trigger, conditions: Vec<Condition>, actions: Vec<Action>,
    last_run: Option<DateTime<Utc>>, next_run: Option<DateTime<Utc>>,
}
pub enum Trigger {
    Cron { expr: String, tz: Tz },                 // validated with `croner`; 5- or 6-field
    Interval { every: Duration, anchor: DateTime<Utc> },
    Once { at: DateTime<Utc> },
    Event(EventTrigger),                           // ServerStarted | ServerStopped | ServerCrashed | PlayerJoined | ServerEmptyFor(Duration) | BackupFinished
}
pub enum Condition {
    ServerRunning, ServerStopped,
    PlayersOnline { min: u32 },
    PlayersSinceLastRun,                           // "only back up if someone played" (AMP lacks this)
    TimeWindow { from: NaiveTime, to: NaiveTime },
}
pub enum Action {
    Command { line: String },                      // supports multiple lines
    Broadcast { message: String },                 // `say` / tellraw with styling
    Restart { warn: Vec<Duration> },               // e.g. [10m, 5m, 1m, 10s] countdown messages
    Stop { warn: Vec<Duration> }, Start,
    Backup { scope: BackupScope, note: Option<String> },
    Wait { duration: Duration },
    CheckUpdates, ApplyServerUpdate,               // later (P2)
    Webhook { id: WebhookId, message: String },
}
```
Actions run **sequentially** as one job. If a step fails, the remaining actions are skipped (`on_error: stop|continue`, per schedule).

## Engine (gsd-core/scheduler/engine.rs)
- One task. Keeps a min-heap of `(next_run, schedule_id)`. `sleep_until(next)` or wakes on a `Notify` when schedules change.
- On fire: evaluate conditions → create a run record → dispatch the actions as a Job → compute and persist `next_run`.
- **Missed runs** (app was off): on startup, a schedule whose `next_run` is in the past either runs once ("catch up",
  for backups) or is skipped (for restarts). Configurable per schedule; the default depends on the action type.
- Event triggers: subscribe to the EventBus; `ServerEmptyFor` uses a per-instance timer reset on join.
- Overlap protection: if the previous run of the same schedule is still running, skip it and record `skipped_overlap`.
- Clock is injected, so tests advance fake time.

## UI
- List view: name, human-readable trigger ("Every day at 04:00 (Europe/London)"), next run (relative), last result
  pill, enabled toggle, "Run now", and grouping and sorting (the AMP complaint).
- Editor: trigger builder with presets (Every N minutes, Daily at, Weekly on, Custom cron with live preview of the next 5
  runs), condition chips, action list with drag reordering, plus templates:
  - "Daily restart at 4 AM with warnings"
  - "Backup every 6 hours if players were online"
  - "Announce a message every 30 min"
  - "Save the world every 10 min (`save-all`)"
- Run history drawer with per-run job log.
