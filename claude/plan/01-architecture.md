# Plan 01: Architecture

## 1. Big picture

```
┌──────────────────────────── Browser ─────────────────────────────┐
│  web/ (plain HTML + CSS + ES-module JS, no framework, no build)   │
│  REST (fetch) for actions & queries · one WebSocket for live data │
└───────────────▲───────────────────────────────▲──────────────────┘
                │ HTTP/JSON  /api/v1/*          │ WS /api/v1/ws
┌───────────────┴───────────────────────────────┴──────────────────┐
│ gsd-app (binary)                                                  │
│   http/   axum routers per feature → thin handlers (parse→call→map)│
│   wiring: config, AppState, startup/shutdown, static asset embed   │
├───────────────────────────────────────────────────────────────────┤
│ gsd-core (game-agnostic services)                                 │
│   instances · supervisor (per-instance actor) · jobs · scheduler   │
│   backups · files · events bus · java runtimes · store (SQLite)    │
│   auth · notifications · metrics                                   │
│   trait GameAdapter  ◄──────────── implemented by ───────┐         │
├──────────────────────────────────────────────────────────┼────────┤
│ gsd-minecraft                                             │        │
│   providers (vanilla/paper/purpur/fabric/quilt/forge/neoforge)     │
│   installers · launch plans · log parser · properties (round-trip) │
│   rcon · mcsmp client · slp ping · commands tree · nbt · players   │
├───────────────────────────────────────────────────────────────────┤
│ gsd-mods                                                          │
│   modrinth client · curseforge client · packwiz index · mrpack     │
│   CF fingerprint (murmur2) · dependency resolver                   │
└───────────────────────────────────────────────────────────────────┘
          │ spawns & supervises                    │ HTTPS
          ▼                                        ▼
   java … server.jar nogui (child processes)   Mojang/Paper/Fabric/Forge/
                                               NeoForge/Quilt/Modrinth/CF/Adoptium
```

## 2. Cargo workspace layout

```
game-server-deploy/
├─ Cargo.toml                 # [workspace], shared [workspace.dependencies], lints
├─ crates/
│  ├─ gsd-brand/              # THE single source of app name / slug / default port (build-time env → consts); see 07-deployment-and-branding.md
│  ├─ gsd-app/                # bin "gsd": main, config, http layer, asset embedding
│  │  └─ src/
│  │     ├─ main.rs           # parse CLI → load config → build AppState → serve → graceful shutdown
│  │     ├─ config.rs
│  │     ├─ state.rs          # AppState { services… } (Arc'd handles only)
│  │     ├─ assets.rs         # rust-embed of ../../web
│  │     └─ http/
│  │        ├─ mod.rs         # router assembly, middleware (auth, tracing, compression)
│  │        ├─ error.rs       # ApiError → JSON {code, message, details}
│  │        ├─ ws.rs          # WebSocket hub: subscribe/unsubscribe topics
│  │        └─ routes/        # one file per feature: instances.rs, console.rs, files.rs, backups.rs …
│  ├─ gsd-core/
│  │  └─ src/
│  │     ├─ lib.rs
│  │     ├─ game.rs           # trait GameAdapter + shared game-facing types
│  │     ├─ instances/        # model.rs, registry.rs, service.rs, layout.rs (paths)
│  │     ├─ supervisor/       # actor.rs, handle.rs, state.rs (state machine),
│  │     │                    # backend/ (ProcessBackend trait: native.rs, docker.rs later),
│  │     │                    # sandbox/ (linux.rs: per-instance UID, Landlock, rlimits, cgroups; fallback.rs)
│  │     ├─ jobs/             # job runner, progress, cancellation
│  │     ├─ scheduler/        # model.rs, engine.rs (tick loop), actions.rs
│  │     ├─ backups/          # model.rs, service.rs, archive.rs, retention.rs
│  │     ├─ files/            # safe_path.rs, service.rs (list/read/write/upload/zip)
│  │     ├─ java/             # detect.rs, download.rs, model.rs
│  │     ├─ events.rs         # EventBus (tokio broadcast), Event enum
│  │     ├─ store/            # sqlite pool, migrations/, repos per table
│  │     ├─ auth/             # users, sessions, password hashing
│  │     ├─ notify/           # webhook sinks
│  │     ├─ metrics/          # sampler, downsampling
│  │     └─ http_client.rs    # shared reqwest client (UA, timeouts, retry, cache dir)
│  ├─ gsd-minecraft/
│  │  └─ src/
│  │     ├─ lib.rs            # MinecraftAdapter: impl GameAdapter
│  │     ├─ version.rs        # McVersion parsing/ordering (1.x and 26.x schemes)
│  │     ├─ providers/        # mojang.rs, paper.rs, purpur.rs, fabric.rs, quilt.rs, forge.rs, neoforge.rs
│  │     ├─ install/          # per-loader install steps → LaunchPlan
│  │     ├─ launch.rs         # LaunchPlan → std::process::Command
│  │     ├─ log_parser.rs     # line → LogLine{time, thread, level, source, msg} + GameEvent detection
│  │     ├─ diagnostics.rs    # known failure patterns → Diagnostic{code, title, fix}
│  │     ├─ properties/       # parser.rs (round-trip), schema.rs (key metadata)
│  │     ├─ rcon.rs · mcsmp.rs · ping.rs
│  │     ├─ commands/         # tree.rs (brigadier json), help_parser.rs, completer.rs, datagen.rs
│  │     ├─ players.rs        # uuid lookup, offline uuid, heads, json lists
│  │     └─ world.rs          # level.dat via fastnbt, stats
│  └─ gsd-mods/
│     └─ src/  modrinth.rs · curseforge.rs · fingerprint.rs · packwiz.rs · mrpack.rs · resolver.rs · model.rs
├─ web/                        # frontend (see 05-frontend.md)
├─ claude/                     # these notes
└─ tests/ (workspace-level integration tests if needed)
```

