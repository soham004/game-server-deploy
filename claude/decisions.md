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

**D-028: M2 loaders (Purpur, Quilt, Forge, NeoForge): installers run sandboxed; lessons from real runs** · accepted · 2026-11-07
- **APIs verified live** (fixtures recorded 2026-11-07): Purpur v2 (`/v2/purpur/{mc}/{build}` gives `md5` + `result`; MD5 is
  the only checksum, so `ExpectedHash::Md5` exists), Quilt meta v3 (loader list per game version is **unsorted**: a semver-like
  `catalog::version_order` sorts it), Quilt/Forge/NeoForge Maven `.sha1` side files (all installers are hash-verified),
  Forge `maven-metadata.json` (`"26.3": ["26.3-66.1.0", …]`) + `promotions_slim.json` (★ = recommended, else latest),
  NeoForge's version list (`26.3.0.101` → MC `26.3`, `26.1.2.126` → `26.1.2`, `21.1.77` → `1.21.1`; 1.20.1 not offered).
  Forge is offered from MC 1.7.10 (older builds have no server installer).
- **Install = data**: `install::plan` turns a loader into steps (download, run installer, remove leftovers) + a launch plan;
  one executor runs them. Downloads go through the shared cache keyed by checksum.
- **Installers are untrusted code** and run like servers: `launch::installer_process` builds `java -jar <installer>` in the
  instance's sandbox (own UID, Landlock fs, web ports only, no listening); `supervisor::process::run_to_completion` streams
  their output to the job log, honours cancel, 30-minute timeout. `HostSandbox` (support + startup problem) is shared by the
  supervisors and the installer, so an unusable sandbox fails installs too.
- **Forge/NeoForge launch** is read from the `run.sh` their installer writes (`[exec] java @user_jvm_args.txt @libraries/…/unix_args.txt`),
  path-checked to stay inside the server dir; `run.sh`/`run.bat` are then deleted. Legacy Forge (≤1.16) falls back to its jar.
  Static inspection of the 26.3 installers confirmed the flags (`--installServer`; NeoForge `--install-server`).
- **Real runs (WSL, Landlock ABI 7)**: all four loaders installed and ran. Two bugs found and fixed:
  1. Forge and NeoForge ship `-Djava.net.preferIPv6Addresses=system` in `unix_args.txt`; under the sandbox every JVM with that
     flag fails to bind (`bind(..) failed with error(-22): Invalid argument`; reproduced with Purpur too, IPv6 is enabled on the
     host). Our flags come before the args files, so the install step removes that one flag (logged). Root cause in the
     Landlock/Netty interaction not isolated.
  2. Minecraft exits with code **0** after "Failed to initialize server", which the supervisor counted as a normal stop. Now a
     clean exit **before ready** is a failed start (crash); typing the stop command in the console (`Input::StopTyped`) keeps
     it a stop.
- **Diagnostics**: one diagnostic per code, a later one with the same code replaces it. Bind failures share
  `port_bind_failed`: the generic "FAILED TO BIND" line is replaced by the cause on the next line (-98 in use, -22 the IPv6
  flag, -13 not allowed by the sandbox). Before, every bind failure said "Port already in use".
- The NeoForge console shows a harmless `kqueue … Only supported on OSX/BSD` stack trace (Netty probing; not sandbox-related).
- Instance DTO carries `software` ("NeoForge 26.3"): the loader names live only in `LoaderKind` (the UI's copy had drifted).

**D-029: `server.properties` editor** · accepted · 2026-11-07
- `properties::format` is a round-trip parser following `java.util.Properties` (separators, continuations, escapes, `\uXXXX`
  incl. surrogate pairs, per-line endings). Unchanged lines render byte for byte; edits rewrite only their line, keep the
  original separator, escape on write (non-ASCII as `\u`). Tested with a generated round-trip property test.
- `schema/server_properties.json`: every key of the file a **real 26.3 server generated** (fixture `server_26.3.properties`;
  62 keys), with real defaults. Notable: `white-list=true` and `difficulty=easy` by default; `pvp` is gone (a game rule now);
  `management-server-*` keys exist. A test fails if the real file has a key the schema lacks or a different default.
- Edits carry the file's revision (SHA-256); a changed file → `409 file_changed`. Validation per key → `422 invalid_properties`
  with `details: [{key, message}]` (new `ApiError::with_details`). App-managed keys (`server-port`, `query.port`, `rcon.port`,
  `management-server-port`) can't change: the sandbox only lets a server bind its own port. Writes are atomic and handed to
  the instance user. Viewers (`instance.view` without `properties.write`) get secrets blanked.
- RCON and the management protocol can't work yet inside the sandbox (their ports aren't allowed); the schema says so. M5.

