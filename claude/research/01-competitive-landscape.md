# Research 01: Competitive Landscape

> Researched 2026-11-07. Sources are listed at the bottom. Third-party blog figures (RAM usage, market share)
> are indicative only.

## 1. Prism Launcher (client-side inspiration)

Prism Launcher is a free, open-source (GPL-3) Qt launcher forked from MultiMC. It is the UX model the user wants
us to copy, applied to **servers** instead of clients.

### What it provides
| Area | Prism behaviour | What we take from it |
|---|---|---|
| Instances | Many isolated instances, each in its own folder with its own mods, resource packs, worlds and settings. Instances can be grouped, renamed, copied, given icons and notes. | Same: one self-contained folder per server instance; groups, icons, notes, clone. |
| Version picker | "Custom" dialog (see the user's screenshot): the **top list** is Minecraft versions filtered by Releases / Snapshots / Betas / Alphas / Experiments, with search and refresh. The **bottom list** is the loader version for the chosen MC version, with a radio group for the loader (None, NeoForge, Forge, Fabric, Quilt, LiteLoader). A ⭐ marks recommended builds. | Copy the two-pane picker exactly, with server loaders: Vanilla, Paper, Purpur, Fabric, Quilt, Forge, NeoForge. ⭐ = recommended/stable. |
| Components ("Version" tab) | An instance is a stack of components (Minecraft, loader, mappings). You can change MC or loader version later via right-click → Change version. | "Change version" action on an existing instance, with a mandatory backup + world-upgrade warning. |
| Mods tab | List of installed mods with enable/disable checkbox (renames to `.disabled`), "Download mods" browser (Modrinth + CurseForge), "Check for updates", view on website, open folder. | Same, plus a server-side compatibility badge. |
| Mod metadata | Stores per-mod metadata in **packwiz-style `.pw.toml` files** under `mods/.index/`. That is how it knows where each jar came from and how it checks for updates. | Adopt the same packwiz index format, which keeps instances portable and interoperable. |
| Modpacks | Install from Modrinth (`.mrpack`), CurseForge (zip + `manifest.json`), ATLauncher, FTB, Technic. Tracks pack version and can update. | Support `.mrpack` and CurseForge packs and server packs first. |
| Export | Exports instances as Prism zip, `.mrpack` or CurseForge zip. | Export instance as a portable zip or `.mrpack`. |
| CurseForge restriction | When a mod author blocks third-party distribution, Prism shows a dialog listing the blocked files with links, then watches the Downloads folder or accepts drag-and-drop and matches files by fingerprint. | Same "manual download" flow with drag-and-drop matching by CurseForge fingerprint (murmur2). |
| Java | Auto-detects installed Javas, can auto-download the right Java (Mojang's runtime or Adoptium), and sets per-instance memory and JVM args. | Auto-detect plus auto-download Temurin per required major version, with per-instance memory and JVM flags. |
| Other tabs | Worlds, Resource packs, Shader packs, Screenshots, Notes, Server list, Logs ("Minecraft Log" with search, pause, copy, upload to paste site), Other logs. | Worlds, Logs (search, filter, upload to mclo.gs), Notes. |
| Look and feel | Lightweight, themeable, no embedded browser. | Ours is a web UI, but it must stay fast and light: no heavy SPA framework. |

## 2. Server panels (direct competitors)

| Panel | Stack | Strengths | Weaknesses / complaints |
|---|---|---|---|
| **Pterodactyl / Pelican** | PHP/Laravel panel + Go "Wings" daemon. Docker per server. | Strong isolation, multi-node, SFTP, S3 backups, granular permissions, REST API, big ecosystem; "eggs" (JSON templates) for Paper, Fabric, Forge, NeoForge. Pelican adds themes, webhooks, plugins, passkeys and a web installer. | Hard install (Docker, Linux, DB, Redis), 1.5–2 GB idle RAM, built for hosting companies. Installing mods or plugins is manual (upload or SFTP) without add-ons. Generic game UI, not Minecraft-aware. |
| **Crafty Controller 4** | Python (Tornado), single process. | Easy install (Docker or portable Windows), native Java processes, scheduler (backups, restarts, commands), file manager, CurseForge plugin search, imports existing servers, Forge installer wizard, roles, webhooks, themes. | Single machine only, no isolation between servers, no S3. Users ask for a world and gamerule editor ("like Aternos"), backup progress in chat, scheduled auto-update with rollback, and cloud backups. |
| **MCSManager 10** | Node.js web + daemon. | Light (~100–200 MB), multi-node without Docker, good file manager (drag-and-drop, zip extract, in-browser YAML/JSON editor), web terminal, per-instance scheduled backups. | Thin English docs, basic RBAC, no S3, upload-centric plugin handling. |
| **AMP (CubeCoders)** | Commercial (.NET). | Many games, scheduler with "triggers" and event hooks, S3 upload. | Paid. Users report a confusing scheduler UI (2019 issue still open asking to sort, group, enable and disable triggers), no "only back up if players were online" condition, retention edge cases, long downtime while backups upload to S3. |
| **PufferPanel** | Go. | Simple, open source. | No scheduled tasks and no remote backups (per Pelican's comparison table). |
| **Hobby tools** (McServerman, mcmngr, IntegratedMinecraftServer, MCSHoster) | Various. | Player management pages (whitelist, op/deop, bans by UUID even for never-joined players), auto-restart on crash, Windows service mode, MOTD editor. | Mostly unmaintained or partial. |
| **Aternos / hosted UIs** | SaaS. | The friendliest UX: world settings, gamerules and properties shown as toggles; one-click mod and plugin installs. | Not self-hosted. Users explicitly want this level of polish in a self-hosted tool. |

### Pelican's comparison matrix (useful checklist)
Free/OSS · plugins/marketplace · themes · roles & permissions · webhooks · passkeys · light/dark mode ·
web installer · captcha login · remote backups · database management · scheduled tasks · Docker install ·
database choice · announcements · self update.

## 3. Our positioning (the gap)

No existing panel combines all of the following:
1. **Prism-quality instance and version UX for servers**: a two-pane MC version × loader picker, a mod browser
   that understands loader, MC version and server-side compatibility, and update checks for mods and server jars.
2. **Easy install for a single machine** (one binary, no Docker, no database server) like Crafty, but **in Rust**
   with a tiny footprint.
3. **Minecraft-aware administration** rather than a generic game panel: a visual `server.properties` editor,
   gamerules, a player manager, command autocomplete, the Server Management Protocol (MCSMP), and world tools.
4. **A scheduler that is pleasant to use**, with conditions like "only if players were online" and restart
   countdown warnings (both pain points in AMP and Crafty).
5. **Safe-by-default operations**: automatic pre-update and pre-restore backups, rollback, and a clear EULA flow.

Out of scope for v1 (but keep the architecture open to them): multi-node, Docker isolation, billing,
non-Minecraft games.

## Sources
- [Prism Launcher](https://prismlauncher.org/) · [About](https://prismlauncher.org/about) · [Instance management (dev docs mirror)](https://www.mintlify.com/PrismLauncher/PrismLauncher/development/architecture/instance-management) · [GDLauncher vs Prism](https://gdlauncher.com/vs/prismlauncher) · [Fabric wiki: Prism](https://wiki.fabricmc.net/_export/raw/player:tutorials:third-party:prism)
- [Pterodactyl vs Crafty vs MCSManager 2026 (MineGuard)](https://mineguard.pro/en/blog/pterodactyl-vs-crafty-vs-mcsmanager-2026)
- [Pelican comparison table](https://pelican.dev/docs/comparison) · [Pelican eggs](https://github.com/M4RC-XX/pelican-eggs)
- [Crafty issue #607 (scheduled updates)](https://gitlab.com/crafty-controller/crafty-4/-/issues/607) · [#193 (backup chat messages)](https://gitlab.com/crafty-controller/crafty-4/-/issues/193) · [#585 (world editor like Aternos)](https://gitlab.com/crafty-controller/crafty-4/-/issues/585) · [Crafty guide](https://space-node.net/blog/crafty-controller-minecraft-panel-guide-2026)
- [AMP scheduler overhaul issue](https://github.com/CubeCoders/AMP/issues/181) · [AMP conditional triggers](https://discourse.cubecoders.com/t/conditional-triggers-and-multiple-backup-branches-minecraft/291) · [AMP S3 downtime](https://discourse.cubecoders.com/t/how-to-schedule-upload-to-s3-after-the-game-server-is-started-again/27411)
- [McServerman](https://github.com/tycrek/McServerman) · [mcmngr](https://github.com/hiarthurbr/mcmngr) · [IntegratedMinecraftServer](https://github.com/DouglasDwyer/IntegratedMinecraftServer) · [MCSHoster](https://github.com/DaDevMikey/MCSHoster/releases/tag/beta1)
