# Feature: Update Checker (server jar, loader, mods)

## What can be "updated"
| Thing | Same MC version (safe-ish) | New MC version (risky, world upgrade) |
|---|---|---|
| Vanilla | n/a | new release available |
| Paper/Purpur/Folia | **new build** for the same MC version (frequent; security and bug fixes) | new MC version supported |
| Fabric/Quilt | new loader version | new MC version |
| Forge/NeoForge | new loader build for the same MC version (recommended vs latest) | new MC version |
| Mods/plugins | new compatible version | (part of a version change) |
| Java | newer patch of the same major (managed runtimes) | n/a |

## Checker
- A background task every 6 h (setting) and on demand: for each instance, compute an `UpdateStatus { server: Option<Upd>,
  loader: Option<Upd>, mc: Option<Upd>, content: count, java: Option<Upd> }` using the providers (cached metadata).
- Publishes `UpdateAvailable` events (dashboard badges, optional webhook). Snoozing an update hides it until a newer one appears.

## Applying (job)
1. Pre-flight: the target exists and is compatible; for an MC change, run the mod compatibility check first.
2. Backup (`pre_update`): full for MC changes; for same-MC builds, `server jar + libraries` scope is enough, but default to
   full for simplicity (the user can choose in settings).
3. Stop if running (with countdown warnings if players are online; optionally schedule for later).
4. Install the new version into the root (keeping the old jar as `.gsd-old/` until the first successful start).
5. Start → wait for Running within a timeout → success: delete `.gsd-old/`; failure: **automatic rollback** (restore the old
   jar/libs and instance.toml; offer full backup restore if the world was touched) and surface diagnostics.
- "Auto-update server builds" toggle per instance (P2): applies same-MC-version updates via the scheduler during a
  maintenance window when the server is empty (Crafty #607).
