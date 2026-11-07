-- Users, sessions, per-instance grants (RBAC) and the audit log.
-- Timestamps are RFC 3339 UTC text. IDs are ULID text.

CREATE TABLE users (
    id                   TEXT PRIMARY KEY,
    username             TEXT NOT NULL UNIQUE COLLATE NOCASE,
    password_hash        TEXT NOT NULL,
    global_role          TEXT NOT NULL CHECK (global_role IN ('owner', 'admin', 'member')),
    must_change_password INTEGER NOT NULL DEFAULT 0,
    disabled             INTEGER NOT NULL DEFAULT 0,
    created_at           TEXT NOT NULL
);

-- Only the SHA-256 of a session token is stored; the token itself lives in the cookie.
CREATE TABLE sessions (
    token_hash   TEXT PRIMARY KEY,
    user_id      TEXT NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    created_at   TEXT NOT NULL,
    expires_at   TEXT NOT NULL,
    last_seen_at TEXT NOT NULL,
    user_agent   TEXT,
    ip           TEXT
);
CREATE INDEX sessions_by_user ON sessions (user_id);
CREATE INDEX sessions_by_expiry ON sessions (expires_at);

-- Access of 'member' users to individual instances: a preset role or a custom permission list.
CREATE TABLE instance_grants (
    user_id          TEXT NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    instance_id      TEXT NOT NULL,
    preset           TEXT NOT NULL CHECK (preset IN ('viewer', 'moderator', 'operator', 'manager', 'custom')),
    permissions_json TEXT,
    PRIMARY KEY (user_id, instance_id)
);

CREATE TABLE audit_log (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    ts           TEXT NOT NULL,
    user_id      TEXT,
    instance_id  TEXT,
    action       TEXT NOT NULL,
    details_json TEXT NOT NULL DEFAULT '{}'
);
CREATE INDEX audit_log_by_ts ON audit_log (ts);
