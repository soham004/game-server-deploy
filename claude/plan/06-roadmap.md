# Plan 06: Roadmap and Milestones

Each milestone ends with something usable and demoable. Check boxes as work lands.
Priorities updated 2026-11-07 after the user's answers: **Linux-first + Docker**, **multi-user RBAC in v1**, **per-server
isolation**, **mods and plugins equally**, CurseForge with a **user-supplied key**, nginx for TLS, **browser-only**, branding set at build time.

## M0: Skeleton
- [x] Toolchain: WSL2 Ubuntu 26.04 with Rust 1.99.1 (kernel 6.18 with Landlock), plus Docker Desktop. Edit on Windows, build and test in WSL
      (see `open-questions.md` → Environment).
- [x] Workspace: `gsd-brand`, `gsd-app`, `gsd-core`, `gsd-minecraft` (stub), `gsd-mods` (stub); pedantic clippy lints, rustfmt;
      CI workflow (Linux fmt/clippy/test/branding, Windows test, Docker build + health).
- [x] `gsd-brand` build script (APP_NAME / APP_SLUG / APP_DEFAULT_PORT, empty = default, validated) + `scripts/check-branding.sh`.
      Verified: a rebranded build ("Blocky Panel", `blocky`, 9123, `BLOCKY_`) and a failing build for an invalid slug.
- [x] `serve`: config (CLI > env > file > brand defaults, `deny_unknown_fields`), tracing, SQLite (WAL) + embedded migrations,
      `/health`, `/meta`, static `web/` (disk in debug, embedded in release, `{{APP_NAME}}`/`{{APP_VERSION}}` templating, ETag),
      security headers (strict CSP), graceful shutdown, hourly session purge.
- [x] Auth foundation: first-run owner setup (setup token when bound to a non-loopback address), login/logout, sliding sessions
      (SHA-256 of token stored), forced password change after `reset-password`, CSRF (`X-CSRF: 1` + Origin check),
      users + global roles + per-instance grants, `Authorized<perm::X>` / `AuthorizedGlobal<perm::X>` extractors, audit log.
- [x] Frontend shell: tokens, layout, router, store, api.js, forms, toasts, login/setup pages, role-aware empty dashboard.
      `ws.js` deferred to M1, when the first WebSocket endpoint exists.
- [x] Dockerfile + compose (app only). Verified 2026-11-07 on Docker Desktop: image 33.5 MB, reports healthy, runs with
      only `SETUID SETGID CHOWN DAC_OVERRIDE FOWNER KILL`, `/data` is `700` and the DB `600`, the setup-token gate activates
      (bound to `0.0.0.0` in the container), setup works only with the token, and `docker stop` shuts down cleanly (exit 0).
- [ ] Login rate limiting (moved to M6, as planned).

## M1: First server runs, isolated (Vanilla, Paper, Fabric) — done 2026-11-07
- [x] Catalog: Mojang manifest (sorted by release time, not id), Paper Fill v3 (stable builds, URLs from responses), Fabric
      meta (launcher jar). Memory + disk cache with stale-on-error fallback. Fixtures recorded from the live APIs.
- [x] Java detection (managed dir, `JAVA_HOME`, `PATH`, platform dirs; `release` file or `-XshowSettings`) + Temurin download
      job (sha256-verified, unpacked world-readable for sandbox users).
- [x] Jobs runner (progress, log, cancel, per-instance lock, panic capture, bounded history) + header task tray.
- [x] Instance create: synchronous validation against the catalog → registry allocates slug/port/UID → install job (Java,
      jar via content-addressed cache, `eula.txt` from consent, seeded `server.properties`, chown) → `installed_at`.
      Retryable install. Delete moves the folder to `trash/`.
- [x] Linux sandbox: per-instance UID/GID (when root), Landlock fs + net (ABI ≥ 4), rlimits, `no_new_privs`. `isolation-check`
      CLI + `POST /system/isolation-check` + isolation level in `/system`; startup check that the sandbox works in the data dir (D-026). cgroup limits came in M2 (D-034).
- [x] Supervisor actor: pure state machine (start/stop/restart/kill, stop timeout → kill, crash backoff 5/15/45/120 s,
      crash-loop guard), ordered output (exit reported after pipes drain), console history, players, diagnostics.
- [x] WebSocket stream (`instances`, `jobs`, `console:<id>` topics; per-event permission checks; session re-validation).
- [x] UI: live dashboard cards, Prism-style version picker, instance page (lifecycle buttons by permission, console with
      history and follow-tail, overview with connect address/players/problems), job tray.
- [x] Diagnostics: port in use, EULA, wrong Java (names the version), OOM, overload, missing dependency, client-only mod,
      mixin failure, network/DNS unreachable, crash loop.
- [x] Verified with a **real Paper 26.3 server** under Landlock in WSL2 (see decisions D-025).
- [x] cgroup v2 memory/pids limits (when writable): done in M2 (D-034).

## M2: All loaders + configuration
- [x] Forge, NeoForge (installer jobs + args-file launch), Quilt (installer CLI), Purpur. Installers run sandboxed; all four
      verified with real 26.3 installs and starts under Landlock (D-028).
- [x] `server.properties` round-trip parser + schema (from a real 26.3 file) + visual editor + raw view (D-029); browser
      end-to-end test (D-030).
- [x] File manager (list, edit, upload, download, zip, rename, delete, extract) with path safety + ownership assignment
      (D-031).
