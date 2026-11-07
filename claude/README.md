# claude/: Project Memory and Plans

This folder is the long-term memory for building **game-server-deploy** (`gsd`): a Rust-backed, web-UI game server
manager, starting with Minecraft: Java Edition ("Prism Launcher for servers").

**Status (2026-11-07):** research and planning done; the user's answers on scope are recorded (D-014 … D-020):
Linux-first Docker image, multi-user RBAC in v1, per-server isolation, mods and plugins equally, user-supplied CurseForge key,
nginx for TLS, browser-only, build-time branding.

**M0 (skeleton) and M1 (first server runs, isolated) are implemented** (2026-11-07): 172 tests pass, clippy (pedantic,
`-D warnings`) is clean, and a real Paper 26.3 server was created, started, commanded and stopped through the API inside the
Landlock sandbox. Open M1 item: cgroup limits (moved to M2). Next step: M2 in `plan/06-roadmap.md`. Implementation log:
`decisions.md` D-024 (M0) and D-025 (M1, including lessons from the real runs).

CI (`.github/workflows/ci.yml`) also publishes a multi-arch image to GHCR on pushes to `main` and `v*` tags.

### How to build and test (dev machine)
Edit on Windows; build in WSL2 with a Linux-native target dir:
`wsl -e bash -lc "cd /mnt/e/Projects/Rust/game-server-deploy && CARGO_TARGET_DIR=~/.cache/gsd-target cargo test --workspace"`.

## Read in this order
1. `plan/00-product-vision.md`: what we're building and for whom.
2. `plan/01-architecture.md`: crates, layers, concurrency, data directory.
3. `plan/02-rust-conventions.md`: **mandatory** code style for a readable, maintainable backend.
4. `decisions.md`: why things are the way they are.
5. `open-questions.md`: what still needs the user's input or verification.

## Research (facts gathered, with sources)
| File | Contents |
|---|---|
| `research/01-competitive-landscape.md` | Prism Launcher features; Pterodactyl/Pelican, Crafty, MCSManager, AMP, PufferPanel; our positioning |
| `research/02-user-needs-and-feature-catalog.md` | Every feature with priority P0–P3 and UX principles |
| `research/03-minecraft-ecosystem-apis.md` | Mojang, Paper Fill v3, Purpur, Fabric, Quilt, Forge, NeoForge, Modrinth, CurseForge, Adoptium, Mojang profile APIs |
| `research/04-server-control-interfaces.md` | stdin/stdout, RCON, MCSMP (JSON-RPC), SLP, server files, data generator, metrics |

## Plans
| File | Contents |
|---|---|
| `plan/03-data-model.md` | `instance.toml`, SQLite schema, core types |
| `plan/04-http-api.md` | REST endpoints + WebSocket protocol |
| `plan/05-frontend.md` | HTML/CSS/JS structure, components, page map, Hallmark hand-off rules |
| `plan/06-roadmap.md` | Milestones M0–M6 with checkboxes |
| `plan/07-deployment-and-branding.md` | Docker image, compose + nginx, systemd, build-time name/slug/port (`gsd-brand`) |
| `plan/features/isolation-and-sandboxing.md` | Per-server UID, Landlock, rlimits/cgroups, ProcessBackend seam, threat model |
| `plan/features/instances-and-installers.md` | Version picker, providers, per-loader install, import, change version |
| `plan/features/console-and-process.md` | Supervisor actor, state machine, console UI, diagnostics |
| `plan/features/command-autocomplete.md` | Brigadier tree + help parsing + dynamic values |
| `plan/features/logs.md` | Log history and crash reports |
| `plan/features/scheduler.md` | Triggers, conditions, actions, engine |
| `plan/features/backups.md` | Quiesce, archive, retention, restore |
| `plan/features/file-manager.md` | Path safety, operations, editor |
| `plan/features/server-properties.md` | Round-trip parser, schema, visual editor |
| `plan/features/mods-and-modpacks.md` | Modrinth/CurseForge, deps, packwiz, packs |
| `plan/features/updates.md` | Update checker and safe apply with rollback |
| `plan/features/java-runtimes.md` | Detect + download Temurin, JVM presets |
| `plan/features/jobs.md` | Background job system |
| `plan/features/players-and-worlds.md` | Players, gamerules, worlds |
| `plan/features/monitoring-and-notifications.md` | Metrics, health, webhooks, audit |
| `plan/features/security-and-auth.md` | Auth, sessions, CSRF/CSP, remote access |

## Rules for maintaining these notes
- Update the roadmap checkboxes and the README status line as work lands.
- New architectural choices go in `decisions.md` (append, don't rewrite history).
- When an API detail is verified against the live service, mark it **(verified)** in the research doc.
- Keep the docs in sync with the code. If the code diverges from a plan, update the plan in the same change.
