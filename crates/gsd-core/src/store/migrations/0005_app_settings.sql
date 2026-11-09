-- Settings of the app itself (not of one server), one row each. Secrets among them stay on the
-- server: the API only says whether they are set.
CREATE TABLE app_settings (
    key        TEXT PRIMARY KEY,
    value      TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
