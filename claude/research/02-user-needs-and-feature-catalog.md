# Research 02: User Needs and Full Feature Catalog

> Built from: the user's brief, Prism's feature set, competitor panels, Crafty, AMP and Pelican issue trackers,
> and general Minecraft server-admin practice. Reddit could not be fetched directly (blocked), so community
> wishes come from issue trackers and comparison articles instead.
>
> Priority: **P0** = MVP must have · **P1** = v1.0 · **P2** = after v1 · **P3** = maybe/never.

## A. Explicitly requested by the user
| # | Feature | Priority | Plan doc |
|---|---|---|---|
| A1 | Multiple instances, choose MC version + loader + loader version | P0 | `plan/features/instances-and-installers.md` |
| A2 | Modrinth + CurseForge mod browser/installer | P0 (Modrinth) / P1 (CurseForge) | `plan/features/mods-and-modpacks.md` |
| A3 | Live server console with command input | P0 | `plan/features/console-and-process.md` |
| A4 | Minecraft command autocomplete | P1 | `plan/features/command-autocomplete.md` |
| A5 | Log viewer | P0 (live) / P1 (history) | `plan/features/logs.md` |
| A6 | Scheduler (cron + MC commands) | P1 | `plan/features/scheduler.md` |
| A7 | Manual + scheduled backups | P0 manual / P1 scheduled | `plan/features/backups.md` |
| A8 | Browse and edit server files in the browser | P0 | `plan/features/file-manager.md` |
| A9 | Visual `server.properties` editor | P0 | `plan/features/server-properties.md` |
| A10 | Update checker for the server jar | P1 | `plan/features/updates.md` |