**D-030: Browser end-to-end tests; auth forms never submit natively** · accepted · 2026-11-07
- `tests/e2e/cdp.mjs`: a dependency-free Chrome DevTools Protocol client (Node's built-in WebSocket) driving headless
  Chrome/Edge. `tests/e2e/ui.e2e.mjs` signs in, starts a **real** server from the UI, checks the console has colours and no
  control characters, runs `list`, edits properties (form, invalid value, raw view), stops the server; screenshots on failure.
  Skipped unless `E2E_URL` is set (it needs a running app with an installed server).
- It found a real bug: clicking "Sign in" before the page script attached its handler submitted the form natively as a GET,
  putting the password in the URL. Server-rendered forms now use `method="post"` and start with a disabled submit button that
  `core/forms.js` enables; a static test (`tests/web/pages.test.mjs`) keeps it that way.
- Screenshots showed toasts covering the bottom-right controls (console Send, properties Save); toasts moved top-right.

**D-031: File manager: containment through `cap-std`** · accepted · 2026-11-07
- The app runs as root in Docker and the game server (another UID, possibly compromised) controls the files it browses, so a
  lexical `..` check is not enough: the server can swap a folder for a symlink between check and use. Every operation
  (`gsd-core/files/root.rs`) goes through a `cap_std::fs::Dir` on the server dir, which resolves paths beneath it and
  refuses symlinks out of it race-free (`openat2` + `RESOLVE_BENEATH`). `SafeRelPath` (`files/path.rs`) is a first lexical
  guard (`..`, NUL, `\`, over-long names; Windows reserved names on Windows). Tests plant symlinks to an outside "secret" and
  check every operation refuses.
- Ownership: files the app creates are `chownat(parent_fd, name, AT_SYMLINK_NOFOLLOW)`ed to the instance UID (single
  component, never following links). Not exercised by the test suite (it runs unprivileged); covered by the Docker run.
- Delete moves to `<instance>/trash/<timestamp>-<name>` (root-owned, outside the server's reach); emptying is a later feature.
- Text editing: ≤ 5 MiB, NUL/invalid UTF-8 = binary; saves need the opened revision (SHA-256) → `409 file_changed`; a save
  without a revision only creates new files. Writes are atomic (temp sibling + rename).
- Uploads stream (raw request body, `PUT …/files/upload?path=&overwrite=`) into a hidden temp file, renamed when complete;
  4 GiB cap; nothing partial is left behind. Downloads: always `attachment` + `application/octet-stream` (a hostile HTML
  file can't run on our origin); folders stream as a zip built on the fly; binary responses skip transfer compression.
- Archives: extract `.zip`/`.tar`/`.tar.gz`/`.tgz`; every entry name through `SafeRelPath` (zip slip), links/devices
  skipped, size and entry caps counted on bytes actually written (zip bombs; `ExtractLimits`); compress a selection to zip.
- UI: Files tab (breadcrumbs, table, upload with progress + drag and drop, rename/copy/delete/extract/compress, a
  textarea editor with Ctrl+S). CodeMirror is deferred: the textarea is enough to edit configs; revisit with the Hallmark pass.
- Browser test findings: the editor rendered two lines tall (stylesheet order let `.c-input`'s min-height win; selectors are
  now scoped); toasts piled up over toolbars (now at most three, repeats replace each other, gaps are click-through).
  On the development machine Chrome returns an empty **204 for any `.zip` response**, even from a plain Python server
  (security software filtering browser downloads, most likely); curl gets the files intact. Zip downloads are covered by the
  API tests instead.

**D-032: Instance settings, clone and import** · accepted · 2026-11-07
- Settings (`instances/settings.rs`): `SettingsChange::apply` validates everything before changing anything (name, memory
  512 MB–64 GB with min ≤ max, crash limit, stop timeout, JVM args). Extra JVM args must be options (`-…`), and may not
  contain `-Xmx`/`-Xms` (the memory fields own them), `-jar`/`-cp` (the launch is ours) or
  `-Djava.net.preferIPv6Addresses=system` (D-028). A specific Java runtime must exist, be at least the required major, and
  be managed unless the user holds `runtimes.custom`. Minecraft's console flags (`DEFAULT_JVM_ARGS`) are kept out of the
  form and always re-added, so users can't break the console by deleting them. Changes apply on the next start.
- Clone (`gsd-minecraft/service/clone.rs`): new instance (own id, port, UID, folders), copy of the files through `cap-std`
  without following links (`files::copy_server_dir`), logs and crash reports never copied, worlds (`level-name` + nether/end)
  only on request, `server.properties` pointed at the new port, `auto_start` off. The source must be stopped.
- Import (`gsd-minecraft/import/` + `service/import.rs`): only folders in `<data>/imports/` (an admin copies them in;
  arbitrary host paths would let any admin read the whole disk). Detection: NeoForge/Forge by their library folders
  (exact versions), Quilt/Fabric by launcher jars, Purpur/Paper by config files, else a server jar; the Minecraft version
  from `version.json` inside the jar. The user confirms; the folder is copied (original kept), Forge/NeoForge launch from
  `run.sh` (IPv6 flag stripped), others from the chosen jar; consent → `eula.txt`; own port; ownership.
- Browser tests cover changing settings, cloning without worlds, and importing a **real vanilla 26.3 folder** that then runs
  in the sandbox. They found: checkboxes styled as text inputs, and the console flags shown (and deletable) in the form.

**D-033: User management** · accepted · 2026-11-07
- Owners only (`users.manage`): `GET/POST /users`, `PATCH/DELETE /users/{id}`, `POST …/reset-password`, `POST …/sign-out`,
  `GET …/grants`, `PUT/DELETE …/grants/{instance}` (`gsd-core/auth/service/accounts.rs`, `gsd-app/http/routes/users.rs`).
- New accounts and resets get a random **temporary password, returned once** and never stored in plain text; the user must
  choose a new one at first sign-in. Owners never type someone else's password, so there is nothing to leak in chat logs.
- Rules: no one changes, disables or deletes their own account here (the account page is for that); the app always keeps
  at least one active owner (demote, disable and delete are refused with `last_owner`). Disabling, a reset and "sign out"
  end every session of that user at once; deleting removes their sessions and grants (foreign keys cascade).
- Grants: a preset (viewer, moderator, operator, manager) or `custom` with a permission list; the instance must exist.
  Grants only matter for members: owners and admins see every server.
- Every change is audited (`user.created/updated/deleted/signed_out`, `auth.password_reset`, `grant.changed`).
- UI: Users page (owner nav link only), table with role/status/sessions and actions, add form showing the temporary
  password once with Copy, a per-member access panel. Browser test: the owner adds a member and gives one server; in a
  second browser the member is made to change the password, then sees exactly that server and no Users link; the owner
  deletes the member. The screenshot showed full-width selects stacking in the access panel (fixed).

**D-034: Hard resource limits with cgroup v2** · accepted · 2026-11-07
- `gsd-core/supervisor/sandbox/cgroup.rs`. The app uses **its own control group** (`/proc/self/cgroup`) and only when it
  is clearly its own: writable, `memory` + `pids` delegated, not the real root, and holding no processes but the app (plus
  a container init such as tini, accepted only in a private cgroup namespace). It **never moves another process**: on a
  host, `init.scope` or a login session is refused, so running the binary from a root shell can't drag systemd around.
- Layout: `<own>/app/` (the app moves itself here: v2 forbids processes next to child groups that use controllers),
  `<own>/instances/<instance id>/` per server. Limits: `memory.max` = heap + max(heap/2, 1 GiB) (a 1 GB heap measured
  ~1.6 GB `memory.current` right after start, page cache included); `memory.swap.max` = 0 (a swapping server is
  unplayable and slows the host; a clean stop with an explanation is better); `memory.oom.group` = 1 (stop the whole
  server, not one of its processes); `pids.max` = 2048 (processes + threads). Not per-instance settings yet; no `cpu.weight`.
- Joining: the app opens `cgroup.procs` and the child writes `0` to it in `pre_exec`, first, before rlimits and Landlock.
  It runs after std's setuid; the kernel checks the opener's credentials (Linux ≥ 5.16, and stable kernels that took the backport).
  A startup probe proves this on the host (as the probe UID when root); on failure limits turn off with the reason
  instead of every server start failing.
- Preparing a group kills processes an earlier run left in it (`cgroup.kill`), so leftovers can't eat a new run's budget.
  Installers therefore don't use the server's group (they could kill a running server).
- After an exit, a new `oom_kill` in `memory.events` → console line + `memory_limit` diagnostic ("used all of its N MB
  memory limit…"); before this a kernel kill showed only "signal 9".
- Reporting: `GET /system` → `hard_limits`, `hard_limits_problem`; startup log line; isolation check "Runs under a hard
  memory limit" (the CLI `isolation-check` marks it n/a: only the running app has the right group). Deleting an instance
  removes its group.
- Tests: rules on a fake tree (11); real kernel as root (group, user, limits; OOM kill detected; fork cap; leftovers
  killed; supervisor diagnostic). End to end: the app as `systemd-run -p Delegate=yes` started a real vanilla 26.3
  server as uid 20000 in `instances/<id>` with the expected limits and a passing isolation check; the Docker image
  (root, compose capabilities) reports limits off ("Read-only file system") and every file the app writes into an
  instance (install, upload, folder, extract, clone, import) belongs to that instance's UID.

**D-035: Backups: zip archives, paused saving, staged restore** · accepted · 2026-11-08
- `gsd-core/backups/`. Two scopes: **world** (the folders the adapter names: `level-name` and its `_nether`/`_the_end`)
  and **full** (the server folder minus what the game recreates: `logs`, `crash-reports`, `cache`, `libraries`,
  `versions`, `.fabric`). Custom include/exclude globs are not built.
- Archive: one **zip** per backup, the selected entries under `server/` plus `gsd-backup.json` (backup and instance id,
  software, scope, kind, counts and total bytes). Written in one pass through a hashing writer (`ZipWriter::new_stream`,
  zip64 for ≥ 4 GiB entries), as `*.zip.partial`, renamed when complete; only complete archives get a row in the
  `backups` table (size, SHA-256). The server folder is read through `cap-std`; symlinks and special files are left out.
  `tar.zst` is not offered. Stored in `<data>/backups/<instance id>/` (0700, root only), named
  `YYYYMMDD-HHMMSS-<kind>-<scope>.zip`; the folder isn't configurable yet.
- Kinds: `manual`, `scheduled`, `pre_restore`, `pre_version_change`. Retention per instance (`[backups]` in
  `instance.toml`, `keep` default 10, `keep_safety` default 3): the newest `keep` manual/scheduled and, separately, the
  newest `keep_safety` safety backups stay; pinned backups never count and are never deleted. Runs after each backup.
  GFS (hourly/daily/weekly) and a total-size cap are not built.
- A running server: `save-off`, `save-all flush`, wait for "Saved the game" (`GameSignal::Saved`, default 5 min), archive,
  `save-on`. The console shows them as `[backup] > …`. The resume command is also sent if the guard is dropped (panic).
  If the server stopped or restarted while its files were read, the archive is discarded (`ServerChanged`). A server
  that is starting, stopping or about to restart after a crash is refused (`server_busy`).
- Disk space: the last backup of the same scope (or the selected files' size) + 20 % must be free; a restore needs the
  archive's unpacked size + 20 % (`insufficient_storage`, 507).
- Restore (a job holding the instance lock): verify (file present, SHA-256, manifest names this backup and instance) →
  stop and wait → `pre_restore` backup (default on) → unpack into `<instance>/restore/` with the file manager's safe
  extractor, limited to the manifest's sizes, files owned by the instance's user → swap: replaced top-level entries
  move to `restore-old/` and the backup's move in (renames on one filesystem; undone if any rename fails) → delete the
  staging folders → retention → start again if it was running (`InstanceService::start_from_job`, which skips the busy
  check only for the lock holder). World restores replace only the world folders; full restores replace everything
  except the excluded entries, which stay as they are. Leftover staging folders from an interrupted restore go to the
  instance's `trash/`, never away. Retention runs after the restore, so restoring a safety backup can't expire it.
- API: `GET/POST /instances/{id}/backups`, `PATCH/DELETE …/{bid}`, `POST …/{bid}/restore`, `GET …/{bid}/download`.
  `instance.view` lists; `backups.create` backs up, pins and notes; `backups.restore` restores, deletes and downloads
  (a download holds every world). `backups_changed` event refreshes open pages.
- Deleting an instance leaves its backups for now (no UI reaches them); the "keep backups" choice comes with trash
  cleanup (M3).

**D-036: Scheduler: plans as JSON, one engine task, runs as the last editor** · accepted · 2026-11-08
- `gsd-core/scheduler/`. A schedule is a `SchedulePlan` (name, enabled, trigger, conditions, actions, `on_error`,
  `catch_up`), the wire format and the storage format at once (`schedules.plan_json`), validated by `validate.rs`
  (limits, cron and IANA zone parse, warnings latest-last, console lines without control characters).
- Triggers: `cron` (5 or 6 fields, `croner`) in an IANA zone (`chrono-tz`, daylight saving handled), `interval`
  (60 s … 31 days, counted from the last run), `once`, and `event`: started, stopped, crashed, player joined, nobody
  online for N (once per empty stretch). Conditions: server running/stopped, at least N players, somebody played
  since the last run (from join/leave events, `activity.rs`), time window in a zone. Actions: command, broadcast
  (`GameAdapter::broadcast_command`, `say` for Minecraft), restart/stop with countdown warnings (`[600, 300, 60, 10]`
  → "Restarting in 10 minutes" …), start, backup (`kind: scheduled`), wait. Webhooks, update checks and drag
  reordering are not built (arrows move actions).
- Engine (`engine.rs`): one task; sleeps until the earliest due time (at most 60 s), a change (`Notify`) or a bus event.
  The next time is saved before a run starts. At startup, runs left `running` are closed as failed, and missed time
  triggers run once (`catch_up`) or are recorded as skipped. Deleting a server deletes its schedules (engine sees
  `InstanceDeleted`).
- A run (`run.rs`): record → checks (server exists; the user it runs as exists, is enabled and holds every action's
  permission; conditions, except for "Run now") → the actions in order in a job holding the instance lock → outcome
  `succeeded | failed | skipped` with a message. The lock means runs never overlap each other, an install, a backup or a
  restore: a busy server records "skipped". Restart and start wait until the server is running (10 min), so a following
  backup finds it up; every wait ends on job cancel. `InstanceService::{start,restart}_from_job` skip the busy check
  only for the lock holder.
- Whose permissions: `run_as` is whoever saved the schedule last (an edit takes it over, so nobody borrows another
  user's rights); saving needs every action's permission; a deleted user leaves `run_as` empty (`ON DELETE SET NULL`)
  and runs fail until someone saves it again. Commands show in the console as `[schedule <name>] > …`.
- History: `schedule_runs`, newest 50 per schedule (`cause`: trigger, catch_up, manual). API: `GET/POST
  /instances/{id}/schedules`, `PUT/DELETE …/{sid}`, `POST …/{sid}/run` (202), `GET …/{sid}/runs`. `instance.view`
  reads; `schedules.manage` changes. `schedules_changed` event after changes and every run.
- UI: Schedules tab; editor with "every day at", interval, once, event and raw cron; templates (daily restart with
  warnings, backup every 6 h if somebody played, announcement, save every 10 min). Times use the browser's zone.
- Tests run in real time (one-off triggers a second ahead), including a fresh engine finding missed runs.

**D-037: Trash: named by deletion time, emptied hourly after a retention** · accepted · 2026-11-08
- `gsd-core/trash.rs`. Every trash entry is named `<YYYYMMDD-HHMMSS>-<name>` (servers used `<name>-<stamp>` before;
  both are read). Two places: `<data>/trash/` (deleted servers, and the backups deleted with one, as
  `<stamp>-<slug>-backups`) and `<instance>/trash/` (`InstanceLayout::trash_dir`: files deleted in the file manager,
  leftovers of an interrupted restore).
- The housekeeping task (hourly, with the session purge) removes entries older than `trash_retention_days` (config
  file or `<PREFIX>_TRASH_RETENTION_DAYS`, default 7, `0` keeps everything). An entry without a readable time is
  never removed; symlinks are removed, never followed. Not a user schedule: it isn't per server and needs no permissions.
- Deleting a server: `DELETE /instances/{id}?delete_backups=true` also deletes its backups (rows gone, folder to the
  trash, so they can be recovered until it is emptied). Without it the backups stay in `<data>/backups/<id>/`; nothing
  in the UI lists them yet (roadmap, Later). The UI asks when the server has backups.

**D-038: Changing a server's version: backup, install over, update last** · accepted · 2026-11-08
- `gsd-minecraft/service/change_version.rs`, `POST /instances/{id}/change-version {mc_version, loader_version?,
  allow_downgrade?}` (`instance.update`; the server must be stopped and not busy). The loader stays the same: switching
  loader (Paper → Fabric) is not offered. `loader_version` absent picks the recommended build for the new version.
- Checked before anything happens: the version and build exist (catalog), something actually changes
  (`version_unchanged`), and an older Minecraft version (by release time) is confirmed (`downgrade_not_confirmed`, 409):
  newer worlds can't be loaded by older versions. The UI asks for the server's name for a downgrade.
- The job: a full `pre_version_change` backup (if it fails, nothing is changed) → Java for the new version → the
  normal install pipeline over the existing files (worlds, configs, mods and `server.properties` stay) → for Forge and
  NeoForge, the old build's own folder (the parent of the old args file, only if it is a plain path under `libraries/`)
  is removed → ownership → `instance.toml` gets the new `[server]`, Java major and launch only now; a specifically
  chosen Java runtime that is too old for the new version falls back to automatic. A failure after the backup reports
  "restore the backup <file> to go back".
- Not built: the mod compatibility check (M4) and changing the loader.

**D-039: Mods and plugins from Modrinth: target, resolver, packwiz index** · accepted · 2026-11-08
- `gsd-mods`. A server's **target** (`target.rs`) decides everything: Fabric → mods with `fabric`; Quilt → `quilt`,
  `fabric`; Forge → `forge`; NeoForge → `neoforge`; Paper → plugins with `paper`, `spigot`, `bukkit`; Purpur → `purpur`
  first; vanilla runs neither (`no_content_for_loader`). Folders `mods/` and `plugins/`.
- Modrinth API v2 (checked live 2026-11-08, fixtures recorded by `crates/gsd-mods/tests/fixtures/record.mjs`):
  search with facets (type, the target's loaders as OR, the Minecraft version, server side required or optional);
  versions filtered by loaders and game version and checked again locally; plugins are typed `mod` in search hits but
  the `project_type:plugin` facet still selects them. A 429 waits for `X-Ratelimit-Reset` (≤ 60 s) and retries once;
  410 means "update the app". Ids and slugs are checked before they go into a URL.
- Resolver (`resolver.rs`, pure behind a `Catalog` trait): the project plus required dependencies, breadth first,
  each at the pinned version if it fits, else the newest release, else the newest of any channel. Dependencies already
  installed are kept; the requested project replaces its installed file. `incompatible` relations with anything
  installed or planned stop the plan; cycles end; at most 64 projects. Optional and embedded dependencies are ignored.
- Installing (a job holding the instance lock): download once into `<data>/cache/content/<sha512>.jar` (checked against
  Modrinth's SHA-512), copy in through `ServerFiles` (owned by the server's user, symlink-safe), the replaced file to the
  trash, and a packwiz entry `<folder>/.index/<slug>.pw.toml` (`[update.modrinth] mod-id`, `version`), the format
  Prism and packwiz read. The UI shows the plan (what comes, why, what it replaces) before installing.
- Installed list (`installed.rs`): every `*.jar` / `*.jar.disabled` joined with the index by file name, plus what the
  jar says (`fabric.mod.json`, `quilt.mod.json`, `META-INF/neoforge.mods.toml` or `mods.toml` with the manifest's
  `Implementation-Version` for `${file.jarVersion}`, `paper-plugin.yml` or `plugin.yml`), and its SHA-1. Read and hash
  results are cached per server by file name, size and time. Unreadable jars are listed by file name.
- Turning off = rename to `.jar.disabled` (as Prism); remove = the jar and its index entry to the server's trash.
  `content_changed` event; the tab says to restart when the server runs.
- API: `GET /instances/{id}/content`, `GET …/content/search?q&sort&offset`, `GET …/content/projects/{project}[/versions]`,
  `POST …/content/plan|install {project_id, version_id?}`, `POST …/content/files/{file}/enable|disable`,
  `DELETE …/content/files/{file}`. `instance.view` reads and searches; `content.manage` changes.
- Not yet: the `ContentSource` trait (it comes with the second source, Hangar, so it fits two real ones), updates and
  identifying unknown jars (D-040), CurseForge, modpacks, datapacks.

**D-040: Content updates, identifying hand-added jars, `pre_update` backups** · accepted · 2026-11-08
- `gsd-mods/src/updates.rs`. **Check**: one `POST /version_files/update` with the SHA-1 of every turned-on file that has
  an index entry, filtered by the target's loaders and Minecraft version; an update is a newest version of the same
  project with another version id. For the files with an update, a `POST /version_files` gives the installed version's
  Modrinth number, so "old → new" reads alike (a jar's own version is often shorter). Turned-off files are not offered (updating one would turn it back on). Files without
  an index entry are listed as `unknown`.
- **Identify**: one `POST /version_files` with the unknown files' SHA-1s, then `GET /projects?ids=` for titles and slugs.
  A match gets a packwiz entry pointing at the file as it is named (nothing is renamed or downloaded), so it is tracked
  and updated from then on. A second copy of a project already tracked (or of one found earlier in the same run, by
  file name) is reported as a duplicate and left alone: one project has one index entry. CurseForge fingerprints join
  this when CurseForge comes.
- **Update** (a `content.update` job holding the instance lock): a **full** `pre_update` backup first (a new backup
  kind, migration 0004 rebuilds the table's CHECK; a safety kind like `pre_restore`, so it counts against the safety
  limit and can't push the user's own backups out). This
  departs from the feature plan's "mods folder only": mod updates migrate world data and rewrite configs, which a
  mods-only backup can't undo. A failed backup stops the job before anything changes. Then the chosen files (all when
  none are named) are replaced by their new versions through the install path (D-039), and required dependencies the
  new versions need and the server lacks are resolved and installed too.
- API: `GET /instances/{id}/content/updates` → `{updates: [{file, name, installed_version, project_id, version}],
  unknown}` (`instance.view`); `POST …/content/identify` → `{identified, still_unknown, duplicates}`;
  `POST …/content/update {files?}` → 202 `{job_id, updates}`, `409 up_to_date` when nothing fits (`content.manage`).
  Audit: `content.update_requested`, `content.identified`.
- UI (`components/content-updates.js`): checked when the tab opens and whenever the installed files change; each update
  with its versions and a checkbox (all chosen), "Update selected", and a "Look them up on Modrinth" button for files
  with no known source. Wording in `core/content-plan.js` (unit-tested).
- Tested: API tests against the mock Modrinth (update after a backup, only chosen and turned-on files, a missing
  dependency coming back, identify then update, duplicates, permissions), the browser suite (update through the UI on
  live Modrinth), and `tests/e2e/content.e2e.mjs` on a real Fabric server (older Lithium updated, an untracked Fabric API
  identified, the server loading them).

**D-041: Deny by default: only signing in is open** · accepted · 2026-11-08
- Before: each handler opted in to authentication through its extractor (`CurrentActor`, `Authorized<P>`), and every
  file of the web UI was served to anyone. An audit found all 67 authenticated handlers correct, but one forgotten
  extractor would have made a route public, unknown API paths answered `404` while real ones answered `401` (outsiders
  could map the API), wrong methods got an empty non-JSON `405`, `/meta` (the version) and the setup endpoints stayed
  open after setup, the WebSocket upgrade didn't check `Origin`, and `login.js` accepted `?next=/\evil.test` (browsers
  read it as `//evil.test`: an open redirect after signing in).
- Now `http/access.rs` sits in front of every route and file and needs a valid session (it passes the session on in the
  request's extensions, so extractors don't look it up twice). Open without one: the sign-in page and exactly the files it
  loads (`SIGN_IN_FILES`), `POST /api/v1/auth/login`, `POST /api/v1/auth/logout` (it only clears a cookie; it must work
  for an expired session) and `GET /api/v1/health` (Docker's healthcheck and load balancers; it says only "ok"). Until
  the first account exists: the setup page and its files (`SETUP_FILES`), `GET /api/v1/setup/status`, `POST /api/v1/setup`;
  every page then leads to setup. Everything else: `401 unauthenticated` for anything under `/api/` (whether or not it
  exists, whatever the method), `303` to `/login.html?next=…` for pages (`no-store`), `401` for other files.
- Signed in: wrong methods get JSON `405 method_not_allowed`. The WebSocket upgrade refuses another site's `Origin`
  (`403`), on top of the cookie's `SameSite=Strict`. `safeNext` parses `next` as the browser will and keeps only same-origin
  paths. Views load on demand, so `main.js` sends you to sign in when one fails to load because the session ended.
- Argon2 runs at most 4 at once (about 19 MiB each); sign-in is open to anyone, so a flood of attempts now waits instead
  of using up memory. Per-IP and per-username login rate limiting is still M6 (`plan/06-roadmap.md`).
- Tested: unit tests of every rule (including path tricks); a test that the open file lists are exactly what
  `login.html` and `setup.html` load (following imports); an API test that reads the route table from the source and
  sends every route all five methods without a session (a route added later is covered automatically); before and after
  setup; garbage and signed-out cookies; the WebSocket `Origin`; the browser suite (a deep link signed out → sign-in →
  back; the closed files refuse; `next` can't leave the site).

**D-042: Installing several mods or plugins at once** · accepted · 2026-11-08
- Each search result has a checkbox. A ticked one moves to a "To be installed" list between the installed list and the
  search (`components/content-install-list.js`): the same table as the installed list (Name, Version, From, Remove),
  kept across searches; Remove puts a project back among the results. "Install N mods" makes
  one plan for everything on the list and, once confirmed, one `content.install` job (one restart, one change event).
- `resolver::plan_many` seeds its breadth-first walk with every project asked for, before any dependency: a project
  that another one needs comes as asked for (at its chosen version), a shared dependency is planned once (for whoever
  needed it first), and an installed one is named once. Duplicates are caught by project id even when asked for by
  slug. The plan's project cap went from 64 to 128.
- API: `POST …/content/plan|install` take `{projects: [{project_id, version_id?}]}` (1 to 32) as well as the single
  `{project_id, version_id?}`; mixing them, an empty list or too many is `422 invalid_install_request`. The audit
  entry lists the projects.
- Search results already installed say "Installed" (with Other versions, to go back to an older one); while an install
  runs, "Installing…". Searching starts 200 ms after typing stops (`core/debounce.js`); late answers to older
  searches are dropped.
- Tested: resolver (shared dependencies, a dependency asked for too, an installed one named once); API (three mods and
  their shared dependency in one job, every malformed request); the browser suite on live Modrinth (REI and FerriteCore
  ticked across two searches, untick and tick again, one confirmation naming Cloth Config and Architectury "needed by"
  REI, all four installed); the real Fabric server (Lithium and AppleSkin in one request bring Fabric API and load).

**D-043: Hangar, and more than one source on a server** · accepted · 2026-11-08
- Sources sit behind an enum, `source::Source` (search, project, projects, versions), not a trait object: `async fn` in a
  trait isn't object-safe and there will only be three. This enum is the plan's `ContentSource`. What only one source can
  do stays on its own client (Modrinth's lookups by hash). `Sources` owns them; `ContentService` asks for one by
  `SourceKind`. `ForServer` wraps one source and one server for the resolver.
- **Dependencies stay on the source of what needs them**, since ids mean nothing elsewhere (Hangar's are numbers, as
  CurseForge's will be, so ids alone could collide). A plan with several sources plans each on its own and merges them.
- **One plugin from two sources is one file.** Two copies conflict (the loader reports it and keeps at most one, not
  necessarily the newer), and now that there are two sources it is easy to get there. So `resolver::Installed` also knows
  every file the source doesn't track (hand-added or from another source) by its name, plugin or mod id, compared
  without case or punctuation:
  a dependency with such a name counts as installed, and asking for the project itself replaces that file (the plan says
  so, and the user confirms it). Names are only compared across that boundary: two projects on one source may share a
  name, and ids tell them apart. Asking for one plugin from two sources in one install is `409 cannot_install`, unless one
  is only a dependency of the other side; then the earlier source's copy satisfies it. Sources are planned Modrinth first.
- Replacing a file also removes the index entry that described it when that entry has another name (the slug changed
  between sources), so one file has one entry.
- **Update checks:** Modrinth stays one request for everything, by SHA-1. Hangar has no lookup by hash, so it is one
  versions request per tracked plugin, one after another, on every check (a server with many Hangar plugins waits a few
  seconds; the panel says it is checking). A project gone from Hangar has no update and doesn't hide the others'.
- **What Hangar offers as an update:** the newest release published after the installed version. A snapshot is only
  offered to someone already on a snapshot. Most projects publish snapshots constantly, and someone on a release didn't
  choose them. Modrinth's own update endpoint decides for Modrinth files.
- **Identify stays Modrinth-only:** Hangar can't look up a file by hash.
- `IndexEntry::new(project, version)` now takes the project; `[update.hangar]` also keeps `page-url`, since Hangar's
  addresses need the owner. Files installed earlier without it just show "Hangar" without a link.
- API: `source` (default `modrinth`) on search, project and versions; on `plan` and `install`, both in the single form and
  per project in the list; `source` beside `projects` is `422`. `GET …/content` lists the `sources` for the server;
  search hits carry `page_url`, updates carry `source`, plans carry `not_available`. The install audit entry names projects
  as `source:id`.
- UI: a source picker appears when the server has more than one source (plugin servers); the install list keeps its entries
  across sources and its "From" column says which.
- **Known limits:** a required dependency hosted outside Hangar can't be installed; the plan's confirmation names it
  (`not_available`) so the user can add it by hand. Hangar's project endpoint sends no long page text, so `Project.body` is
  empty. Only versions Hangar hosts itself are offered, and only its newest 25 for the server's Minecraft version are read.
  Name matching can't see a plugin whose jar name differs from its project's title and slug; two such copies are left to
  the loader.
- Tested: resolver (names, sources, what an earlier source brings, hosted elsewhere), update choice (release, snapshot,
  unlisted), API with both mocks (search, install with dependency, update after backup, a gone project, replacing across
  sources, a hand-added dependency, mixed lists), the real "E2E paper" with live Hangar (Chunky replaced from Modrinth,
  ViaBackwards with ViaVersion, all three enable, an older Chunky updates after a backup), and the browser suite on live
  APIs (picker, links, ticking from both sources, the plan, From column, "Installed" per source).

**D-044: CurseForge, with the user's own key** · accepted · 2026-11-09
- **The key:** kept in a new `app_settings` table, in the database no server's sandbox can reach (0600, D-041's reasoning
  applies). `PUT`/`DELETE /settings/curseforge-key` and `GET /settings` need `settings.manage` (owners). It is write-only:
  the API only says whether one is set. It is left out of the audit log, sent in a header marked sensitive to CurseForge's
  API only (never with downloads, whose addresses are elsewhere), and `CurseForge` has a hand-written `Debug`. Saving
  tries the key first (`GET /games/432`): a refused key is `422 curseforge_key_refused` and nothing is saved; if CurseForge
  can't be reached, nothing is saved either. Without a key the source is simply not offered; using it anyway is
  `409 source_key_missing`; a key refused later is `409 source_key_refused`.
- **The client** is built from CurseForge's documented schemas, with a mock made from them, because no key was available
  while building it (`test_support::mock_curseforge`, and `tests/e2e/lib/mock-curseforge.mjs` for the browser). Only
  `tests/e2e/curseforge.e2e.mjs`, run by the user with their key, checks the real thing.
- **What fits:** mods are class 6 and plugins class 5, with the loader as `modLoaderType`, or `modLoaderTypes=[5,4]` on Quilt.
  The files endpoint takes one loader, so with several the filter happens here, from the loader names among a file's
  `gameVersions`. A file naming no loader is a plugin. Server packs are left out. CurseForge says nothing about sides, so
  client-only mods show in results (shown as unknown, not hidden).
- **Files the author blocks** (`downloadUrl` is null): no paused job, unlike the original plan. Installing does everything
  it can and leaves such files out, with the plan's confirmation and a "Download by hand" panel naming them, each with a
  link to its page. The user downloads and adds them (`PUT …/content/upload?file=`, `content.manage`), which puts the jar in
  the folder and identifies it. This keeps no state on the server and needs no job that waits for days; the price is that
  the list is gone once the page is left (planning again brings it back). An upload never overwrites an existing file,
  since that could orphan its index entry.
- **Identify:** Modrinth by SHA-1, then what is left on CurseForge by fingerprint: `MurmurHash2`, seed 1, whitespace bytes left
  out, tested against the values in Apache Kafka's tests, an independent implementation. Fingerprints are worked out only when
  identifying, not for every listing. CurseForge trouble here leaves those files unknown rather than losing what Modrinth found.
- **Updates:** one files request per tracked project, as for Hangar (D-043), and only with a key. A blocked newer version is
  offered; updating backs up, leaves the file out, and the panel lists it.
- **Index:** packwiz's own `[update.curseforge] project-id, file-id` (numbers), plus an optional `page-url`, so entries from
  packwiz and Prism are now read as tracked.
- `GSD_CURSEFORGE_API` (environment only) points the app at the browser suite's stand-in. Whoever sets a process's
  environment already controls it, so this gives nothing away.
- **Known limits:** `Project.body` is empty (long text is a separate request). Only a project's newest 50 files for the
  server's Minecraft version are read. Replacing a file through a by-hand update leaves the old jar next to the new one until
  removed, and identify calls it a second copy. Nothing checks whether a mod is client-only. CurseForge's Minecraft version
  names for new releases are assumed to match ours ("26.3"); the live check will show.
- Tested: fingerprint values; client against both a mock and strict query matchers; API with the mock (key rules including
  roles, restart, audit; mod with dependency and update after backup; Bukkit plugin; blocked file through upload and identify;
  path and file-type refusals; identify order); the browser suite against the stand-in (settings page, wrong key, picker,
  install, by-hand panel, upload, removing the key). Against the real API only: a made-up key is refused with 403.

**D-046: The real CurseForge API in tests, with a key that never ships** · accepted · 2026-11-09
- Tests take `CURSEFORGE_API_KEY` from the environment, or else from `.env` at the repository root (only that variable; an
  empty value counts as none). Nothing prints it. The file's value is used as written, since keys contain `$`.
- `crates/gsd-mods/tests/curseforge_live.rs` checks the client against the real API: search per loader and sort, projects,
  files, dependencies, and that our fingerprint of a real downloaded file is the one CurseForge has. It uses Minecraft 1.20.1,
  whose mods are settled, and skips without a key. So `cargo test` on a machine with a `.env` makes a few real requests.
- **CI:** one step of the Linux job gets the repository secret. Forks and Dependabot get no secrets, so they skip;
  everywhere else `CURSEFORGE_REQUIRED` makes a missing key a failure, so these checks can't go quiet.
  `curseforge.e2e.mjs` (real servers) stays a manual run.
- **Never shipped:** `.env` and `.env.*` are ignored by git and Docker at any depth, the image holds only the compiled binary and the
  entrypoint, and no build step receives the key. `scripts/check-secrets.sh` (in CI and `check.sh`) checks the ignore rules, tracked
  files and for anything shaped like a key; CI also proves with a stand-in `.env` that the file stays out of the build context.
- **What the real API showed (2026-11-09):** the fingerprint matches on a real file; a made-up key gets 403; `modLoaderTypes=[5,4]`
  works; file ids are 32-bit (a bigger one is 400, not "unknown"); "most downloaded" is only roughly ordered, since its order lags
  the live counts. On real servers, JEI with MezzConfig and Fabric API loads on 26.3 and WorldEdit enables on Paper. In that run
  Modrinth named all three identified files first, so the CurseForge branch of identify is proven by the live test and the API
  mock, not on a real server. No blocked file was among the most downloaded projects, so that flow has only met mocks.
- Seen while testing: killing the app outright can leave a server's Java running (parent 1), and the next start then fails with
  `session.lock: already locked`. Not addressed here.

**D-045: Modpacks and uploaded servers** · accepted · 2026-11-09
- **One upload, then a choice the archive makes:** `PUT /modpacks/upload?file=` (`instance.create`) streams it to
  `<data>/cache/modpacks/`, removes uploads older than a day, and looks inside. `modrinth.index.json` is a Modrinth pack,
  `manifest.json` a CurseForge pack; the answer says what it needs, and `POST /modpacks/install` makes the server. Any other
  zip is taken for a server folder and unpacked into `<data>/imports/`, without a single top folder if it has one. The
  existing import takes over from there (loader detection, confirming, copying).
- **One job:** `MinecraftService::create_then` runs the pack's part inside the install job, after the server files and before
  `installed_at`, so a half-filled server can't be started and the job's failure is the install's failure. A failed
  pack leaves an uninstalled server, like any failed install, and retrying that reinstalls without the pack: delete it and
  upload again.
- **An archive is untrusted.** Entries go through `SafeRelPath`, links are skipped, unpacking is capped by bytes actually
  written (16 GB, 200,000 entries) and files reach the server through the file manager. An `.mrpack` file's path is checked the
  same way, and a bad one fails the install. **Its downloads are limited to the hosts the format allows** (`cdn.modrinth.com`,
  `github.com`, `raw.githubusercontent.com`, `gitlab.com`, https only), since otherwise an upload could aim this app's requests
  anywhere, such as a cloud metadata address. Every file is checked against its hash. The configured Modrinth API's own origin
  is allowed too, which is how the tests' mock serves files.
- **What goes in:** files with `env.server: unsupported` are left out; files with no `env` go in, since leaving out a mod a server
  needs is worse. `overrides/` then `server-overrides/`, the later one winning; `client-overrides/` never. For a CurseForge
  manifest, required files only, jars only, into `mods/`.
- **Tracking:** after a pack, and after any import, the jars are looked up (`identify`), so they can be updated. A failed lookup
  only logs: the files work either way.
- **Known limits:** the plan's "search for a pack" isn't built: only uploads, not browsing Modrinth's or CurseForge's packs, so
  CurseForge's `serverPackFileId` isn't followed yet (an official server pack works by uploading its zip). Blocked files in a
  CurseForge pack are named in the job's log, not the by-hand panel; they can be added on the Mods tab. No `[modpack]` record in
  `instance.toml`, and no pack updates. Client-only mods in CurseForge packs aren't detected. `.mrpack`'s fields `file.fileSize` and
  `formatVersion` aren't enforced. The host rule checks a file's first address: GitHub's redirects to its own storage are followed,
  as any other redirect would be, so the rule stops an upload naming a target, not a redirect from an allowed host.
  Files in a pack that Modrinth doesn't know by hash (often hosted on GitHub) work but stay "Unknown", without updates.
- Tested: format parsing; download host rules; API with both mocks (files, overrides order, client-only left out, entry and
  path escapes, a disallowed host, a changed download, broken archives, rights, CurseForge manifest with key and blocked file,
  server zip into imports, import then tracked); a real pack from Modrinth installed and started; the browser suite uploading one.

**D-047: A server stops by itself when the app dies** · accepted · 2026-11-09
- **The problem (seen in D-046):** an app killed outright (crash, out-of-memory killer, `kill -9`) gets no chance to stop its
  servers. Java ignores the broken pipes and keeps running, holding the port and `session.lock`, so every later start of that
  server fails. Reproduced on a real server before the fix: still running 60 s after the kill.
- **The fix, Linux:** every process we start sets `PR_SET_PDEATHSIG` to SIGTERM between fork and exec, so the kernel tells it
  however the app dies. SIGTERM rather than SIGKILL: Java's shutdown hook saves the world, and SIGKILL would lose everything
  since the last autosave. The setting is made after the server's user change (which would reset it), and the child checks its parent
  afterwards, in case the app died just before.
- **A trap to keep in mind:** the signal follows the *thread* that started the process, not the app. Servers are started from
  tasks on tokio's worker threads, which last as long as the app. `block_in_place` can end one, which would stop servers for no
  reason, so it must not be used here: neither the workspace nor its 278 normal dependencies use it (checked by search; tokio itself excluded).
- **A new app doesn't adopt or kill anything.** No pid files: the signal covers the normal cases. A server that hangs while
  stopping stays up with no app to kill it, and a new start then fails; a new diagnostic (`world_locked`) says another process has the
  world open. A start right after a kill can hit this while the old server is still saving; trying again works once it has
  exited. That timing wasn't exercised: the test waits for the old process to exit first.
- **Known limits:** only the started process gets the signal, not what it started (Java starts nothing in normal use). Windows has no
  such signal, so a killed app can still leave a server running there; a job object would solve it, but Windows isn't a release target yet.
- Also found: when the probe user can't run the app binary (say it lives in a private home folder), the start check blames the
  filesystem (`sandbox_unusable`), which is misleading. Not changed here.
- Tested: `tests/e2e/app-killed.e2e.mjs` with a real vanilla server: a block placed seconds before SIGKILL to the app is still there
  after a restart under a new app. It failed before the fix, and a control that also kills Java directly fails with "the world
  was not saved", so the test can tell the difference. Passes as a normal user and as root, where the server ran as UID 20000. A unit
  test checks the signal arrives when the starting thread ends.

**D-048: No `unsafe` in our code** · accepted · 2026-11-09
- **Why:** the user chose Rust for memory safety. Three blocks existed, all in `sandbox/linux.rs`: two `pre_exec` closures and
  a raw `libc::syscall`. The workspace lint is now `unsafe_code = "forbid"`, which can't be allowed again further down.
- **Instead of `pre_exec`, a launcher:** the app starts itself as `gsd sandbox-launch <plan as JSON>`. `launch_if_asked` runs first in
  `main`, which is why `main` no longer uses `#[tokio::main]`: the launcher has one thread. It joins the control group,
  builds the Landlock rules, switches user, enters the folder, sets rlimits, sets the parent-death signal (D-047), restricts itself and
  `exec`s the server. All of it is safe calls through `nix`, `landlock` and std. `exec` keeps the pid, process group and pipes, so the
  supervisor is unchanged.
- **The Landlock version** now comes from the `landlock` crate, which reports it only after restricting the caller, so a thread of its
  own is restricted, reads it and ends. It still reports 7 on this kernel.
- **The launcher is `/proc/self/exe`**, so a server still starts after the binary on disk was replaced by a rebuild or an upgrade.
- **Fails closed:** if a step fails, nothing runs. A sandbox this host could enforce with no launcher configured refuses to start.
- **What behaves differently:**
  - `spawn` returns before the sandbox is applied, so the process joins its control group a moment later. Only one test relied on the
    old order. It failed in CI, and now waits for the process to start.
  - Problems only the launcher can see (a user switch failing, a file that can't be executed) are now a console line and exit 126, so the
    server shows as crashed. A missing program or folder is still an error from starting, because the app checks those first.
  - Joining the control group now happens before dropping root, so it no longer relies on the kernel checking who opened
    `cgroup.procs`.
  - Each start costs one extra `exec`. The plan is visible in `/proc/<pid>/cmdline` for that moment; it holds paths, ports and the
    server's arguments, which were visible there anyway, and not its environment.
- **Tests moved:** the Landlock tests are now in `gsd-app/tests/sandbox.rs`, where Cargo says where the binary is. A test binary can't
  be a launcher, so the root-only kernel tests in `gsd-core` use `target/debug/gsd` and need it built first; CI builds it in the step before.
- `unsafe` remains inside `std`, `tokio`, `nix`, `landlock` and other dependencies, as in any Rust program. This covers only our code.
- Tested: the launcher's result (same program, arguments, environment and folder), a failed launch running nothing, and that removing the
  signal call fails its test. As a normal user: all seven loaders installed or started, a real NeoForge build change, real mods and plugins
  loading, and a server started after its binary was deleted. As root: the isolation check at level Strong (server as UID 30999),
  the 5 kernel tests on five runs in a row. Also `app-killed.e2e.mjs` in both, and the Docker suite. Not run:
  `cgroup-systemd.sh`, so hard limits under real systemd are covered only by the kernel tests.

**D-049: Finding modpacks on a source** · accepted · 2026-11-09
- **Search and versions** for Modrinth (`project_type:modpack`, server-side only) and `CurseForge` (class 4471, which can't say what runs on
  a server), with an optional Minecraft version. Hangar has no packs. A search result leads to the same confirm-and-create step as an upload.
- **Fetching is a job:** a pack can be gigabytes, longer than a reverse proxy keeps a request open. `POST /modpacks/fetch` answers at once with
  an upload id, and `GET /modpacks/uploads/{id}` says what it is once the job has succeeded. Hashes are checked; an unknown version and a file
  only downloadable by hand are refused before anything downloads.
- **A `CurseForge` server pack doesn't go through the import, unlike the plan.** The plan was to import it and detect the loader. The first real
  one (The Vanilla Experience, 26.3) holds `mods/`, `config/` and `server.properties`, with no loader or jar, so it couldn't start and no version
  could be detected. So both files come down: `manifest.json` says which loader to install, and the server pack's files replace the pack's list
  and overrides. A server pack only downloadable by hand falls back to the pack's own files. A pack's own file only downloadable by hand is
  refused even with a good server pack, since nothing else names the loader.
- **A pack can't undo what is settled for this server** (a fix for uploads too, wrong since D-045). Its `server.properties` keeps everything but the
  ports, because the sandbox wouldn't let the server bind the default one, and its `eula.txt` is ignored: the EULA was accepted when it was created.
- **`[modpack]`** in `instance.toml` records source, project, version and names (`file` for an upload). It is saved after the files, and failing to
  save it only logs: the server works either way.
- Real API (2026-11-09): class 4471 is right, and 6 of the 10 most downloaded packs for 1.20.1 have a server pack.
- Tested: API with both mocks (search facets, versions, release over a newer beta, a changed download, server pack with no top folder, its fallback,
  by hand, port and EULA kept, rights); real Modrinth pack found, installed and started; a real `CurseForge` pack with no server pack (Xray Pack) and
  one with (The Vanilla Experience: 150 mods tracked, started), both also as root with the server as UID 20000; the browser suite, whose screenshot
  showed a layout bug its assertions couldn't see.

**D-050: Updating a server's modpack** · accepted · 2026-11-09
- **The lock:** `.modpack-files.json` in the server's folder lists what the pack last installed, with SHA-1s, and its version. In the server's
  folder so that a backup and its restore carry it: after a restore it still describes the files that are there, while `[modpack]` may be ahead, so the
  status reads the version from the lock. The server can edit it, which gains nothing: its paths only reach its own files through the file manager. A
  pack can't bring its own. Without one, an update is refused (`pack_update_unavailable`), since it can't tell whose files are whose.
- **The rule, one pure function (`changes::compare`):** a file changes only if the pack installed it and it still has that hash. A file changed since is
  kept and named; so is one removed since, one turned off, and one that didn't come from the pack. Files the pack doesn't list are never touched.
  This is the plan's v1: no three-way merge. Since mods rewrite their configs on first run, a config can count as changed with no user involved, so the
  review offers "replace these too" (`replace_changed`). That never brings back a turned-off file.
- **An install is the same thing from nothing,** with `replace_changed` so a pack's files replace what the loader seeded, so there is one code path.
- **Review, then apply:** the new version comes from the source (a job on the server, so members who can see it can follow it) or an upload; `plan` changes
  nothing. Applying needs a stopped server, and takes a full `pre_update` backup first. If it fails after that, the error names the backup.
- **The pack decides the versions:** a different Minecraft or loader version is installed as part of the job, with the version change's own code and checks,
  so going to an older Minecraft version is refused. Another loader is refused (`pack_changes_loader`): that is a new server.
- Index entries of removed and replaced jars go, and the lookup afterwards names what is there.
- **A fault found while testing:** a file kept for not being the pack's was first recorded in the lock, so a later "replace" would have deleted it.
- **What is offered:** the newest version for the server's Minecraft that is newer than what is installed and at least as mature. Both parts come from one real pack
  (Adrenaline): it publishes each version for six Minecraft versions, so taking the newest overall offered `26.5.1+mc26.2` to a 26.3 server, a downgrade; and all its 26.3
  builds are `beta`, so "releases only" offered nothing. A server on a release isn't moved to a beta. A pack's move to a newer Minecraft isn't offered; fetching that
  version by id through the API, or uploading it, still works, and the update then installs that Minecraft.
- **Known limits:** servers made before the lock existed can't be updated. Uploading a different pack is allowed and then counts as that pack; the review shows
  its name. No "changelog" is shown. A pack's override that the user changed stays old until they choose to replace it.
- Tested: `compare` table tests; API (owned files, kept with reasons, replace, source and upload, downloaded mod re-tracked, loader version change, another
  loader, not a pack, no source, running server, missing lock, a pack bringing a lock, rights); a real pack already run once and updated (Adrenaline 26.5.0 to
  26.5.1: 9 added, 1 replaced, 8 removed), started afterwards; browser scenario for the Mods tab.