**Why these crate boundaries?**
- `gsd-minecraft` and `gsd-mods` have **no dependency on HTTP or the app**, so they can be unit-tested against
  fixtures (sample manifests, log files, properties files).
- `gsd-core` knows nothing about Minecraft: only the `GameAdapter` trait. Adding a game later means a new adapter crate.
- `gsd-app` is the only place that knows about axum and wiring. Handlers stay thin.
- Dependency direction: `app → core, minecraft, mods`; `minecraft → core (trait + shared types)`; `mods → (nothing internal)`;
  **everyone → brand** (leaf crate, no deps). **No cycles. core never depends on minecraft.**
- `gsd-brand` exists so the product name, slug and default port are declared once and changeable at build time (user requirement).

## 3. Request flow (the "clean logical flow" rule)
Every feature follows the same four steps, so any file is easy to navigate:

```
HTTP handler (gsd-app/http/routes/x.rs)
  1. extract + validate input (typed extractor / DTO)   — no business logic
  2. call ONE service method                             — e.g. backups.create(instance_id, opts)
  3. map the domain result to a response DTO             — or ApiError via From<ServiceError>
Service (gsd-core/x/service.rs)
  - orchestrates: load → check invariants → do work (often spawns a Job) → persist → publish Event
  - depends on repos / adapters via constructor-injected handles
Domain model (gsd-core/x/model.rs)
  - plain structs/enums, validation, state transitions; no IO
Infrastructure (store/, process.rs, http_client.rs, providers/)
  - the only places that touch disk, network, processes, DB
```

## 4. Concurrency model
- **Tokio multi-thread runtime.** Blocking work (zip, hashing large files, NBT parsing) goes through `spawn_blocking`.
- **Supervisor actor per instance**: one task owns the `Child`, its stdin writer and the stdout and stderr readers.
  Others talk to it through a cloneable `SupervisorHandle` (an `mpsc::Sender<SupervisorCmd>`) and observe it through
  the **EventBus**. No `Arc<Mutex<Child>>` anywhere. See `features/console-and-process.md`.
- **EventBus**: a `tokio::sync::broadcast` of `Event { instance_id: Option<Id>, kind: EventKind }`. The WS hub,
  scheduler (event triggers), notifier, metrics and audit log are subscribers. Slow subscribers that lag are
  dropped with a resync hint (the UI refetches state over REST).
- **Jobs**: long operations (install, backup, restore, modpack import, Java download, update) run as `Job`s with
  `JobId`, progress events, logs and a `CancellationToken`. One `JobRunner` and a per-instance lock prevent
  conflicting jobs (for example a backup during a reinstall). See `features/jobs.md`.
- **Scheduler**: a single task that wakes at the next due time (`tokio::time::sleep_until`) and dispatches actions.
- **Graceful app shutdown**: on Ctrl+C or a service stop, stop accepting requests, then optionally (per setting) stop
  all servers gracefully in parallel with a timeout, flush the DB, then exit.

## 5. Data directory layout
Default: `/data` in Docker; `/var/lib/<slug>` for the systemd service; otherwise the OS data dir
(`~/.local/share/<slug>`, `%APPDATA%\<slug>`, `~/Library/Application Support/<slug>`). Overridable with `--data-dir` or
`<PREFIX>_DATA_DIR`. `<slug>` and `<PREFIX>` come from `gsd_brand`.
Ownership on Linux: the app's files (`gsd.sqlite`, `config.toml`, `backups/`, `cache/`) are `root:root 0600/0700`; each
`instances/<slug>/server` is owned by that instance's sandbox UID (`0700`). Runtimes are world-readable and read-only.

