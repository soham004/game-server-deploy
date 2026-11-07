# Feature: Monitoring, Status and Notifications

## Metrics
- Sampler task every 5 s per running instance: CPU % and RSS of the process tree (`sysinfo`, including children for
  args-file launches), players online (supervisor/MCSMP), disk usage of the instance (every 5 min, async walk).
- TPS/MSPT (P2): every 30 s, when available: `tick query` (1.20.3+, via RCON to capture the reply), Paper `mspt`, spark
  if installed. Show median, p95 and max (spark guidance: averages hide spikes). Hidden from the console view.
- Storage: in-memory ring of raw samples (1 h); downsampled buckets into SQLite (1 min for 24 h, 15 min for 30 days).
  Pruned by a daily task.
- Host panel: total CPU, RAM, disk free for the data dir and backup dir, with warnings (low disk blocks backups and installs).

## Health / status
- SLP ping every 30 s when Running (latency, MOTD, version, online count) to detect a hung server (process alive but
  not answering). After 3 failed pings, raise a "Server unresponsive" diagnostic, with optional auto-restart (P2, off by default).
- "How to connect" card: `LAN-IP:port`, public IP (from an opt-in lookup service), and a hint about port forwarding.

## Notifications
- Sinks: in-app toasts (always), Discord webhook (embed with colour per severity), generic JSON webhook, ntfy (P2), email (P3).
- Events: server started/stopped/crashed/crash-loop, backup failed/succeeded (optional), update available,
  disk low, player joined/left (opt-in, noisy), schedule failed.
- Config per webhook: event filter + instance filter, "send test" button. Rate limit to avoid spam (collapse repeated crashes).

## Audit log
Every mutating API call writes `(ts, user, instance, action, details)`: console commands, file writes, property changes,
start/stop, installs, deletes. Viewable and filterable in the UI; retained for 90 days (setting).
