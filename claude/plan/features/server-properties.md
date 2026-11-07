# Feature: Visual `server.properties` Editor

## Round-trip parser (gsd-minecraft/properties/parser.rs)
- Parse into a `Vec<Line>` where `Line = Comment(String) | Blank | Entry { key, value, raw_key, raw_sep }`.
- Support the Java properties spec: `=`, `:` or whitespace separators; `\` line continuation; `\uXXXX`, `\t`, `\n`, `\\`
  escapes; leading whitespace; `!` comments. Files are read as UTF-8 (modern servers write UTF-8) with an ISO-8859-1 fallback.
- **Edit** = replace the value in place (escape on write), **add** = append at the end, **remove** = drop the line.
  Order, comments and unknown keys (plugins sometimes add some) are preserved. Golden round-trip tests: `parse(x).render() == x`.

## Schema (gsd-minecraft/properties/schema.rs, data in `schema/server_properties.json`)
Each key: `{ key, type: bool|int|string|enum|uuid|url|password, default, min?, max?, options?, category, label,
description, since?, until?, restart_required: true, advanced: bool, sensitive: bool }`.
The editor shows only keys valid for the instance's MC version (`since`/`until`). Unknown keys appear under "Other".

### Categories and keys (current Java Edition, from minecraft.wiki 2026-11)
| Category | Keys |
|---|---|
| **General** | `motd` (with live MOTD preview incl. § colours), `max-players`, `difficulty` (peaceful/easy/normal/hard), `gamemode`, `force-gamemode`, `hardcore`, `pvp`* , `allow-flight`, `spawn-protection`, `player-idle-timeout`, `pause-when-empty-seconds` (1.21.2+) |
| **World** | `level-name`, `level-seed`, `level-type` (minecraft:normal/flat/large_biomes/amplified/single_biome_surface), `generator-settings`, `generate-structures`, `max-world-size`, `initial-enabled-packs`, `initial-disabled-packs`, `region-file-compression` (deflate/lz4/none) |
| **Players & access** | `white-list`, `enforce-whitelist`, `online-mode` (big warning when false), `enforce-secure-profile`, `prevent-proxy-connections`, `op-permission-level`, `function-permission-level`, `hide-online-players`, `log-ips`, `enable-code-of-conduct` (25w34a+), `accepts-transfers` |
| **Performance** | `view-distance`, `simulation-distance`, `entity-broadcast-range-percentage`, `network-compression-threshold`, `max-tick-time`, `max-chained-neighbor-updates`, `sync-chunk-writes`, `use-native-transport`, `rate-limit`, `chat-spam-threshold-seconds` / `command-spam-threshold-seconds` (26.2+) |
| **Network** | `server-ip`, `server-port` (port-conflict check across instances), `enable-status`, `enable-query`, `query.port` |
| **Resource pack** | `resource-pack` (URL), `resource-pack-sha1` (button: "compute from URL"), `resource-pack-id`, `resource-pack-prompt`, `require-resource-pack` |
| **Remote control** (advanced) | `enable-rcon`, `rcon.port`, `rcon.password` (sensitive), `broadcast-rcon-to-ops`, `broadcast-console-to-ops`, `management-server-*` (1.21.9+), `status-heartbeat-interval`, `enable-jmx-monitoring` |
| **Misc / internal** | `bug-report-link`, `text-filtering-config`, `text-filtering-version`, upcoming 26.4: `allowed-connection-ids`, `enable-legacy-status`, `status-contact-details` |

\* `pvp` was moved to a gamerule in recent versions. Verify per version and hide the property where it doesn't exist.
Note: the wiki fetch reported `white-list` default `true`. Verify against a freshly generated 26.3 file; historically it was `false`.
**Rule: generate defaults by running each new MC version once and diffing, rather than trusting docs.**

## UI
- Category sidebar + search box (searches labels, keys and descriptions).
- Typed controls: toggles, number steppers with min/max, selects, text with validation, password fields with
  reveal and generate, a seed field with a "random" button.
- Each field shows: label, the real key in monospace, help text, a "default" badge, and a "reset to default" action.
- Changed fields are highlighted; a sticky footer shows "N changes · Discard · Save" (and "Save & restart" if running).
- Raw mode tab: CodeMirror with properties highlighting; both views edit the same model.
- Some keys are managed by the app (`rcon.*`, `management-server-*` when the integration is on) and shown as read-only with an explanation.
- Related editors linked from here: Gamerules (via MCSMP or commands, running only), Whitelist/Ops (Players tab).