```
<data>/
├─ config.toml                 # app settings (bind addr, port, defaults, API keys*)
├─ gsd.sqlite                  # users, sessions, schedules, backups index, jobs history, metrics, audit
├─ instances/
│  └─ <slug>/                  # slug fixed at creation; display name lives in instance.toml
│     ├─ instance.toml         # THE instance definition (portable, human-readable)
│     ├─ icon.png
│     ├─ notes.md
│     └─ server/               # the Minecraft server root (cwd of the process; file manager root)
│        ├─ server.properties, eula.txt, mods/, mods/.index/*.pw.toml, world/, logs/ …
├─ backups/<slug>/<timestamp>-<kind>.zip|tar.zst
├─ runtimes/java/<vendor>-<major>-<arch>/           # managed JREs
├─ cache/
│  ├─ meta/                     # cached manifests (with ETag/expiry)
│  ├─ downloads/<sha1>          # content-addressed jar cache (shared across instances)
│  ├─ commands/<mcversion>.json # generated command trees
│  └─ heads/<uuid>.png
└─ logs/gsd.log (+ rotated)
```
\* Secrets (CurseForge key, webhook URLs) live in `config.toml` with restrictive file permissions. Later we can move them
to the OS keychain (the `keyring` crate). See `decisions.md`.

**Source of truth**: `instance.toml` defines an instance (so a folder can be copied between machines). SQLite holds
operational data. On startup the registry scans `instances/*/instance.toml` and reconciles it with the DB (orphaned
schedules get flagged, not deleted).

## 6. Key dependencies (choose latest stable at implementation time)
| Concern | Crate | Note |
|---|---|---|
| Async runtime | `tokio` (full), `tokio-util` (CancellationToken, codecs) | |
| HTTP server + WS | `axum` (with `ws`), `tower-http` (trace, compression, cors off, limit) | |
| HTTP client | `reqwest` (rustls, json, stream) | one shared client with UA |
| Serialization | `serde`, `serde_json`, `toml` | |
| DB | `sqlx` (sqlite, migrate), using runtime-checked queries plus `FromRow` | avoids the `DATABASE_URL` build friction of macros |
| Errors | `thiserror` in libs, `anyhow` only in `main.rs` | |
| Logging | `tracing`, `tracing-subscriber`, `tracing-appender` | |
| Time / cron | `chrono` + `chrono-tz`, `croner` (cron parsing) | |
| Archives | `zip`, `tar`, `zstd`, `flate2` | |
| FS | `walkdir`, `globset`, `tempfile`, `dunce` (Windows canonical paths) | |
| Hashing | `sha1`, `sha2`, `murmur2` (CF fingerprint) | |
| NBT | `fastnbt` | level.dat |
| WS client | `tokio-tungstenite` | MCSMP |
| System | `sysinfo` (CPU, RAM), `win32job` (Windows job objects), `nix` (signals, setuid/setgroups, rlimits) | |
| Sandbox | `landlock` (fs + net rules, best effort), cgroup v2 via plain file writes (no crate needed) | Linux only |
| Docker backend (P2) | `bollard` | optional per-server containers |
| Static assets | `rust-embed` | serves `web/` from the binary in release builds; from disk in dev |
| Auth | `argon2`, `rand` | |
| CLI | `clap` | `gsd serve`, `gsd service install`, `gsd reset-password` |
| Misc | `uuid`, `directories`, `open` (launch browser) | |

## 7. Cross-cutting concerns
- **Config**: `config.toml` plus env overrides plus CLI flags, in that precedence. Validated at startup with clear errors.
- **Logging**: structured `tracing` with an `instance_id` span field; app logs are separate from server logs.
- **Errors**: see `02-rust-conventions.md`. API errors have a stable `code` the UI can switch on.
- **Security**: multi-user RBAC from v1; bind `127.0.0.1` on bare metal; TLS via nginx. See `features/security-and-auth.md`.
- **Isolation**: every server runs as its own UID with Landlock and rlimits (cgroups when available). See `features/isolation-and-sandboxing.md`.
- **Branding**: never hard-code the name, slug or port; use `gsd_brand`. See `07-deployment-and-branding.md`.
- **Testing**: unit tests with fixtures in each lib crate; integration tests spin up the app with a temp data dir and a
  **fake server** (a tiny script or binary that mimics MC stdout and stdin) so the supervisor, console, backups and scheduler can be
  tested without Java.
- **Platform matrix (Linux first)**: Linux x64/arm64 is the primary target, shipped as a Docker image (plus systemd for bare
  metal). Windows and macOS are development and home targets without sandboxing. CI must pass on Linux; Windows builds and unit tests too.
