# game-server-deploy

A self-hosted game server manager with a Rust backend and a plain HTML/CSS/JS web UI, starting with
Minecraft: Java Edition ("Prism Launcher for servers"). The product name, slug and default port are
build-time settings (see `crates/gsd-brand`).

**Status:** milestones M0 + M1. Create Vanilla, Paper and Fabric servers from a version picker, run them
sandboxed (own user, Landlock filesystem and network rules, resource limits), with a live console,
auto-restart on crash, diagnostics, background jobs, and multi-user roles. Plans and research live in
[`claude/`](claude/README.md).

## Layout
| Path | What |
|---|---|
| `crates/gsd-brand` | The single declaration of name, slug and default port |
| `crates/gsd-core` | Game-agnostic services: store, auth/RBAC, audit, jobs, Java runtimes, instances, sandboxed supervisor |
| `crates/gsd-minecraft` | Minecraft adapter: version catalog (Mojang/Paper/Fabric), installer, log parsing, diagnostics |
| `crates/gsd-mods` | Modrinth / CurseForge / Hangar (from M4) |
| `crates/gsd-app` | The binary: config, CLI, HTTP API, embedded web UI |
| `web/` | Frontend: plain HTML, CSS (design tokens in `css/tokens.css`) and ES modules |
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
cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace
bash scripts/check-branding.sh
node --test tests/web/   # frontend unit tests (Node 22+, no npm install)
```

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
`GSD_SESSION_TTL_DAYS`, `GSD_LOG`. The same settings (minus the data dir) can go in `<data dir>/config.toml`.

## Docker
```bash
docker build -t gsd .
docker compose -f deploy/docker/compose.yaml up -d --build
```

Published images (built by CI on pushes to `main` and `v*` tags, for linux/amd64 and linux/arm64):
`ghcr.io/<owner>/<repo>:latest`, `:main`, `:<version>`, `:sha-<commit>`.
