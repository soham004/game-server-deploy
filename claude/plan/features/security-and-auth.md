# Feature: Security, Auth and Remote Access

The app can run arbitrary commands on the host (it launches Java with user-controlled args and edits files), so a
compromised panel is a compromised machine. Security defaults matter.

## Defaults
- Bare metal: bind `127.0.0.1:<port>` (default from `gsd_brand::DEFAULT_PORT`, configurable). Remote access is an explicit
  setting change, shown with a warning. In Docker: bind `0.0.0.0` inside the container; exposure is controlled by the compose port mapping and nginx.
- First run: a `/setup` page creates the admin (username + password ≥ 10 chars). If the bind is non-loopback before setup
  completes, require a one-time **setup token** printed to the terminal/log (prevents a drive-by takeover on a LAN).
- `gsd reset-password <user>` CLI for recovery.

## Sessions
- Passwords hashed with Argon2id (default params tuned to ~50 ms).
- Session token = 32 random bytes; only its SHA-256 is stored. Cookie `gsd_session`: `HttpOnly`, `SameSite=Strict`,
  `Secure` when served over TLS, `Path=/`. 30-day sliding expiry; "log out other sessions".
- CSRF: SameSite=Strict + a required custom header `X-CSRF: 1` on non-GET requests, plus an `Origin` check.
- Login rate limiting: per IP + per username exponential backoff.
- WebSocket upgrade checks the session cookie and `Origin`.

## Hardening
- Strict CSP: `default-src 'self'; img-src 'self' data: https://cdn.modrinth.com https://media.forgecdn.net; connect-src 'self'; script-src 'self'; style-src 'self'`
  (no inline scripts, which also enforces clean frontend code). `X-Frame-Options: DENY`, `Referrer-Policy: same-origin`.
- Request body limits per route (uploads separately configured).
- File manager path safety (see `file-manager.md`). Never follow symlinks outside the instance root.
- Mod descriptions (markdown/HTML from Modrinth/CurseForge) are sanitised server-side (`ammonia`) before display.
- Secrets (RCON passwords, MCSMP secrets, the CurseForge key) are never returned by the API in plain text (write-only fields).
- RCON and MCSMP bind to loopback only when the app manages them.
- Child processes inherit a minimal environment.

## Remote access (decided 2026-11-07)
- The app serves **plain HTTP**. TLS is terminated by **nginx** as a reverse proxy, documented with a ready-made compose file
  and site config in `plan/07-deployment-and-branding.md`. No built-in TLS in v1.
- `X-Forwarded-For/Proto` are honoured **only** from `TRUSTED_PROXIES` CIDRs. Cookies become `Secure` when the forwarded proto is https.
- Alternatives documented: Tailscale/WireGuard (bind to that interface).

## Multi-user with roles: **in v1** (decided 2026-11-07)
### Model
- **Global role** per user: `owner` (exactly ≥1; everything incl. users and app settings), `admin` (all instances, all
  instance permissions, can create and delete instances; no user or app-setting management), `member` (only what per-instance grants give;
  **cannot create instances** in v1, per D-023).
- **Per-instance grants** for members: a role preset **or** a custom permission set.

| Permission | Viewer | Moderator | Operator | Manager |
|---|---|---|---|---|
| `instance.view` (overview, status, metrics) | ✔ | ✔ | ✔ | ✔ |
| `console.read` · `logs.read` | ✔ | ✔ | ✔ | ✔ |
| `players.manage` (kick, ban, whitelist, op ≤ level 2*) | | ✔ | ✔ | ✔ |
| `console.write` (any command) | | | ✔ | ✔ |
| `lifecycle` (start/stop/restart; kill) | | | ✔ | ✔ |
| `backups.create` | | | ✔ | ✔ |
| `schedules.manage` | | | | ✔ |
| `files.read` · `files.write` | | | | ✔ |
| `properties.write` · `gamerules.write` | | | | ✔ |
| `content.manage` (mods, plugins, packs) | | | | ✔ |
| `backups.restore` · `backups.delete` | | | | ✔ |
| `instance.settings` (memory, JVM args, Java from managed list) | | | | ✔ |
| `instance.update` / `change-version` | | | | ✔ |
| `instance.delete` · custom Java path · sandbox exceptions (extra net ports) | admin/owner only | | | |
\* Granting op level 3–4 needs `console.write` (it is effectively full command access).

### Enforcement (clean flow)
- Permissions are an `enum Permission` in `gsd-core/auth/permissions.rs`; role presets are `const` tables in the same file.
- Handlers declare what they need with a typed extractor: `Authorized<perm::FilesWrite>` resolves the session user, the instance from the
  path, and checks the grant. Missing → `403 forbidden` with `code = "missing_permission"` and the permission name.
- **Services also receive an `Actor`** (user id + effective permissions) for audit logging and for any internal fan-out (for example the
  scheduler acts as a `system` actor with the creating user's permissions captured at save time).
- WebSocket topic subscriptions are checked against the same permissions (`instance:<id>:console` needs `console.read`).
- Console lines and the audit log record which user sent each command.
- The frontend hides what the user can't do (`GET /me` returns effective permissions per instance), but the **backend is the only real check**.

### Why the sandbox matters for RBAC
`content.manage`, `files.write` and `instance.settings` let a Manager run arbitrary code *inside that instance's sandbox*
(upload a jar, change JVM args). Isolation (`features/isolation-and-sandboxing.md`) keeps that contained to the one instance.
Anything that could escape the sandbox (custom Java path, extra network ports, disabling isolation) is **admin/owner-only**.

### User management UI (owner)
Invite or create users (temporary password, forced change on first login), disable or delete, reset password, assign the global role,
per-instance grants matrix, and a sessions list with revoke.

## Later
- TOTP 2FA (P1.5), passkeys (WebAuthn via `webauthn-rs`), OIDC login (P3).