- [x] Instance settings (memory, JVM presets, Java from the managed list, auto-start), clone, delete, import an existing folder
      (D-032).
- [x] User management UI (owner): users, roles, per-instance grants, sessions (D-033).
- [x] cgroup v2 hard limits (memory ceiling, task cap, out-of-memory diagnostic) when the app's group is delegated;
      verified under systemd with `Delegate=yes`; off with the reason in Docker's default setup (D-034).

## M3: Safety net
- [x] Backups: manual, world and full scope, quiesce (save-off/flush/save-on), keep-last-N retention with pins and a
      separate safety limit, staged restore with a pre-restore backup, files owned by the server user (D-035).
- [x] Scheduler: cron (with time zone), interval, once and event triggers; conditions; command, broadcast, restart/stop
      with warnings, start, backup and wait actions; run history; catch-up after downtime; runs with the last editor's
      permissions, checked at every run (D-036).
- [x] Change version (Minecraft or loader build, same loader): world-conversion warning, typed confirmation for
      downgrades, mandatory `pre_version_change` backup, old Forge/NeoForge build folder removed, `instance.toml`
      updated last (D-038). The mod compatibility check comes with M4.
- [x] Trash: entries named by deletion time, emptied hourly after `trash_retention_days` (default 7); deleting a
      server asks whether to delete its backups too (D-037).

## M4: Content: mods **and** plugins together
- [x] `ContentSource` abstraction (an enum, `source::Source`, D-043); Modrinth search/install for **mods
      (Fabric/Quilt/Forge/NeoForge) and plugins (Paper/Purpur/Folia)**; dependency resolution; packwiz index;
      enable/disable; remove.
- [x] Hangar (PaperMC plugin repo) source: search, install with dependencies, update checks (one request per
      project), a source picker on plugin servers; one plugin from two sources stays one file (D-043).
- [x] Identify unknown jars (Modrinth SHA-1, CF fingerprint, `plugin.yml`/mod metadata); bulk update check + apply
      (D-040, D-044).
- [x] CurseForge (user-pasted API key, server-side only): mods + Bukkit plugins, fingerprints, blocked-download flow
      (D-044), checked against the real API in CI and on real servers (D-046). A real blocked file hasn't turned up yet.
- [x] Modpacks by upload: `.mrpack` and CurseForge manifest packs become a server in one job (D-045).
- [x] Modpacks by search: browsing Modrinth and CurseForge packs, following CurseForge's `serverPackFileId`, recording the
      pack in `instance.toml` (D-049).
- [x] Pack updates: from the pack's source or an upload, reviewed first, after a full backup, changing only what the pack owns (D-050).
- [x] Import from an uploaded zip / server pack (unpacked into the imports folder, then the usual folder import), and identify
      the imported `mods/*.jar` into the packwiz index (D-045).

## M5: Minecraft-aware admin
- [x] Log history + crash report viewer (D-051).
- [x] MCSMP set up automatically on 1.21.9+ (loopback, random secret) with a client; RCON client, on older servers, only if the user
      turns it on: it can't be bound to loopback (D-052).
- [x] Players: online, heads, op/allowlist/ban/kick/message, lookup by name, visit history (D-053).
- [x] Gamerules editor: live, through the console, or in a stopped world's own file (D-054).
- [x] Worlds manager: facts from the files, make active, download, upload, delete and reset after a backup (D-055).
- [x] Command autocomplete: the data generator's tree and ids per version, mods' and plugins' commands from `help`, players online (D-056).
- [x] Code editor: CodeMirror 6 vendored as one ESM file (D-008; deferred in D-031) for the file manager, the raw
      `server.properties` view and the log viewer: syntax highlighting (json, yaml, toml, properties, cfg/ini, mcfunction,
      snbt, log), line numbers, search. Check it works under the strict CSP (`style-src 'self'`) (D-057).
- [x] Editor checks: JSON/YAML/TOML validation on save with the error location; diff view before saving (P2) (D-058).
- [ ] File manager extras (`features/file-manager.md` → UI): folder tree pane, multi-select, context menu, shortcuts
      (F2, Del, Ctrl+C/V), image preview, "set server icon" helper (resize to 64×64).

## M6: Updates, observability, release
- [ ] Update checker + apply with backup and rollback.
- [ ] Metrics + charts; notifications (Discord/webhooks); audit log UI.
- [ ] **nginx reverse-proxy docs + compose example**; trusted proxies; login rate limiting.
- [ ] Multi-arch images (amd64/arm64) on ghcr; systemd `service install` for bare metal (`Delegate=yes`).

## Later (P2/P3)
DockerBackend (container per server) · remote backup targets (S3/SFTP/rclone) · TOTP / passkeys · UPnP · mclo.gs share ·
datapack manager · Chunky · Velocity · Geyser · multi-node agents · other games · self-update · Windows release packaging ·
member quotas: server count, RAM, disk (D-023) · backups beyond D-035: GFS retention (hourly/daily/weekly/monthly)
and a total-size cap, custom include/exclude globs, `tar.zst` archives, a configurable backup folder (another disk),
progress while archiving, restoring a single dimension, uploading a backup as a new instance, listing the backups of deleted servers · switching a server to another loader (Paper → Fabric) · scheduler beyond
D-036: webhook and update-check actions, a "backup finished" event trigger, drag reordering of actions · import "adopt in place" (`server_root` outside `instances/`; needs a
sandbox and ownership review) · opt-out of the network-filesystem refusal for the data dir (D-026) · real product name
(placeholders until the user picks one, D-021).
