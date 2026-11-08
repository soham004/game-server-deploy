-- A new backup kind, `pre_update` (taken before mods or plugins are updated). SQLite can't change
-- a CHECK constraint in place, so the table is rebuilt with the same columns and rows.
CREATE TABLE backups_new (
    id          TEXT PRIMARY KEY,
    instance_id TEXT NOT NULL,
    kind        TEXT NOT NULL CHECK (kind IN ('manual', 'scheduled', 'pre_restore', 'pre_version_change', 'pre_update')),
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
INSERT INTO backups_new SELECT id, instance_id, kind, scope, file_name, size_bytes, sha256, software, note, pinned,
    created_at, created_by FROM backups;
DROP TABLE backups;
ALTER TABLE backups_new RENAME TO backups;
CREATE INDEX backups_by_instance ON backups (instance_id, created_at);
