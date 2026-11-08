# Plan 04: HTTP + WebSocket API

Conventions:
- Base path `/api/v1`. JSON in and out. Timestamps are RFC 3339 UTC. IDs are strings.
- Errors: `{"error": {"code": "server_running", "message": "Stop the server first.", "details": {...}}}` with the proper status.
- Long operations return **202** with `{"job_id": "…"}`. Progress arrives over the WebSocket (`job:<id>` topic) and via `GET /jobs/{id}`.
- Auth: session cookie (name from `gsd_brand`; `HttpOnly; SameSite=Strict; Secure` behind https). State-changing requests also need the
  generic header `X-CSRF: 1` (brand-neutral on purpose, so the frontend stays name-free).
- Deny by default (D-041): without a valid session every request is refused before routing (`401 unauthenticated`, the
  same for paths that don't exist); pages redirect to `/login.html`. Open: `POST /auth/login`, `POST /auth/logout`,
  `GET /health`, the sign-in page's files, and, only until the first account exists, setup.
- Authorisation: every route declares its `Permission` (see `features/security-and-auth.md`); `403 missing_permission` otherwise.
- Pagination: `?limit=&cursor=`.

## Endpoints (v1 target)

Sections marked **(built)** describe what exists after M2; the rest is the plan.

### System and auth (built, except where noted)
| Method | Path | Purpose |
|---|---|---|
| GET | `/health` | liveness (no auth) |
| GET | `/setup/status` | has an admin been created? (first-run wizard; no auth only until the first account exists) |
| POST | `/setup` | create the first owner `{username, password, setup_token?}` (same) |
| POST | `/auth/login` · `/auth/logout` · `/me/password` | session; change own password |
| GET | `/meta` | `{name, slug, version}` from `gsd_brand` (signed in; the frontend never hard-codes the name) |
| GET | `/me` | current user + global role + effective permissions per instance |
| GET/POST | `/users` | list (with active session counts) · create `{username, role}` → `{user, temporary_password}` (owner, D-033) |
| PATCH/DELETE | `/users/{id}` | `{role?, disabled?}` · delete; never your own account, never the last active owner |
| POST | `/users/{id}/reset-password` · `/users/{id}/sign-out` | new temporary password · end every session |
| GET | `/users/{id}/grants` | the member's per-instance grants |
| PUT/DELETE | `/users/{id}/grants/{instance_id}` | `{grant: viewer|moderator|operator|manager|custom, permissions?}` · remove |
| GET/DELETE | `/me/sessions` | list and revoke own sessions (planned) |
| POST | `/system/isolation-check` | sandbox self-test, including the hard memory limit (D-034) |
| GET/PATCH | `/settings` | app settings (CurseForge key write-only) (planned) |
| GET | `/system` | version, OS, isolation level, `sandbox_problem` (D-026), `hard_limits` + `hard_limits_problem` (D-034), game ports |

### Catalog (version pickers, built)
| GET | `/catalog/minecraft/versions` | every Minecraft version, newest first, from the Mojang manifest (the UI filters by kind) |
| GET | `/catalog/minecraft/loaders?mc=26.3` | each loader's availability and versions for that Minecraft version (`recommended`, `stable` flags) |

### Instances (built: list, create, show, delete, settings, clone, import, lifecycle, command, console)
| GET | `/instances` | list with state, players, `software` label |
| POST | `/instances` | create `{name, mc_version, loader, loader_version?, max_memory_mb, eula_accepted}` → 202 job |
| GET/POST | `/imports` | folders in `<data>/imports/` with what was detected · import `{folder, name, mc_version, loader, jar?, …}` → 202 (D-032) |
| GET/DELETE | `/instances/{id}` | details, delete (stopped only) |
| GET/PATCH | `/instances/{id}/settings` | name, memory, JVM preset and extra args, Java runtime, auto-start, restart rules (D-032) |
| POST | `/instances/{id}/clone` | `{name, include_worlds}` → 202 (stopped source only) |
| POST | `/instances/{id}/install` | re-run the loader install → 202 |
| POST | `/instances/import` | zip, `.mrpack` or CurseForge zip → 202 job (planned, M4) |
| POST | `/instances/{id}/change-version` | `{mc_version, loader_version?, allow_downgrade?}` → 202 `{job_id}`; full backup first; same loader; stopped server (built) |
| POST | `/instances/{id}/start` · `/stop` · `/restart` · `/kill` | lifecycle |
| POST | `/instances/{id}/command` | `{command}` → write to stdin |
| GET | `/instances/{id}/console?lines=500` | recent ring-buffer lines |
| GET | `/instances/{id}/complete?input=/tp Al&cursor=7` | autocomplete suggestions |
| GET | `/instances/{id}/diagnostics` | current detected problems |

### Logs
| GET | `/instances/{id}/logs` | list log files (latest, archived, crash reports) |
| GET | `/instances/{id}/logs/{name}?q=&level=&from=&to=` | read with filter (gz decompressed server-side, paged) |
| POST | `/instances/{id}/logs/{name}/share` | upload to mclo.gs (explicit user action) |

### Files (built, D-031)
| GET | `/instances/{id}/files?path=` | list a directory |
| GET/PUT | `/instances/{id}/files/text` | read `?path=` (≤ 5 MiB) with its `revision` · write `{path, content, revision?}` (`409 file_changed` if it moved on) |
| GET | `/instances/{id}/files/download?path=` | file, or a folder as a streamed zip (always an attachment) |
| PUT | `/instances/{id}/files/upload?path=&overwrite=` | raw body streamed to a temp file, renamed when complete |
| POST | `/instances/{id}/files/folder` · `rename` · `copy` · `delete` · `extract` · `compress` | one operation each; delete moves to the instance's trash |

### Properties, gamerules, players, worlds
| GET/PUT | `/instances/{id}/properties` | values + schema + `revision` · `{revision, changes}` (key → value or null) or `{revision, text}`; `422` with per-key `details`, managed keys (ports) refused (built, D-029) |
| GET/PATCH | `/instances/{id}/gamerules` | via MCSMP or commands (running only) |
| GET | `/instances/{id}/players` | online + known players, ops, whitelist, bans |
| POST | `/instances/{id}/players/actions` | `{action: op|deop|whitelist_add|whitelist_remove|ban|pardon|kick|ban_ip, target, reason?}` |
| GET | `/players/lookup?name=` | name → UUID (cached) |
| GET | `/players/{uuid}/head.png` | rendered head (cached) |
| GET | `/instances/{id}/worlds` · POST `/worlds/{name}/reset` · upload/download | world manager |

### Mods / plugins / packs
| GET | `/content/search?source=modrinth&type=mod&q=&instance={id}` | search pre-filtered for the instance |
| GET | `/content/{source}/{project_id}?instance={id}` | details + compatible versions |
| GET | `/instances/{id}/content` | installed mods/plugins/datapacks with metadata + update flags |
| POST | `/instances/{id}/content` | install `{source, project_id, version_id?}` → 202 (resolves deps) |
| PATCH | `/instances/{id}/content/{file}` | enable/disable |
| DELETE | `/instances/{id}/content/{file}` | remove |
| POST | `/instances/{id}/content/check-updates` → 202 · POST `/content/update` `{files:[…]}` → 202 |
| POST | `/instances/{id}/content/manual-match` | upload blocked CurseForge files, matched by fingerprint |

### Mods and plugins (built: Modrinth, D-039, D-040)
| GET | `/instances/{id}/content` | installed `{kind, folder, target, items}` |
| GET | `/instances/{id}/content/search?q=&sort=&offset=` | Modrinth, filtered to the server |
| GET | `/instances/{id}/content/projects/{project}` · `…/versions` | project page, fitting versions |
| POST | `/instances/{id}/content/plan` · `…/install` | `{project_id, version_id?}` → plan / 202 `{job_id, plan}` |
| POST/DELETE | `/instances/{id}/content/files/{file}/enable` · `…/disable` · `DELETE …/files/{file}` | turn on, off, remove |
| GET | `/instances/{id}/content/updates` | `{updates: [{file, name, installed_version, project_id, version}], unknown}` |
| POST | `/instances/{id}/content/identify` | name hand-added jars by SHA-1 → `{identified, still_unknown, duplicates}` |
| POST | `/instances/{id}/content/update` | `{files?}` (all when absent) → 202 `{job_id, updates}` after a full `pre_update` backup · `409 up_to_date` |

### Backups, schedules, updates, jobs, Java, notifications
| GET/POST | `/instances/{id}/backups` | list `{backups, policy, total_bytes}` / create `{scope, note?}` (scope `world` or `full`) → 202 `{job_id}` (built) |
| POST | `/instances/{id}/backups/{bid}/restore` `{safety_backup?=true}` → 202 `{job_id}` · PATCH `{pinned?, note?}` (empty note removes it) · DELETE → 204 · GET `…/download` (built) |
| GET/POST | `/instances/{id}/schedules` | list `{schedules}` (each: `plan`, `description`, `run_as`, `next_run_at`, `last_run`) / create a `SchedulePlan` → 201 (built) |
| PUT/DELETE | `/instances/{id}/schedules/{sid}` | replace the plan (runs as the editor from then on) / delete → 204 · POST `…/{sid}/run` → 202 · GET `…/{sid}/runs` (built) |
| GET | `/instances/{id}/updates` | server and loader update status |
| POST | `/instances/{id}/updates/apply` → 202 |
| GET | `/jobs?active=true` · GET `/jobs/{id}` · POST `/jobs/{id}/cancel` |
| GET | `/java` (detected + managed) · POST `/java/install` `{major}` → 202 · DELETE `/java/{id}` |
| GET/POST/DELETE | `/webhooks` · POST `/webhooks/{id}/test` |
| GET | `/audit?instance=&limit=` |
| GET | `/instances/{id}/metrics?range=1h|24h|7d` |

## WebSocket `/api/v1/ws`
One connection per tab. Client → server:
```json
{"op":"subscribe","topics":["instances","instance:01JC…:console","instance:01JC…:metrics","jobs"]}
{"op":"unsubscribe","topics":["instance:01JC…:console"]}
{"op":"command","instance":"01JC…","command":"say hi"}     // low-latency console input (also available via REST)
```
Server → client:
```json
{"topic":"instance:01JC…:console","type":"line","data":{"ts":"…","level":"INFO","thread":"Server thread","text":"…"}}
{"topic":"instances","type":"state","data":{"id":"01JC…","state":"running","players":3}}
{"topic":"jobs","type":"progress","data":{"id":"…","pct":42,"step":"Downloading libraries"}}
{"topic":"*","type":"resync","data":{"reason":"lagged"}}
```
- Topics are authorised per user (future RBAC).
- Console subscribers first get the last N lines (snapshot), then live lines, so the UI never shows a gap.
- Heartbeat ping every 20 s. The client reconnects with backoff and resubscribes.
