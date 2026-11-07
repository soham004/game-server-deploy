# Feature: Players, Gamerules and Worlds

## Data sources by priority
1. **MCSMP** (1.21.9+, running): players, allowlist, ops, bans, ip_bans, gamerules, serversettings, with live notifications.
2. **Commands + log parsing** (running, any version): `list`, `op`, `deop`, `whitelist add/remove/on/off/reload`,
   `ban`, `ban-ip`, `pardon`, `pardon-ip`, `kick`, `gamerule <name> [value]`; join/leave from log lines.
3. **JSON files** (stopped): `ops.json`, `whitelist.json`, `banned-players.json`, `banned-ips.json`, `usercache.json`.
   Written directly only while stopped.
The `PlayerService` hides this behind one API: `list()`, `apply(action)`, choosing the channel by state and capability.

## Player directory
- Online now (with a session timer), known players (`usercache.json` + `player_sessions` table), ops (level), whitelist, bans (reason, source, expiry).
- Add by name: Mojang API lookup (cached) → UUID; offline-mode servers use the offline UUID instead (detected from `online-mode=false`).
- Heads: rendered server-side from the skin texture and cached (`/players/{uuid}/head.png`), with a Steve/Alex fallback offline.
- Player drawer: head, name, UUID (copy), first/last seen, playtime (sum of sessions, or `stats/<uuid>.json`
  `minecraft:play_time`), actions (message, kick, ban with reason/duration*, op level, whitelist toggle, teleport-to-spawn P2).
  \* Vanilla bans have no duration. Temp bans are an app feature: we schedule the `pardon`.

## Gamerules
- Running: MCSMP `gamerules` (types and values), else `gamerule` commands. The list comes from the commands tree
  (gamerule literals) and values are queried one by one. Stopped: read-only from `level.dat` (`Data.GameRules`) via fastnbt
  (editing level.dat offline is P2 and risky; prefer "apply on next start" queued commands).
- UI: grouped (Player, Mobs, World updates, Chat, Drops, Misc), toggles and number inputs, search, "changed from default" markers.

## Worlds
- List worlds: folders containing `level.dat`. Show name, size (async), last played, MC version (`Data.Version.Name`),
  seed (`WorldGenSettings.seed`), game type, difficulty, and whether it is active (`level-name`).
- Actions: set active (`level-name`), download as zip, upload a zip (validate that it contains `level.dat`, detect a nested folder),
  reset/regenerate (backup first; optionally a new seed), delete (backup first), datapacks list (P2).
- Map preview (P3): link to BlueMap/Dynmap/squaremap when detected (plugin/mod present + port from config).
