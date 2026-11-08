-- Schedules: a trigger, conditions and actions, stored together as JSON (`plan_json`, the
-- `SchedulePlan` type). `run_as` is whose permissions the actions use: whoever saved it last.
CREATE TABLE schedules (
    id          TEXT PRIMARY KEY,
    instance_id TEXT NOT NULL,
    plan_json   TEXT NOT NULL,
    run_as      TEXT REFERENCES users (id) ON DELETE SET NULL,
    created_at  TEXT NOT NULL,
    last_run_at TEXT,
    next_run_at TEXT
);
CREATE INDEX schedules_by_instance ON schedules (instance_id);

-- What each run did (the newest runs per schedule are kept).
CREATE TABLE schedule_runs (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    schedule_id TEXT NOT NULL REFERENCES schedules (id) ON DELETE CASCADE,
    cause       TEXT NOT NULL CHECK (cause IN ('trigger', 'catch_up', 'manual')),
    started_at  TEXT NOT NULL,
    finished_at TEXT,
    outcome     TEXT NOT NULL CHECK (outcome IN ('running', 'succeeded', 'failed', 'skipped')),
    message     TEXT
);
CREATE INDEX schedule_runs_by_schedule ON schedule_runs (schedule_id, id);
