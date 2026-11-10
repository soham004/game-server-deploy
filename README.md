# game-server-deploy

A self-hosted game server manager with a Rust backend and a plain HTML/CSS/JS web UI, starting with
Minecraft: Java Edition ("Prism Launcher for servers"). The product name, slug and default port are
build-time settings (see `crates/gsd-brand`).

**Status:** milestones M0 to M3. Create Vanilla, Paper, Purpur, Fabric, Quilt, Forge and NeoForge servers from a
version picker, or import an existing server folder; run them sandboxed (own user, Landlock filesystem and network
rules, resource limits, and a hard memory ceiling with cgroup v2 where the host delegates it), with a live console,
auto-restart on crash and diagnostics. Edit `server.properties` visually, manage files, change settings, clone
servers, change their Minecraft or loader version, and manage users with per-server access. Back up worlds or whole
servers (a running world is paused and flushed first) and restore them; schedule commands, restarts with in-game
warnings and backups by time or event. Deleted servers and files wait in a trash for a week. Plans and research live in
[`claude/`](claude/README.md).

## Layout
| Path | What |
|---|---|
| `crates/gsd-brand` | The single declaration of name, slug and default port |
| `crates/gsd-core` | Game-agnostic services: store, auth/RBAC, audit, jobs, Java runtimes, instances, sandboxed supervisor, backups, scheduler, trash |
| `crates/gsd-minecraft` | Minecraft adapter: version catalog and installers for every loader, import, clone, `server.properties`, log parsing, diagnostics |
| `crates/gsd-mods` | Modrinth / CurseForge / Hangar (from M4) |
| `crates/gsd-app` | The binary: config, CLI, HTTP API, embedded web UI |
| `web/` | Frontend: plain HTML, CSS (design tokens in `css/tokens.css`) and ES modules; `web/vendor/codemirror.js` is the one prebuilt file (the code editor) |
| `tools/codemirror` | Builds `web/vendor/codemirror.js` with npm and esbuild, pinned (not needed to run or develop the app) |
| `deploy/docker` | Container entrypoint and an example compose file |

## Develop (Linux / WSL2)
```bash
cargo run -p gsd-app -- --data-dir ~/gsd-data serve
```
Open the URL printed at startup. Debug builds read `web/` from disk, so frontend edits only need a reload.

Keep the data folder on a Linux filesystem. On WSL, Windows drives (`/mnt/c`, `/mnt/e`, …) don't support the
sandbox's file rules, so servers there can't even open their own jar. The app checks this at startup and refuses to start
servers with an explanation. They are also much slower for world saves.

Logs: lifecycle events (start, exit, crash, problems found) and failed jobs go to the terminal. To also see every line
the game servers print, run with `GSD_LOG=info,gsd_core::supervisor=debug`.

```bash
bash scripts/check.sh   # what CI runs: fmt, clippy, all tests, branding, frontend syntax and unit tests
```
Real servers, a real browser, Docker as root and systemd: see [`tests/e2e/`](tests/e2e/README.md).

## Commands
| Command | Purpose |
|---|---|
| `serve` (default) | Run the web server |
| `reset-password <user>` | Give a user a temporary password; they must change it at next sign-in |
| `healthcheck` | Exit 0 if the local server answers (used by the Docker health check) |
| `brand name\|slug\|port\|env-prefix` | Print a branding value |
| `isolation-check` | Prove the per-server sandbox works in the data folder on this host (run as root to include separate users) |

Global flags: `--data-dir`, `--bind`. Environment variables use the brand prefix (`GSD_` by default):
`GSD_DATA_DIR`, `GSD_BIND`, `GSD_TRUSTED_PROXIES`, `GSD_GAME_PORTS`, `GSD_INTERNAL_PORTS`,
`GSD_SESSION_TTL_DAYS`, `GSD_TRASH_RETENTION_DAYS` (default 7; 0 keeps the trash), `GSD_LOG`. The same settings (minus the data dir) can go in `<data dir>/config.toml`.

## Docker
```bash
docker build -t gsd .
docker compose -f deploy/docker/compose.yaml up -d --build
```

Published images (built by CI on pushes to `main` and `v*` tags, for linux/amd64 and linux/arm64):
`ghcr.io/<owner>/<repo>:latest`, `:main`, `:<version>`, `:sha-<commit>`.

## Hard memory limits (cgroup v2)
Every server's heap is capped by `-Xmx`. A hard ceiling for the whole server process (heap plus Java's own memory, no
swap) and a cap on processes and threads need a control group the app may manage:

- **systemd**: run the app as a service with `Delegate=yes` in its unit. The app moves itself into
  `app/` inside its group and gives each server `instances/<id>/`.
- **Docker**: the default compose setup mounts the cgroup filesystem read-only, so hard limits are off. Servers still run.

The startup log, `GET /api/v1/system` (`hard_limits`, `hard_limits_problem`) and the isolation check say which applies
and why. When a server outgrows its ceiling, the console and its problems list say so.
