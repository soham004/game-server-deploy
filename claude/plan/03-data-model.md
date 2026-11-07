# Plan 03: Data Model

## 1. `instance.toml` (portable instance definition)
```toml
schema = 1
id = "01JC9Z…"                 # ULID, stable forever
name = "Survival with friends" # display name, editable
group = "Friends"              # optional, for grouping in the dashboard
created_at = "2026-11-07T10:00:00Z"
game = "minecraft"

[server]                       # what software runs
mc_version = "26.3"
loader = "neoforge"            # vanilla | paper | purpur | folia | fabric | quilt | forge | neoforge
loader_version = "26.3.0.101"  # absent for vanilla
build = ""                     # e.g. Paper build id
launch = { kind = "args_files", jvm_args_file = "user_jvm_args.txt", args_file = "libraries/net/neoforged/neoforge/26.3.0.101/win_args.txt" }
# or launch = { kind = "jar", jar = "server.jar" }

[java]
runtime = "managed:temurin-25-x64"   # or "path:C:/Program Files/Java/jdk-25/bin/java.exe" or "auto"
min_memory_mb = 1024
max_memory_mb = 4096
preset = "aikar"               # none | aikar | zgc | custom
extra_jvm_args = []
extra_server_args = ["nogui"]

[behavior]
auto_start = false             # start when the app starts
auto_restart = true            # restart on crash
crash_loop = { max_restarts = 3, window_secs = 600 }
stop_timeout_secs = 60
eula_accepted_at = "2026-11-07T10:00:05Z"   # recorded consent; we write eula.txt from this

[sandbox]                      # Linux isolation (see features/isolation-and-sandboxing.md)
uid = 20001                    # allocated from the configured range at creation; never reused while the instance exists
gid = 20001
extra_connect_ports = []       # admin/owner-only allow-list for outbound connections beyond 80/443
memory_hard_limit_factor = 1.4 # cgroup memory.max = Xmx × factor (when cgroups are available)
pids_max = 2048

[ports]
game = 25565                   # from the public game port range (default 25565–25584, D-022)
query = 25565                  # same as game unless changed
rcon = 35565                   # internal loopback range (default 35565–35664), never published
management = 35566

[integrations]
rcon = { enabled = true, port = 25575 }                  # password stored in server.properties only
management = { enabled = true, port = 25585 }            # MCSMP, loopback-only, secret in server.properties

[modpack]                      # present if created from a pack
source = "modrinth"            # modrinth | curseforge | file
project_id = "1KVo5zza"
version_id = "…"
```
- Rust: `InstanceDefinition` (serde) with `schema` versioning plus `migrate(v) -> v+1` functions.
- Written atomically (write temp file, fsync, rename). Never partially written.

