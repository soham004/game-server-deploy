# Research 04: How to Talk to a Running Minecraft Server

We have five channels. The design uses them in layers: **stdin/stdout is always there**, and the others are
optional upgrades.

| Channel | Available | Direction | Use for |
|---|---|---|---|
| **stdin / stdout** of the child process | Always (we spawn the process) | Commands in, log lines out | Console, commands, log streaming, state detection ("Done (…)!"). |
| **RCON** (TCP, default 25575) | All versions, opt-in via properties | Command → response text | Commands that need a *captured reply* (`list`, `tick query`). Optional, since stdin plus log parsing covers most of it. |
| **Server Management Protocol (MCSMP)** | **1.21.9+** (snapshot 25w35a) | JSON-RPC 2.0 over WebSocket, plus push notifications | Structured players, allowlist, bans, ops, gamerules, server settings, live events. Best source when available. |
| **Server List Ping (SLP)** | Always (game port) | Status JSON | Health check, MOTD, online count, version, favicon. |
| **Files on disk** | Always | Read and write | `server.properties`, `ops.json`, `whitelist.json`, `banned-*.json`, `usercache.json`, `level.dat`, `stats/`, logs, crash reports. |

## 1. stdin / stdout
- Launch with `nogui` so no Swing window appears.
- Write `command\n` to stdin (no leading `/`).
- Log line format (vanilla, Paper similar): `[HH:MM:SS] [Thread/LEVEL]: message`, for example
  `[12:01:22] [Server thread/INFO]: Done (3.421s)! For help, type "help"`.
  Forge and NeoForge add `[modid/]` or logger names. Fabric uses `[HH:MM:SS] [Server thread/INFO] (Minecraft) msg`.
  The parser must be **tolerant**: a regex with optional parts, and unmatched lines become `level=Unknown` continuation lines
  (stack traces).
- Key patterns (all best-effort; prefer MCSMP when present):
  - Ready: `Done \(([\d.]+)s\)!`
  - Join: `(\w{3,16}) joined the game` · Leave: `(\w{3,16}) left the game`. **(verified)** 26.x logs both as system messages:
    `System chat: E2E_Bot joined the game`.
  - UUID: `UUID of player (\w+) is ([0-9a-f-]{36})`
  - Save done: `Saved the game`, used to sequence backups after `save-all flush`.
  - Port bind failure: `FAILED TO BIND TO PORT` → friendly "port in use" diagnostic.
  - EULA: `You need to agree to the EULA` → EULA dialog.
  - Wrong Java: `UnsupportedClassVersionError` / `class file version 69.0` → "needs Java 25" diagnostic.
- **Windows caveat**: no POSIX signals. A graceful stop is always `stop` on stdin. A forced kill uses a **Job Object** so
  the whole process tree dies (installer and launcher jars can spawn children). On Unix, use a process group and
  `SIGTERM` (the server handles it as a graceful stop), then `SIGKILL`.
- stdout encoding: force `-Dfile.encoding=UTF-8 -Dstdout.encoding=UTF-8` so non-ASCII player names survive on Windows.
- Colour codes: some servers emit ANSI or `§` codes. Convert both to styled spans in the UI.

## 2. RCON
- Enable with `enable-rcon=true`, `rcon.password`, `rcon.port`. **Unencrypted**: bind or firewall it locally and never expose it.
- Packet: `i32 length | i32 request_id | i32 type | payload\0 | \0`, **little-endian**. Types: 3 = login,
  2 = command, 0 = response. Auth failure → `request_id = -1`.
- Responses are fragmented at **4096 bytes** with no end marker. Workaround: after the command, send an empty
  type-0 (or invalid) packet and read until its echo arrives.
- **(verified)** RCON has no address setting: a real 1.20.1 server listens on `::`, every interface, so it can't be bound to
  loopback (D-052). It stays off unless the user turns it on.

## 3. Minecraft Server Management Protocol (MCSMP), 1.21.9+
- JSON-RPC 2.0 over WebSocket at `ws(s)://<management-server-host>:<management-server-port>`.
- Properties: `management-server-enabled`, `-host` (default localhost), `-port` (0 = must set), `-secret`
  (40 alphanumeric characters, generated if blank), `-tls-enabled` (**default true**; the server **fails to start** if TLS
  is on and no keystore is set), `-tls-keystore`, `-tls-keystore-password`, `management-server-allowed-origins`,
  `status-heartbeat-interval`.
