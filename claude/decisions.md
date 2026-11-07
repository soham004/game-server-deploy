# Decision Log (ADR-lite)

Format: **D-NNN: title** · date · status. Context → Decision → Consequences. Append new entries; supersede old ones
instead of editing them.

**D-001: Rust backend, plain HTML/CSS/JS frontend** · 2026-11-07 · accepted
User requirement. The frontend must stay framework-free so the `/hallmark` design skill can restyle it later. → Native ES
modules, Web Components (light DOM), design tokens in `web/css/tokens.css`. No build step to run the app.

**D-002: Single binary, native processes, no Docker** · accepted
Target users are home/hobby admins. Crafty's success shows native processes are fine for one machine. → Supervise
`java` children directly; Windows Job Objects and Unix process groups for clean kills. Docker isolation is a possible later option.

**D-003: Cargo workspace with 4 crates (app, core, minecraft, mods)** · accepted
Keeps Minecraft and mod-platform logic testable without HTTP, and keeps core game-agnostic behind a `GameAdapter` trait.
Only one adapter exists for now. Keep the trait minimal and grow it when a second game arrives.

**D-004: `instance.toml` is the instance source of truth; SQLite for operational data** · accepted
Portable instance folders (copy between machines, like Prism) + queryable history. Startup reconciles the two.

**D-005: packwiz `.pw.toml` index for mod metadata** · accepted
Interoperable with Prism Launcher and packwiz, human-readable, travels with the instance.

**D-006: stdin/stdout as the baseline control channel; RCON and MCSMP as upgrades** · accepted
stdin always works on every version and loader. MCSMP (1.21.9+) gives structured data and events; RCON is used only when a
captured reply is needed. The backend connects to them over loopback; the browser never gets their secrets.

**D-007: Custom console component instead of xterm.js** · accepted
The MC console is line-oriented, not a TTY. A custom virtualised list allows filtering, search, clickable players and
autocomplete UX.

**D-008: CodeMirror 6 vendored as a prebuilt ESM file** · proposed
The only place a bundler is used, as a one-off vendor refresh script; the app itself needs no build.

**D-009: Command autocomplete from the vanilla data generator + `help` parsing** · accepted
Exact vanilla trees per version; approximate coverage for mods and plugins. See `plan/features/command-autocomplete.md`.

**D-010: sqlx with runtime-checked queries** · proposed
Avoids the compile-time `DATABASE_URL`/offline-data friction. Mitigated by repo-level tests on every query.

**D-011: EULA is never auto-accepted** · accepted
Legal requirement: the user must actively agree. Consent time is recorded in instance.toml and `eula.txt` is written from it.

**D-012: The CurseForge API key is user-supplied and server-side only** · accepted
CF requires a key for all calls. It is never exposed to the browser, and the app respects distribution opt-outs (manual download flow).

**D-013: Safety backups before destructive operations** · accepted
Version change, restore, world reset, update and delete all create a `pre_*` backup by default.

---
*Decisions below come from the user's answers on 2026-11-07.*

**D-014: Linux first, distributed as a Docker image** · accepted · refines D-002
User: "Linux first so that it can be packaged into a docker container later for easy deployment on any platform." → Multi-arch
image (amd64/arm64), `/data` volume, configured game port range, systemd for bare metal. Windows/macOS are dev or home targets
without sandboxing. CI requires Linux. See `plan/07-deployment-and-branding.md`.

**D-015: Multi-user RBAC in v1** · accepted
Global roles (owner/admin/member) + per-instance grants (viewer/moderator/operator/manager/custom). Enforced by a typed
`Authorized<P>` extractor in handlers; services receive an `Actor` for auditing. See `plan/features/security-and-auth.md`.

**D-016: Per-server isolation inside one container; Docker-per-server later** · accepted · refines D-002
User wants one compromised server unable to affect others, preferring the same container. → Each server runs as its own UID/GID,
with a Landlock fs (and, on ABI ≥ 4, net) sandbox, rlimits, and cgroup v2 limits when writable (not by default in Docker).
A `ProcessBackend` trait leaves room for a `DockerBackend` (container per server, opt-in, needs the Docker socket). See
`plan/features/isolation-and-sandboxing.md`.