## 2. SQLite schema (initial migration sketch)
```sql
-- users & auth
CREATE TABLE users (id TEXT PRIMARY KEY, username TEXT UNIQUE NOT NULL, password_hash TEXT NOT NULL,
                    global_role TEXT NOT NULL CHECK (global_role IN ('owner','admin','member')),
                    must_change_password INTEGER NOT NULL DEFAULT 0, disabled INTEGER NOT NULL DEFAULT 0,
                    created_at TEXT NOT NULL);
-- per-instance access for 'member' users: a preset role or a custom permission list
CREATE TABLE instance_grants (user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE, instance_id TEXT NOT NULL,
                              preset TEXT CHECK (preset IN ('viewer','moderator','operator','manager','custom')),
                              permissions_json TEXT,           -- only when preset = 'custom'
                              PRIMARY KEY (user_id, instance_id));
CREATE TABLE sessions (token_hash TEXT PRIMARY KEY, user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
                       created_at TEXT NOT NULL, expires_at TEXT NOT NULL, last_seen_at TEXT, user_agent TEXT, ip TEXT);

-- instance index (cache of instance.toml + runtime info)
CREATE TABLE instances (id TEXT PRIMARY KEY, slug TEXT UNIQUE NOT NULL, name TEXT NOT NULL,
                        last_started_at TEXT, last_stopped_at TEXT, last_exit_code INTEGER);

-- scheduler
CREATE TABLE schedules (id TEXT PRIMARY KEY, instance_id TEXT NOT NULL, name TEXT NOT NULL, enabled INTEGER NOT NULL,
                        trigger_json TEXT NOT NULL,      -- {kind:"cron", expr, tz} | {kind:"interval",secs} | {kind:"once",at} | {kind:"event", event}
                        conditions_json TEXT NOT NULL,   -- [{kind:"server_running"}, {kind:"players_since_last_run"}]
                        actions_json TEXT NOT NULL,      -- ordered list of actions
                        last_run_at TEXT, next_run_at TEXT, created_at TEXT NOT NULL);
CREATE TABLE schedule_runs (id INTEGER PRIMARY KEY, schedule_id TEXT NOT NULL, started_at TEXT NOT NULL,
                            finished_at TEXT, outcome TEXT, message TEXT);

-- backups
CREATE TABLE backups (id TEXT PRIMARY KEY, instance_id TEXT NOT NULL, kind TEXT NOT NULL,  -- manual|scheduled|pre_update|pre_restore|pre_version_change
                      scope TEXT NOT NULL,             -- world|full|custom
                      path TEXT NOT NULL, size_bytes INTEGER, sha256 TEXT, mc_version TEXT, loader TEXT,
                      note TEXT, pinned INTEGER NOT NULL DEFAULT 0, created_at TEXT NOT NULL, status TEXT NOT NULL);

-- jobs history (live jobs are in memory)
CREATE TABLE jobs (id TEXT PRIMARY KEY, instance_id TEXT, kind TEXT NOT NULL, status TEXT NOT NULL,
                   started_at TEXT NOT NULL, finished_at TEXT, error TEXT, log TEXT);

-- player history
CREATE TABLE player_sessions (id INTEGER PRIMARY KEY, instance_id TEXT NOT NULL, uuid TEXT NOT NULL, name TEXT NOT NULL,
                              joined_at TEXT NOT NULL, left_at TEXT);

-- metrics (downsampled; raw ring buffer stays in memory)
CREATE TABLE metrics (instance_id TEXT NOT NULL, ts INTEGER NOT NULL, resolution INTEGER NOT NULL, -- secs per bucket
                      cpu_pct REAL, rss_bytes INTEGER, players INTEGER, mspt REAL, tps REAL,
                      PRIMARY KEY (instance_id, resolution, ts));

-- audit
CREATE TABLE audit_log (id INTEGER PRIMARY KEY, ts TEXT NOT NULL, user_id TEXT, instance_id TEXT,
                        action TEXT NOT NULL, details_json TEXT);

-- notifications
CREATE TABLE webhooks (id TEXT PRIMARY KEY, kind TEXT NOT NULL, url TEXT NOT NULL, events_json TEXT NOT NULL,
                       instance_id TEXT, enabled INTEGER NOT NULL);
```
- `PRAGMA journal_mode=WAL; foreign_keys=ON; busy_timeout=5000`.
- Migrations live in `crates/gsd-core/src/store/migrations/NNNN_name.sql` and run on startup.
- Repos are one struct per table group (`ScheduleRepo`, `BackupRepo`, …) exposing typed methods. Raw SQL never appears in services.

## 3. Core domain types (sketch)
```rust
pub struct InstanceId(Ulid);
pub enum LoaderKind { Vanilla, Paper, Purpur, Folia, Fabric, Quilt, Forge, NeoForge }
pub struct ServerSoftware { mc: McVersion, loader: LoaderKind, loader_version: Option<String>, build: Option<String> }

pub enum ServerState {
    Stopped { exit: Option<ExitInfo> },
    Installing { job: JobId },
    Starting { since: DateTime<Utc>, pid: u32 },
    Running  { since: DateTime<Utc>, pid: u32 },
    Stopping { since: DateTime<Utc>, pid: u32, reason: StopReason },
    Crashed  { at: DateTime<Utc>, exit: ExitInfo, restart_in: Option<Duration> },
}

pub enum EventKind {
    StateChanged(ServerState), ConsoleLine(LogLine), PlayerJoined(Player), PlayerLeft(Player),
    JobProgress(JobProgress), JobFinished(JobSummary), MetricsSample(Sample),
    Diagnostic(Diagnostic), BackupCreated(BackupSummary), UpdateAvailable(UpdateInfo), …
}
```