- Auth: `Authorization: Bearer <secret>`, or (for browsers) `Sec-WebSocket-Protocol: minecraft-v1,<secret>`.
- **Our plan**: the backend (not the browser) connects. For instances ≥ 1.21.9 we offer "Enable management API",
  which sets host 127.0.0.1, a random free port, a generated secret and `tls-enabled=false` (safe because it is
  loopback-only), so no keystore is needed. The backend connects after "Done", and the browser never sees the secret.
- Discovery: `rpc.discover` returns the schema for the running version. Use it to feature-detect, not version numbers.
- Methods (namespace `minecraft:`): `allowlist[/set|/add|/remove|/clear]`, `bans[...]`, `ip_bans[...]`,
  `players`, `players/kick`, `operators[...]`, `server/status|save|stop|system_message`,
  `serversettings/<name>[/set]` (autosave, difficulty, enforce_allowlist, use_allowlist, max_players,
  pause_when_empty_seconds, player_idle_timeout, allow_flight, motd, spawn_protection_radius, force_game_mode,
  game_mode, view_distance, simulation_distance, accept_transfers, status_heartbeat_interval,
  operator_user_permission_level, hide_online_players, status_replies, entity_broadcast_range),
  `gamerules`, `gamerules/update`.
- Notifications: `server/started|stopping|saving|saved|status|activity`, `players/joined|left`,
  `operators/added|removed`, `allowlist/added|removed`, `bans/…`, `ip_bans/…`, `gamerules/updated`,
  `world/upgrade_started|upgrade_progress|upgrade_finished|upgrade_failed` (API 3.1.0, 26.3 era).
  **(verified)** on real 26.3 servers: `minecraft:notification/<area>/<event>`, API 3.1.0, 89 methods (full list in D-052). Still **take them from `rpc.discover`**.
