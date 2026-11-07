# Plan 00: Product Vision and Scope

## One-liner
A self-hosted, single-binary **game server manager** with a Rust backend and a fast, polished web UI. It starts with
Minecraft: Java Edition and aims to be **"Prism Launcher for servers"**.

Working name: `game-server-deploy` (crate prefix `gsd`). The final name is an open question.

## Who it is for
1. **Home and hobby admins** running 1–5 servers for friends on a PC, mini-PC or Raspberry Pi. They want it to
   just work, explained in plain language.
2. **Modded server admins** who juggle loaders, versions, mod compatibility and crashes. They want Prism's mod tooling for servers.
3. **Small community admins** with a VPS who need scheduling, backups, players and remote access.

Multiple users with roles **are** in v1: an owner can give friends or moderators scoped access per instance.
Not for (v1): hosting companies (billing, multi-node). The architecture keeps seams for remote "agents" and per-server containers later.

## How it runs (decided 2026-11-07)
- **Linux first, shipped as a Docker image** (amd64 + arm64): `docker compose up` with a `/data` volume, plus an optional
  **nginx** container for HTTPS. Bare-metal Linux runs as a systemd service. Windows and macOS work for development and simple home use.
- **Browser-only** UI. No desktop wrapper.
- One executable, no external database, no Node.js. Data lives in one data directory (SQLite plus instance folders).
- Minecraft servers run as **native child processes inside the same container**, each **isolated**: its own UID, a Landlock
  filesystem and network sandbox, and resource limits. A compromised server can't touch the others
  (`plan/features/isolation-and-sandboxing.md`).
- The product name, slug and default port are **build-time settings** declared in one place (`plan/07-deployment-and-branding.md`).

## Experience pillars
1. **Create a server in under a minute**: a two-pane version × loader picker (Prism style), memory slider, EULA
   checkbox, then Create. Java is auto-provisioned.
2. **Everything Minecraft-aware**: players, worlds, gamerules and properties are first-class UI, not just files.
3. **Safe by default**: automatic backups before risky operations, graceful stops, rollback on failed updates.
4. **Explain, don't dump**: known errors map to human diagnostics with a one-click fix.
5. **Fast and light**: plain HTML, CSS and JS, no framework, instant navigation, works on low-end hardware.
6. **Full control for experts**: raw file editor, raw JVM args, raw properties view, console.

## Feature scope by release (summary; see `06-roadmap.md`)
- **MVP (M1–M3)**: instances (Vanilla, Paper, Fabric, then Forge, NeoForge, Quilt, Purpur), Java management, console,
  live logs, file manager, properties editor, manual and scheduled backups, scheduler, EULA, auto-restart.
- **v1.0 (M4–M6)**: Modrinth and CurseForge mods, plugins and modpacks, update checker, player manager, gamerules,
  MCSMP, command autocomplete, log history, metrics, notifications, auth hardening, OS service.
- **Later**: remote backups, RBAC, multi-node agents, Velocity, Bedrock/Geyser, other games.

## Non-goals
- Not a Minecraft *client* launcher.
- No reimplementation of the Minecraft protocol beyond SLP, RCON and MCSMP.
- No mirroring or re-hosting of mod files (respects Modrinth and CurseForge terms).
