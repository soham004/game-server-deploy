-- Who played on each server and when: one row per visit, from the join and leave lines of its console.
CREATE TABLE player_sessions (
    id          INTEGER PRIMARY KEY,
    instance_id TEXT NOT NULL,
    name        TEXT NOT NULL,
    joined_at   TEXT NOT NULL,
    -- NULL while the player is on, and for a visit whose end the app didn't see.
    left_at     TEXT,
    -- 1 while the player is on.
    open        INTEGER NOT NULL DEFAULT 1
);
CREATE INDEX player_sessions_by_instance ON player_sessions (instance_id, joined_at);
