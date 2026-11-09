# Feature: Mods, Plugins, Datapacks and Modpacks (Modrinth + CurseForge)

## Content kinds by instance type
| Instance loader | Folder | Modrinth `project_type` / loader facet | CurseForge `classId` / `modLoaderType` |
|---|---|---|---|
| Fabric / Quilt | `mods/` | mod / `fabric` (Quilt also accepts `fabric` mods) | 6 / 4 or 5 |
| Forge | `mods/` | mod / `forge` | 6 / 1 |
| NeoForge | `mods/` | mod / `neoforge` | 6 / 6 |
| Paper / Purpur / Folia | `plugins/` | plugin / `paper`, `spigot`, `bukkit`, `purpur`, `folia` | 5 (Bukkit plugins) |
| Any | `<world>/datapacks/` | datapack | (P2) |

**Priority (user, 2026-11-07): mods and plugins are equally important in v1.** Sources in v1: Modrinth (mods + plugins),
Hangar (plugins), CurseForge (mods + Bukkit plugins, user-supplied key). SpigotMC/Spiget is P3.

## Source abstraction (gsd-mods)
`source::Source` is an enum over the clients, with `search`, `project`, `projects` and `versions` (D-043). `ForServer`
narrows one to a server for the resolver. Lookups by hash stay on the clients that have them.
- Normalised `Project` has `server_side: SideSupport` (Modrinth gives it; CurseForge doesn't, so it is `Unknown`, and
  we can cross-check by Modrinth slug match or hash when the same file exists on both).
- A shared rate limiter per source (Modrinth 300/min), retry with jitter on 429/5xx, cache search pages for 60 s.

## Install flow (job)
1. Pick the version: the user's choice, or the newest compatible with `CompatFilter` (release > beta > alpha).
2. **Resolve dependencies** (resolver.rs): BFS over required deps, choosing compatible versions; detect conflicts
   (an already-installed different version, `incompatible` relations) → present a plan: "Will install X, Y (dependency),
   replace Z 1.2 → 1.3". The user confirms in the UI unless it is just the one file.
3. Download each file (content-addressed cache), verify hashes (Modrinth sha512; CurseForge has sha1 in `hashes`).
4. Place the jar, write `mods/.index/<slug>.pw.toml` (packwiz format, see the research doc), and update the instance's
   content list in memory.
5. If the server is running → "Restart required" banner.

## Installed content list
- Scan the folder: each jar → hash (sha1, sha512, CF murmur2 fingerprint; cached by size+mtime in `.index` or the DB) →
  join with the `.pw.toml` index → read embedded metadata (`fabric.mod.json`, `quilt.mod.json`,
  `META-INF/mods.toml`/`neoforge.mods.toml`, `plugin.yml`/`paper-plugin.yml`) for name, version, icon and mod id.
- Unknown jars (no index) → "Identify" job: Modrinth `POST /version_files` by sha1, then CurseForge `POST /fingerprints`.
- Columns: icon, name, version, source badge, side badge (⚠ "client-only, won't work on a server"), enabled toggle, update badge.
- Enable/disable = rename to `.jar.disabled` (Prism compatible). Remove = delete jar + index file.

## Updates
- Modrinth: `POST /version_files/update` with all sha1s plus loader and MC filters → candidates.
- CurseForge: `GET /mods/{id}/files?gameVersion&modLoaderType` for each indexed project (batched).
- The UI shows "N updates available"; "Update selected" makes a full backup (`pre_update`; full, not the mods folder:
  updates can migrate world data, D-040), then replaces files.
  Changelogs are shown inline when available.

## CurseForge specifics
- API key from Settings, stored server-side, never sent to the browser; all CF calls are proxied.
- **Blocked distribution** (`downloadUrl=null`): the job installs everything else and leaves these out; the UI lists them
  with "Open on CurseForge" links and a drop zone. Uploaded files are fingerprinted and matched (D-044: no paused job).
- Show CF attribution and links per their terms.
- Tests use the real API with a key from `CURSEFORGE_API_KEY` or `.env`, which never ships (D-046).

## Modpacks
Built so far: uploads only (D-045). Below is the full plan; search and `[modpack]` records are still to come.
- **Modrinth `.mrpack`** (from search or file upload): read `modrinth.index.json` → required MC + loader → create the instance
  via the installer → download files where `env.server != unsupported` (files with no env go in and get flagged) →
  extract `overrides/` then `server-overrides/` → record `[modpack]` in instance.toml.
- **CurseForge**: if the pack file has `serverPackFileId`, download the **official server pack** zip and import it as a
  folder (detect the loader). Otherwise use `manifest.json` (`minecraft.version`, `modLoaders[primary]`, `files[{projectID,fileID,required}]`)
  → batch `POST /mods/files` → download (blocked → manual flow) → copy `overrides/`. Client-only mods can't be detected
  reliably on CF, so offer a "check against Modrinth" pass and a known-client-only list (shaders, minimaps, Sodium/Iris/…)
  as warnings, not silent removals.
- Pack updates: diff the old and new manifests → add, remove and update files, preserving user-changed configs (P2:
  three-way merge; v1: overwrite only files the pack owns and back up first).

## UI
- "Content" tab: Installed | Browse (Modrinth / CurseForge switch) | Updates.
- Browse is pre-filtered to the instance (MC version, loader, server-compatible); filters for categories, sort, and an
  "include client-only" checkbox (off by default). Project page drawer: description (rendered markdown, sanitised),
  gallery, versions table, dependencies, an Install button with a version dropdown.
