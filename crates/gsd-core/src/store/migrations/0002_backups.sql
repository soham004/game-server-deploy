-- Backup archives. The files live in <data>/backups/<instance id>/<file_name>; only finished,
-- complete archives get a row. Timestamps are RFC 3339 UTC text, IDs ULID text.
CREATE TABLE backups (
    id          TEXT PRIMARY KEY,
    instance_id TEXT NOT NULL,
    kind        TEXT NOT NULL CHECK (kind IN ('manual', 'scheduled', 'pre_restore', 'pre_version_change')),
    scope       TEXT NOT NULL CHECK (scope IN ('world', 'full')),
    file_name   TEXT NOT NULL,
    size_bytes  INTEGER NOT NULL,
    sha256      TEXT NOT NULL,
    software    TEXT,
    note        TEXT,
    pinned      INTEGER NOT NULL DEFAULT 0,
    created_at  TEXT NOT NULL,
    created_by  TEXT REFERENCES users (id) ON DELETE SET NULL,
    UNIQUE (instance_id, file_name)
);
CREATE INDEX backups_by_instance ON backups (instance_id, created_at);
