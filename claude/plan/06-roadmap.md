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

## M1: First server runs, isolated (Vanilla, Paper, Fabric)
- [ ] Mojang manifest provider + `McVersion` ordering; Paper Fill v3; Fabric meta.
- [ ] Java detection + Temurin download (job) into `<data>/runtimes`.
- [ ] Jobs runner + tray UI.
- [ ] Instance create job (sandboxed install): download → verify → EULA from consent → `instance.toml`; port allocation from
      the configured game port range; sandbox UID allocation + chown.
- [ ] `ProcessBackend::Native` + **Linux sandbox** (UID/GID, Landlock fs, rlimits; Landlock net and cgroups when available) +
      isolation level badge + "isolation check" self-test.
- [ ] Supervisor actor: start, graceful stop, kill, state machine, ring buffer, auto-restart + crash-loop guard.
- [ ] Console view, dashboard cards, version picker UI. Permission-aware UI (hide or disable actions).
- [ ] Diagnostics v1: wrong Java, port in use, EULA, out of memory, sandbox denials (Landlock `EACCES` hints).

## M2: All loaders + configuration
- [ ] Forge, NeoForge (installer jobs + args-file launch), Quilt (installer CLI), Purpur.
- [ ] `server.properties` round-trip parser + schema + visual editor + raw view.
- [ ] File manager (list, edit, upload, download, zip, rename, delete, extract) with path safety + ownership assignment.
- [ ] Instance settings (memory, JVM presets, Java from the managed list, auto-start), clone, delete, import an existing folder.
- [ ] User management UI (owner): users, roles, per-instance grants, sessions.

## M3: Safety net
- [ ] Backups: manual, world and full scope, quiesce, retention, restore with a pre-restore backup (re-chown on restore).
- [ ] Scheduler: cron, interval, once and event triggers; actions; conditions; run history; runs with the creator's captured permissions.

## M4: Content: mods **and** plugins together
- [ ] `ContentSource` abstraction; Modrinth search/install for **mods (Fabric/Quilt/Forge/NeoForge) and plugins
      (Paper/Purpur/Folia)**; dependency resolution; packwiz index; enable/disable; remove.
- [ ] Hangar (PaperMC plugin repo) source.
- [ ] Identify unknown jars (Modrinth SHA-1, CF fingerprint, `plugin.yml`/mod metadata); bulk update check + apply.
- [ ] CurseForge (user-pasted API key, server-side only): mods + Bukkit plugins, fingerprints, blocked-download flow.
- [ ] Modpacks: `.mrpack` server install; CurseForge server packs.

## M5: Minecraft-aware admin
- [ ] Log history + crash report viewer.
- [ ] RCON auto-setup (loopback, random password); MCSMP client for 1.21.9+.
- [ ] Players (online, heads, op/whitelist/ban/kick, lookup, history), gamerules editor, worlds manager.
- [ ] Command autocomplete (datagen tree + help parsing + dynamic values).

## M6: Updates, observability, release
- [ ] Update checker + apply with backup and rollback.
- [ ] Metrics + charts; notifications (Discord/webhooks); audit log UI.
- [ ] **nginx reverse-proxy docs + compose example**; trusted proxies; login rate limiting.
- [ ] Multi-arch images (amd64/arm64) on ghcr; systemd `service install` for bare metal (`Delegate=yes`).

## Later (P2/P3)
DockerBackend (container per server) · remote backup targets (S3/SFTP/rclone) · TOTP / passkeys · UPnP · mclo.gs share ·
datapack manager · Chunky · Velocity · Geyser · multi-node agents · other games · self-update · Windows release packaging.