**D-017: Branding is declared once, set at build time** · accepted
User: name and port must be configurable at build time, or else declared in as few places as possible. → The `gsd-brand` crate
reads `APP_NAME`/`APP_SLUG`/`APP_DEFAULT_PORT` in `build.rs` and exposes consts and derived helpers; the frontend gets them from
`/api/v1/meta` and `{{APP_NAME}}` templating; a CI grep guard blocks hard-coded copies. Docker build args pass them through.

**D-018: TLS via nginx reverse proxy (documented), no built-in TLS in v1** · accepted
User chose nginx over Caddy. The compose example and site config live in `plan/07-deployment-and-branding.md`; trusted-proxy CIDRs gate
`X-Forwarded-*`.

**D-019: Browser-only, no desktop wrapper** · accepted

**D-020: Mods and plugins equally prioritised in v1** · accepted
M4 covers Modrinth (mods + plugins), Hangar, and CurseForge (mods + Bukkit plugins) together.

**D-012 confirmed**: the user supplies their own CurseForge key in Settings.

**D-021: Keep placeholder branding for now** · accepted
Defaults stay `APP_NAME="Game Server Deploy"`, `APP_SLUG=gsd`, `APP_DEFAULT_PORT=8737` until the user picks a real name.
Changing them later is just the build args (D-017).

**D-022: Default game port range 25565–25584 (20 ports)** · accepted
The Docker image default is `<PREFIX>_GAME_PORTS=25565-25584`, mapped 1:1 in the compose example. Query shares the game port;
RCON and management ports are internal (loopback) and allocated outside this range (default internal range `35565–35664`), so they are never published.

**D-024: M0 implementation choices** · accepted · 2026-11-07
- Dependencies are current majors: axum 0.8, sqlx 0.9 (needs the `macros` feature for `migrate!`), tower-http 0.7,
  argon2 0.6 (`hash_password` generates the salt itself, with the `getrandom` feature), ulid 3 (`Ulid::generate()`), toml 1,
  getrandom 0.4. Rust 1.99, edition 2024.
- **No cookie crate**: the session cookie is built and parsed in `gsd-app/src/http/session.rs` (about 30 lines, unit-tested).
- The CSRF header is the brand-neutral `X-CSRF: 1`, so the frontend stays name-free.
- `gsd-app` is a **library plus a thin binary**, so integration tests drive the real router (`tests/api.rs`).
- Migrations are added **per milestone** (only `0001_auth_and_audit.sql` now) rather than creating every table from
  `plan/03-data-model.md` up front.
- First-run setup returns `already_set_up` before validating input; an atomic `INSERT … WHERE NOT EXISTS` guards races.
- Users who must change their password are rejected by `CurrentActor` (`password_change_required`); only `SignedIn` routes
  (`/me`, `/me/password`) admit them.
- The Docker image reads its slug, port and env prefix from the built binary (`gsd brand …` → `/etc/app-brand`), so neither
  the Dockerfile nor the entrypoint contains branding literals.

**D-023: Only owners and admins create instances in v1** · accepted
Members manage only the instances they are granted. Member quotas (instance count, RAM, disk) are a possible later feature.

**D-025: M1 implementation choices and lessons from real runs** · accepted · 2026-11-07
- **Game seam**: the core's `GameAdapter` trait covers only what the supervisor needs (parse a line, detect signals,
  diagnose, stop command). Instance *creation* is game-specific and lives in `gsd-minecraft::service`, which registers
  instances through the core registry. The `[server]` table in `instance.toml` is opaque to the core.
- **Supervisor** = pure state machine (`supervisor/state.rs`) + one actor task per instance. Readers forward lines; the waiter
  reports the exit only after the pipes drain, so the last output always precedes "exited". Clean exit nobody requested
  (someone typed `stop` in the console) counts as a stop, not a crash.
- **Data dir permissions**: `data/` and `instances/` are `0711` (traversable, not listable) so sandbox users can reach
  their own folder; `runtimes/` is `0755`; each instance root is root-owned `0711`, with `server/ home/ tmp/` owned by the
  instance UID (`0700`). `instance.toml` stays out of the writable set.
