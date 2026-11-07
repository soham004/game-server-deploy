# game-server-deploy

A self-hosted game server manager with a Rust backend and a plain HTML/CSS/JS web UI, starting with
Minecraft: Java Edition ("Prism Launcher for servers"). The product name, slug and default port are
build-time settings (see `crates/gsd-brand`).

**Status:** milestone M0 (skeleton): config, SQLite, users and roles, sessions, audit log, web shell,
Docker image. Game server management starts in M1. Plans and research live in [`claude/`](claude/README.md).

## Layout
| Path | What |
|---|---|
| `crates/gsd-brand` | The single declaration of name, slug and default port |
| `crates/gsd-core` | Game-agnostic services: store, auth/RBAC, audit (later: supervisor, jobs, backups…) |
| `crates/gsd-minecraft` | Minecraft adapter (from M1) |
| `crates/gsd-mods` | Modrinth / CurseForge / Hangar (from M4) |
| `crates/gsd-app` | The binary: config, CLI, HTTP API, embedded web UI |
| `web/` | Frontend: plain HTML, CSS (design tokens in `css/tokens.css`) and ES modules |
| `deploy/docker` | Container entrypoint and an example compose file |

## Develop (Linux / WSL2)
```bash
cargo run -p gsd-app -- --data-dir ./data serve
```
Open the URL printed at startup. Debug builds read `web/` from disk, so frontend edits only need a reload.

```bash
cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace
bash scripts/check-branding.sh
```

## Commands
| Command | Purpose |
|---|---|
| `serve` (default) | Run the web server |
| `reset-password <user>` | Give a user a temporary password; they must change it at next sign-in |
| `healthcheck` | Exit 0 if the local server answers (used by the Docker health check) |
| `brand name\|slug\|port\|env-prefix` | Print a branding value |

Global flags: `--data-dir`, `--bind`. Environment variables use the brand prefix (`GSD_` by default):
`GSD_DATA_DIR`, `GSD_BIND`, `GSD_TRUSTED_PROXIES`, `GSD_GAME_PORTS`, `GSD_INTERNAL_PORTS`,
`GSD_SESSION_TTL_DAYS`, `GSD_LOG`. The same settings (minus the data dir) can go in `<data dir>/config.toml`.

## Docker
```bash
docker build -t gsd .
docker compose -f deploy/docker/compose.yaml up -d --build
```