## B. Additional features users need for "proper" administration (discovered)
### Lifecycle and reliability
| # | Feature | Why | Priority |
|---|---|---|---|
| B1 | **EULA acceptance flow** | The server will not start without `eula=true`. Legally the *user* must accept the Mojang EULA, so show it and require a checkbox. Never auto-accept silently. | P0 |
| B2 | Start / stop / restart / kill with a **graceful stop** (`stop`, then timeout, then kill) | Prevents world corruption. | P0 |
| B3 | **Auto-restart on crash** with crash-loop protection | Top hobby-panel feature. | P0 |
| B4 | **Crash report viewer** (`crash-reports/*.txt`) with a "likely culprit" hint (mod id in the stack trace) | Modded servers crash often. | P1 |
| B5 | Auto-start instances when the app starts (and run the app as an OS service) | Home servers reboot. | P1 |
| B6 | **Java runtime management**: detect installed JDKs, auto-download the right Temurin version (MC 26.x needs Java 25; 1.20.5–1.21.x needs 21; 1.17–1.20.4 needs 17; older needs 8) | The most common "server won't start" cause (`UnsupportedClassVersionError`). | P0 |
| B7 | Memory (Xms/Xmx) slider with a sensible default, plus JVM flag presets (Aikar's G1 flags, generational ZGC for big heaps) | Everyone asks "how much RAM?" | P0 |
| B8 | Restart **countdown warnings** to players ("Restarting in 5 min…") | Pain point in AMP and Crafty. | P1 |

### Players and world
| # | Feature | Why | Priority |
|---|---|---|---|
| B9 | **Player manager**: online list with heads, kick, ban, op, whitelist, pardon; add players who have never joined by name → UUID lookup | Most used admin action after the console. | P1 |
| B10 | Player history: join and leave times, playtime (from `world/stats/<uuid>.json`), last seen | Very common request. | P2 |
| B11 | **Gamerule editor** (toggles and number inputs) | Crafty issue #585 ("like Aternos"). | P1 |
| B12 | World manager: list worlds, seed and version (from `level.dat`), upload or download a world zip, reset or regenerate, switch `level-name` | Common. | P1 |
| B13 | Datapack manager (`world/datapacks`) with Modrinth datapack search | Vanilla servers rely on datapacks. | P2 |
| B14 | Chunk pre-generation via Chunky integration (install the mod or plugin and drive it) | Best practice to avoid lag. | P2 |
| B15 | Web map integration (BlueMap, Dynmap, squaremap): detect the plugin and show a link or embed | Nice to have. | P3 |

### Mods, plugins, packs
| # | Feature | Why | Priority |
|---|---|---|---|
| B16 | **Server-side compatibility filter**: hide client-only mods (Modrinth `server_side == unsupported`) | The most common modded-server mistake is installing client-only mods (shaders, minimaps). | P0 |
| B17 | Dependency resolution (auto-add required dependencies, warn on incompatibilities) | Prism does this. | P1 |
| B18 | Identify existing unknown jars (Modrinth SHA-1 / CurseForge fingerprint lookup) so imported servers get update checks | Needed for imported servers. | P1 |
| B19 | Modpack install: `.mrpack` (server env filtering + `server-overrides/`), CurseForge server packs | Huge for modded users. | P1 |
| B20 | Plugin support for Paper/Purpur (Modrinth plugins + Hangar) | Plugin servers are the majority of public servers. | P1 |
| B21 | Enable/disable a mod without deleting it (`.jar.disabled`) | Debugging crashes. | P0 |
| B22 | "Bisect" helper to find which mod breaks startup (disable half, test, repeat) | A killer feature for modded admins. | P3 |

### Observability
| # | Feature | Why | Priority |
|---|---|---|---|
| B23 | Resource graphs: CPU, RAM (RSS), disk usage of the instance, player count | Universal. | P1 |
| B24 | **TPS / MSPT**: vanilla `/tick query` (1.20.3+), Paper `/mspt`, spark if installed; show median, p95 and max, not just an average | Spark guidance says averages hide spikes. | P2 |
| B25 | Server List Ping status (MOTD, player count, version) as an external health check | Cheap and reliable. | P0 |
| B26 | Notifications: Discord webhook and in-app toasts on crash, start, stop, backup failure, update available | Pelican and Crafty both have webhooks. | P1 |
| B27 | Audit log: who did what (commands, file edits, config changes) | Needed once multi-user exists. | P1 |
| B28 | Upload a log to mclo.gs for sharing (Prism does this) | Support requests. | P2 |

### Networking and access
| # | Feature | Why | Priority |
|---|---|---|---|
| B29 | Show "how to connect": LAN IP, public IP, port; port-conflict detection between instances | Beginners struggle here. | P0 (basic) |
| B30 | Optional UPnP port forwarding (`igd` crate) and a playit.gg tunnel guide | Home hosting. | P2 |
| B31 | Login + sessions; multiple users with global roles + per-instance grants; remote access via nginx | Security; **user decided RBAC is in v1** (2026-11-07). | P0 (auth + RBAC model) / P1 (user management UI) |
| B31b | **Per-server isolation** (UID, Landlock, limits) so one compromised server can't affect others | User requirement 2026-11-07. | P0 (Linux) |
| B32 | Passkeys and 2FA | Pelican has them. | P3 |

### Operations
| # | Feature | Why | Priority |
|---|---|---|---|
| B33 | **Background job tray**: installs, backups, downloads, with progress and cancel | Long operations need visibility. | P0 |
| B34 | Clone an instance, export or import an instance zip, import an existing server folder | Crafty imports existing servers; Prism clones. | P1 |
| B35 | Remote backup destinations (S3, SFTP, rclone) with upload *after* restart to avoid downtime | Pelican and AMP; AMP downtime complaint. | P2 |
| B36 | Backup only if players were online since the last backup | Missing in AMP. | P1 |
| B37 | Scheduled server and mod update with backup and rollback | Crafty #607. | P2 |
| B38 | Proxy support (Velocity) and network view | Larger setups. | P3 |
| B39 | Bedrock (Geyser) | Later. | P3 |
| B40 | Self-update of the app | Pelican lists it; nobody has it. | P2 |
| B41 | Dark and light theme, keyboard shortcuts, mobile-friendly layout | UX goal. | P0 (structure) |

## C. UX principles distilled from complaints
1. **Never lose a world.** Destructive operations (version change, restore, world reset, delete instance) take an
   automatic safety backup and need a typed confirmation.
2. **Explain errors in plain language.** Map known failure patterns (wrong Java, port in use, EULA not accepted,
   missing dependency mod, client-only mod on the server, out of memory) to friendly diagnostics with a fix button.
3. **Minecraft-aware, not generic.** Show players, worlds and gamerules as first-class objects, not just files.
4. **Fast and light.** No multi-megabyte SPA. The UI should feel instant on a Raspberry Pi.
5. **Progress everywhere.** Every long action shows progress, logs and a cancel button.
6. **Sensible defaults, full control.** Beginners get presets; experts can always reach the raw file or flag.

## Sources
See `01-competitive-landscape.md`, plus:
- [Spark profiler guide](https://space-node.net/blog/spark-profiler-minecraft-server-guide-2026) · [Lag troubleshooting with spark](https://teramont.net/blog/minecraft-server-lag-troubleshooting-spark)
- [Modrinth `.mrpack` format](https://docs.modrinth.com/modpacks/format) · [Modpack server readiness tool](https://tools.supercraft.host/tools/minecraft-modpack-server-readiness)
- [itzg docker-minecraft-server CurseForge restricted mods issue](https://github.com/itzg/docker-minecraft-server/issues/2067)
- [Java version table (Wepwawet)](https://docs.wepwawet.net/games/minecraft/setup/java-version) · [2026 numbering change (GameServerKings)](https://www.gameserverkings.com/knowledge-base/minecraft/updating-your-server-version/)