- **Lesson 1 (real Paper run)**: `/etc/resolv.conf` is a symlink out of `/etc` on WSL (`/mnt/wsl/…`) and with
  systemd-resolved (`/run/systemd/resolve/…`). Landlock checks resolved paths, so DNS failed inside the sandbox. Fix: allow
  the canonical targets of a short list of `/etc` files (`LINKED_CONFIG_FILES`); regression test
  `name_resolution_config_stays_readable`; new diagnostic `network_unreachable`.
- **Lesson 2 (real Paper run)**: Paper's JLine console prefixes lines with `> ` when stdout isn't a TTY, so "Done" wasn't
  detected. Fix: Minecraft instances get `-Dterminal.jline=false -Dterminal.ansi=true` (`DEFAULT_JVM_ARGS`), and the parser
  strips prompt residue; regression test with the exact captured line.
- **Fabric** server launcher jars are built on demand by Fabric meta and have no published checksum (downloaded over HTTPS
  without hash verification; noted in code). Everything else is hash-verified.
- **Tests**: fixtures recorded from the live APIs (`crates/gsd-minecraft/tests/fixtures`), wiremock for upstreams, a shell
  script as fake server/fake `java`, real Landlock tests that skip when the kernel lacks it.
- **CI/GHCR**: multi-arch image built on native amd64 + arm64 runners, pushed by digest and merged into one manifest with
  `org.opencontainers.image.source`, which links the package to the repository. Publishing only after all tests pass,
  only on pushes to `main` and `v*` tags.
- **Verified in Docker** (2026-11-07, production capability set): `isolation-check` reports **Strong** (Landlock ABI 7,
  separate UID): other servers' files unreadable, writes outside its folder blocked, other servers' admin ports
  unreachable. The container becomes healthy and `docker stop` exits 0.

**D-026: Refuse to start servers when the sandbox can't work in the data folder; log lifecycle events** · accepted · 2026-11-07
- **Found by the user**: with `--data-dir ./data` in the repo on `/mnt/e` (WSL's 9p Windows drive), a Paper server
  crash-looped with only `Error: Unable to access jarfile server.jar`. Landlock identifies files by inode, and 9p hands out a
  fresh inode per lookup, so the rule granting the server its own folder never matched. The isolation check passed because
  it ran in `/tmp` (ext4). Reproduced: same check, scratch on `/mnt/e` → "Can write its own folder" fails.
- **Decision**: at startup, run one sandboxed probe in `<data>/sandbox-check/` (0711). If it fails, refuse every start/restart
  with `503 sandbox_unusable` and an actionable message (move the data folder to a Linux filesystem), log it at startup,
  expose it as `sandbox_problem` in `GET /system`, and show a dashboard banner. Rejected: silently dropping Landlock's
  filesystem rules on such filesystems, because per-server isolation is a v1 requirement (D-016) and the user wouldn't know
  it was off. An explicit opt-out can be added later if someone needs network storage.
- `isolation-check` (CLI and API) now runs in the data dir, because that filesystem and its parents' permissions are what
  servers get. This also fixed the API check, which used the private `cache/` dir that sandbox users can't traverse when running
  as root. CI runs the root check with `--data-dir /srv/gsd-ci` (root's home isn't traversable by sandbox users either).
- **Logging**: supervisor lifecycle lines (starting, exited, crashed/restarting, crash loop, spawn failure) are also written to
  the app log with the instance id; detected problems are logged at `warn`, and finished/failed jobs at `info`/`warn`. Game output
  is logged at `debug` (`GSD_LOG=info,gsd_core::supervisor=debug`), off by default because it is high volume.

**D-027: Render ANSI colours in the web console** · accepted · 2026-11-07
- **Found by the user**: Paper output showed invisible characters around coloured text (`ESC[92m…ESC[0m`, e.g. the seed).
  The console displays each server line's `raw` text, which keeps the colour codes (the parsed `text` already strips them).
- **Decision**: keep `-Dterminal.ansi=true` (D-025) and render the codes in the browser (`web/js/core/ansi.js`): SGR
  attributes plus 16-colour (theme tokens `--ansi-0…15`, tuned for the dark console), 256-colour and 24-bit colours. Every
  other escape or control sequence is dropped. Rejected: stripping server-side, because the colours carry meaning
  (highlighted values, warnings) and Prism Launcher users expect them.
- First frontend unit tests: `tests/web/ansi.test.mjs` (Node's built-in runner, no npm dependencies); CI runs them and
  `node --check` on every module.