- **(verified)** parameter shapes on a real 26.3 server, by name (`params: {"add": [...]}`) or position (`[[...]]`): `player` is
  `{id?, name?}`, and a name alone is resolved by the server (offline servers: the offline UUID). `operators/add` takes
  `{player, permissionLevel?, bypassesPlayerLimit}`; `bans/add` `{player, reason?, source?, expires?}`; `ip_bans/add`
  `{ip | player, reason?, source?, expires?}`; `ip_bans/remove` `{ip: [addresses]}`; `players/kick`
  `{player, message: {literal | translatable}}`, answering who it kicked (`[]` if they weren't on); `server/system_message`
  `{message: {literal}, receivingPlayers, overlay}`. `expires` is ISO 8601 (`2026-11-09T18:01:34Z`): the files' format is
  refused. Each change is saved to its JSON file before the answer comes.
- **(verified)** names are resolved by the server: online players, then `usercache.json` (any case), then Mojang; a name found
  nowhere is lowercased first, so in offline mode it gets the lowercase name's UUID. Console commands do the same. An `id` next to
  a name is ignored, and a disagreeing one makes the call do nothing (`[]`).
- The data generator's `reports/json-rpc-api-schema.json` has the same schema without a running server.
- A Rust client exists (`mcsmp-rs`). Evaluate it, but we may write our own thin client (it is just JSON-RPC over `tokio-tungstenite`).
- Older servers: a "legacy support" plugin exists on Modrinth for Bukkit-based servers. We just fall back to stdin and files.

## 4. Server List Ping (SLP)
- TCP to the game port: Handshake (protocol −1 or current, next state 1) → Status Request → JSON
  `{version{name,protocol}, players{max,online,sample[]}, description, favicon}` → Ping/Pong for latency.
- Uses VarInt framing. Small, easy to implement ourselves (~150 lines), or use a crate.
- `enable-status=false` or `hide-online-players=true` limits it. Treat that as "unknown", not "down".

## 5. Files we read and write
| File | Format | Notes |
|---|---|---|
| `server.properties` | Java properties (`key=value`, `#` comments, `\` escapes, unicode escapes) | **Round-trip edits**: keep order, comments and unknown keys. |
| `eula.txt` | properties | `eula=true` only after explicit user consent. |
| `ops.json` | `[{uuid,name,level,bypassesPlayerLimit}]` | Edit only while stopped; use commands or MCSMP while running (the server overwrites the file). |
| `whitelist.json` | `[{uuid,name}]` | Same rule. |
| `banned-players.json` / `banned-ips.json` | `[{uuid?,name?,ip?,created,source,expires,reason}]` | Same rule. |
| `usercache.json` | `[{name,uuid,expiresOn}]` | Name ↔ UUID cache. |
| `<world>/level.dat` | gzipped NBT | Seed, version, game type, spawn; game rules before 1.21.11 (`Data.GameRules`, strings). Read and written by our own NBT module (D-054). |
| `<world>/data/minecraft/game_rules.dat` | gzipped NBT | **(verified)** game rules since 1.21.11: `data` compound, `minecraft:<rule>` keys, a byte for true/false, an int for numbers. Written at a save. |
| `<world>/stats/<uuid>.json` | JSON | `minecraft:custom` → `minecraft:play_time` (ticks), deaths, and so on. |
| `logs/latest.log`, `logs/YYYY-MM-DD-N.log.gz` | text, gzip | History viewer. |
| `crash-reports/crash-*.txt` | text | Crash viewer. |
| `config/`, `*.yml` (Paper: `config/paper-global.yml`, `paper-world-defaults.yml`, `bukkit.yml`, `spigot.yml`) | yaml, toml, json5 | Raw editor first; structured editors later. |

**Rule: while the server runs, mutate state through the server (stdin command or MCSMP), not by writing JSON files.**
The server holds them in memory and will overwrite our edits.

## 6. Command metadata for autocomplete
- The vanilla data generator dumps the full **Brigadier command tree**:
  `java -DbundlerMainClass=net.minecraft.data.Main -jar server.jar --reports --output <tmp>` (1.18+; pre-21w39a used
  `-cp server.jar net.minecraft.data.Main`). The output is `reports/commands.json` (plus `registries.json`, `blocks.json`,
  `items.json`, …).
- `commands.json` node shape: `{ type: "root"|"literal"|"argument", children: {name: node}, executable?, redirect?: [path],
  parser?: "brigadier:integer"|"minecraft:entity"|…, properties?: {...} }`.
- It covers vanilla only. For mods and plugins, `help` output gives usage lines for every registered Brigadier
  command (`/<cmd> <arg> [opt] (a|b)`), which we parse into a shallower tree. Details are in
  `plan/features/command-autocomplete.md`.
- Pre-built per-version data also exists in `misode/mcmeta` (GitHub, branch `summary`/`commands`). Generating
  locally is preferred (offline, exact version), with mcmeta as a fallback.

## 7. Performance metrics
- `tick query` (vanilla 1.20.3+): average and percentile tick times. Run it via RCON (the reply is needed) or parse it from stdout.
- Paper: `mspt`, `tps`. spark (if installed): `spark tps`, `spark health`.
- Process metrics (CPU %, RSS) come from the OS via the `sysinfo` crate; the server itself is not involved.

## 8. server.properties reference (Java Edition, current)
The key table (type, default, range, description, version introduced) was captured from minecraft.wiki and is
reproduced in `plan/features/server-properties.md` as the **schema** for the visual editor. Notable recent keys:
`management-server-*` (1.21.9), `status-heartbeat-interval`, `enable-code-of-conduct` (25w34a),
`chat-spam-threshold-seconds` and `command-spam-threshold-seconds` (26.2), `pause-when-empty-seconds` (1.21.2),
`region-file-compression` (1.20.5). Upcoming in 26.4: `allowed-connection-ids`, `enable-legacy-status`,
`status-contact-details`.

## Sources
- [wiki.vg RCON](https://wiki.vg/Rcon) · [RCON spec (MSM docs)](https://minecraft-server-manager.readthedocs.io/en/latest/advanced/rcon/)
- [Minecraft Server Management Protocol (wiki)](https://minecraft.wiki/w/Minecraft_Server_Management_Protocol) · [25w35a snapshot](https://www.minecraft.net/en-us/article/minecraft-snapshot-25w35a) · [mcsmp-rs](https://docs.rs/mcsmp-rs/latest/mcsmp_rs/) · [Legacy support plugin](https://modrinth.com/project/38WxG5ix)
- [server.properties (wiki)](https://minecraft.wiki/w/Server.properties) · [server.properties reference (GameServerKings)](https://www.gameserverkings.com/knowledge-base/minecraft/server-properties-reference/)
- [Running the data generator (wiki)](https://minecraft.gamepedia.com/Tutorials/Running_the_Data_Generator) · [Data generators (wiki.vg)](https://wikivg.booky.dev/Data_Generators) · [misode/mcmeta](https://github.com/misode/mcmeta)
