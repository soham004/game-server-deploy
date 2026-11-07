# Feature: Backups and Restore

## Scopes
- **World**: the `level-name` folder(s) (plus `world_nether` and `world_the_end` for Bukkit-style layouts).
- **Full**: the entire `server/` except excluded globs (default excludes: `logs/`, `crash-reports/`, `cache/`,
  `libraries/`, `versions/`, `.fabric/`, `*.tmp`, BlueMap/Dynmap tile folders (detected)).
- **Custom**: user-chosen include and exclude globs (`globset`).

## Consistent snapshot while running (quiesce)
1. `save-off` (stop autosave) → `save-all flush` → wait for the `Saved the game` log line (or MCSMP `server/saved`), with a timeout.
2. Archive (in `spawn_blocking`), streaming files → `zip` (default, user-friendly) or `tar.zst` (faster and smaller; setting).
3. **Always** send `save-on` afterwards, even on failure (a guard pattern).
4. Optional in-game messages "Backup started/finished" (opt-in; Crafty #193).
If the server is stopped, just archive.

## Storage, metadata, retention
- Path: `<data>/backups/<slug>/<YYYYMMDD-HHMMSS>-<kind>.<ext>`. The backup dir is configurable (another disk).
- Each archive embeds a `gsd-backup.json` (instance id, MC version, loader, scope, created_at) so it is self-describing.
- DB row with size and sha256 (computed while streaming).
- Retention policy per instance: keep last N (default 10) **and/or** GFS (keep 24 hourly, 7 daily, 4 weekly, 6 monthly),
  plus a max total size. **Pinned** and `pre_*` safety backups are excluded from the count (they have their own limit and age).
  Retention runs after each backup and logs what it deleted.
- Disk-space pre-check: estimate from the last backup size or the folder size; refuse if free space is under the estimate × 1.2.

## Restore
1. Confirm dialog (shows the backup's MC version vs the current one; warns on a mismatch).
2. Stop the server (graceful).
3. Automatic `pre_restore` backup of the current state (can be unticked with a warning).
4. Extract to a temp dir inside the same filesystem → verify → swap the folders (rename) → delete the old ones.
   Never extract over live files directly.
5. World-only restores replace only the world folders. Optionally restore a single dimension.
6. Start the server if it was running before.

## Later (P2)
- Remote targets: S3-compatible (`aws-sdk-s3` or `rust-s3`), SFTP, a local folder (NAS mount), rclone passthrough.
  Upload **after** the server resumes (AMP downtime complaint), as a separate job step.
- Incremental/deduplicated backups (content-defined chunking, restic-style) for big worlds.
- Download a backup from the UI and upload a backup zip as a new instance.
