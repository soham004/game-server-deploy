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
