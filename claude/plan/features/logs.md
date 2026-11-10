# Feature: Log Viewer and Crash Reports

Built (D-051): listing, reading with search and level, tail-first paging, crash summaries, the Logs tab and the overview's last-crash
card. Below is the original plan; not built from it: time windows, per-run stdout capture, mapping packages to mods, next/previous match,
error context lines, and sharing to mclo.gs.

## Sources
- **Live**: supervisor ring buffer + WS (console tab).
- **History**: `server/logs/latest.log`, `server/logs/YYYY-MM-DD-N.log.gz`, `debug.log` (Forge/NeoForge/Fabric),
  plus our own **per-run stdout capture** for servers that don't log to file (rare).
- **Crash reports**: `server/crash-reports/crash-<date>-server.txt`; JVM `hs_err_pid*.log` in the server root.

## Backend
- `GET /logs` lists files with size, mtime and kind (latest, archive, debug, crash, jvm_crash).
- `GET /logs/{name}` streams a decompressed file (gz via `flate2`, streamed, never fully in memory),
  server-side filters `q` (substring or regex with timeout), `level`, time window, paged by line offset with `next_cursor`.
- Lines are parsed with the same `log_parser` as live console lines, so the UI renders both identically.
- Large-file safety: cap the response size and stream chunks; regex has a complexity limit (`regex` crate is linear-time).
- **Crash analysis**: parse `Description:`, the exception, and the first frames; map package names to mod ids
  (via the mods index: jar → package prefixes from `fabric.mod.json`/`mods.toml`) to show "Likely caused by: <mod>".

## UI
- File list on the left (grouped by date), viewer on the right reusing the console's virtualised renderer (read-only).
- Toolbar: search (with next/prev match), level filter, "only errors + context ±5 lines", wrap toggle, download,
  **Share** (upload to mclo.gs, behind an explicit confirmation since logs can contain IPs).
- Crash reports get a summary card on top (time, description, suspected mod, "open full report").
- Overview tab shows the "last crash" card if a crash happened since the last successful start.
