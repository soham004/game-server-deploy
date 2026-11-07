# Plan 04: HTTP + WebSocket API

Conventions:
- Base path `/api/v1`. JSON in and out. Timestamps are RFC 3339 UTC. IDs are strings.
- Errors: `{"error": {"code": "server_running", "message": "Stop the server first.", "details": {...}}}` with the proper status.
- Long operations return **202** with `{"job_id": "…"}`. Progress arrives over the WebSocket (`job:<id>` topic) and via `GET /jobs/{id}`.
- Auth: session cookie (name from `gsd_brand`; `HttpOnly; SameSite=Strict; Secure` behind https). State-changing requests also need the
  generic header `X-CSRF: 1` (brand-neutral on purpose, so the frontend stays name-free).
- Authorisation: every route declares its `Permission` (see `features/security-and-auth.md`); `403 missing_permission` otherwise.
- Pagination: `?limit=&cursor=`.

## Endpoints (v1 target)

### System and auth
| Method | Path | Purpose |
|---|---|---|
| GET | `/health` | liveness (no auth) |
| GET | `/setup/status` | has an admin been created? (first-run wizard) |
| POST | `/setup` | create the first admin |
| POST | `/auth/login` · `/auth/logout` | session |
| GET | `/meta` | `{name, slug, version}` from `gsd_brand` (no auth; the frontend never hard-codes the name) |
| GET | `/me` | current user + global role + effective permissions per instance |
| GET/POST | `/users` · PATCH/DELETE `/users/{id}` · POST `/users/{id}/reset-password` | user management (owner) |
| GET/PUT/DELETE | `/instances/{id}/grants/{user_id}` | per-instance role or permissions (owner/admin) |
| GET/DELETE | `/me/sessions` · `/users/{id}/sessions` | list and revoke sessions |
| GET | `/instances/{id}/isolation` · POST `/isolation/check` | achieved isolation level + self-test |
| GET/PATCH | `/settings` | app settings (CurseForge key write-only) |
| GET | `/system` | host info: OS, CPUs, RAM, disk free, app version, LAN/public IP, isolation level, `sandbox_problem` (D-026) |

### Catalog (version pickers)
| GET | `/catalog/minecraft/versions?types=release,snapshot` | from the Mojang manifest |
| GET | `/catalog/loaders/{loader}/versions?mc=26.3` | loader versions for an MC version (with `recommended`, `stable` flags) |
| GET | `/catalog/loaders?mc=26.3` | which loaders support this MC version |

### Instances
| GET | `/instances` | list with state, players, resource summary |
| POST | `/instances` | create `{name, mc, loader, loader_version, memory, eula_accepted, java?}` → 202 job |
| POST | `/instances/import` | from an existing folder, zip, `.mrpack` or CurseForge zip → 202 job |
| GET/PATCH/DELETE | `/instances/{id}` | details, update definition, delete (with a `confirm` body) |
| POST | `/instances/{id}/clone` | → 202 |
| POST | `/instances/{id}/change-version` | `{mc?, loader?, loader_version?}` → 202 (auto backup) |
| POST | `/instances/{id}/start` · `/stop` · `/restart` · `/kill` | lifecycle |
| POST | `/instances/{id}/command` | `{command}` → write to stdin |
| GET | `/instances/{id}/console?lines=500` | recent ring-buffer lines |
| GET | `/instances/{id}/complete?input=/tp Al&cursor=7` | autocomplete suggestions |
| GET | `/instances/{id}/diagnostics` | current detected problems |

### Logs
| GET | `/instances/{id}/logs` | list log files (latest, archived, crash reports) |
| GET | `/instances/{id}/logs/{name}?q=&level=&from=&to=` | read with filter (gz decompressed server-side, paged) |
| POST | `/instances/{id}/logs/{name}/share` | upload to mclo.gs (explicit user action) |

### Files
| GET | `/instances/{id}/files?path=` | list a directory |
| GET | `/instances/{id}/files/content?path=` | read text (size-limited) |
| PUT | `/instances/{id}/files/content?path=` | write text `{content, expected_mtime}` (optimistic concurrency) |
| GET | `/instances/{id}/files/download?path=` | file, or a folder as a streamed zip |
| POST | `/instances/{id}/files/upload?path=` | multipart / chunked |
| POST | `/instances/{id}/files/ops` | `{op: mkdir|rename|move|copy|delete|extract|compress, …}` |

### Properties, gamerules, players, worlds
| GET/PUT | `/instances/{id}/properties` | structured `{entries:[{key,value,schema?}]}` · raw text via `?format=raw` |
| GET | `/schema/server-properties?mc=26.3` | editor schema |
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

### Backups, schedules, updates, jobs, Java, notifications
| GET/POST | `/instances/{id}/backups` | list / create `{scope, note}` → 202 |
| POST | `/instances/{id}/backups/{bid}/restore` → 202 · DELETE · PATCH (pin, note) · GET `/download` |
| GET/POST | `/instances/{id}/schedules` · PATCH/DELETE `/schedules/{sid}` · POST `/schedules/{sid}/run` · GET `/schedules/{sid}/runs` |
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
