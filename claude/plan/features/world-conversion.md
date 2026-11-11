# World conversion between server types (planned, researched 2026-11-10)

A world uploaded from one kind of server to another can fail to load. This file records what was found on real 26.3 servers
and what to build. Nothing here is built yet; the next decision number is D-060.

## What happens today (reproduced)

| Uploaded world from | Played on | Result |
|---|---|---|
| Paper 26.3 | vanilla 26.3 | **Crash:** `Missing data pack paper`, `Failed to load datapacks, can't proceed with server load`, `java.lang.IllegalStateException: Overworld settings missing` |
| Purpur 26.3 | vanilla 26.3 | Same crash as Paper |
| Paper 26.3 | Fabric 26.3 | Same crash (Fabric runs the vanilla server) |
| vanilla 26.3 | Paper 26.3 | Works. Paper converts it itself: `[WorldFolderMigration]` prints an alert, **waits 30 seconds**, then `[VanillaWorldMigration] ... Completed Vanilla import` for each dimension |
| NeoForge 26.3 (Chunky only) | vanilla 26.3 | Works; warns `Missing data pack mod_data` |
| Fabric 26.3 | vanilla 26.3 | Works |
| Paper 26.3, converted by hand (below) | vanilla 26.3 | Works; `seed` prints the Paper world's seed; vanilla drops the `paper` pack from `level.dat` when it saves |
| that converted world | Paper 26.3 | Works, seed kept, but Paper ran no migration because the per-dimension copies were still there (the files had been copied, not moved) |

Also found: the Worlds tab shows **no seed** for Paper 26.x worlds, because the app only reads
`data/minecraft/world_gen_settings.dat`.

## Why: the layouts

**26.1 and later**
- **Vanilla, Fabric, Quilt, Forge, NeoForge:** `world/level.dat`, plus world-wide data in `world/data/minecraft/`:
  `world_gen_settings.dat` (seed and dimensions), `game_rules.dat`, `weather.dat`, `scheduled_events.dat`,
  `wandering_trader.dat`, and others. Each dimension is in `world/dimensions/minecraft/<overworld|the_nether|the_end>/`,
  holding `region/`, `entities/`, `poi/` and `data/minecraft/` (chunk tickets, raids, world border).
- **Paper and Purpur (26.1+):** the same tree, but Paper treats each dimension as its own world. So `world_gen_settings.dat`,
  `game_rules.dat`, `weather.dat`, `scheduled_events.dat`, `wandering_trader.dat` and `world_clocks.dat` live in *each*
  dimension's `data/minecraft/`, and Paper removes them from `world/data/minecraft/`.
- **Paper's own data:** `data/paper/{level_overrides,metadata,persistent_data_container}.dat` and `paper-world.yml` per
  dimension. `level.dat` gains `Bukkit.Version` and `paperSpawnDimension`, and `DataPacks.Enabled` gains `paper`.
- Vanilla needs `data/minecraft/world_gen_settings.dat` at the top, hence "Overworld settings missing".
- Paper's overworld copy of `world_gen_settings.dat` holds all three dimensions and the seed (checked by dumping the NBT).

**Before 26.1 (Bukkit layout; all Paper, Purpur and Spigot up to 1.21.11)**
- Vanilla: `world/` with the nether in `world/DIM-1/` and the end in `world/DIM1/`.
- Bukkit: `world/` (overworld only), `world_nether/DIM-1/`, `world_the_end/DIM1/`. A vanilla server given only `world/` plays
  the overworld; the nether and end look reset.
- The app already lists `<w>_nether` and `<w>_the_end` as a world's parts (D-055).

**Paper's own documentation** ([migration](https://docs.papermc.io/paper/migration/)) says the same:
- **Paper to vanilla (26.1+):** stop the server, then move `game_rules.dat`, `scheduled_events.dat`, `wandering_trader.dat`,
  `weather.dat` and `world_gen_settings.dat` from `world/dimensions/minecraft/overworld/data/minecraft/` to
  `world/data/minecraft/`.
- **Paper to vanilla (before 26.1):** copy `world_nether/DIM-1` and `world_the_end/DIM1` into `world/`, then delete those two
  folders.
- **Vanilla, Fabric or Forge to Paper:** automatic. Modded blocks and items are lost without their mods.
- **Spigot or CraftBukkit 26.1 to Paper:** "fundamentally impossible" directly; go through vanilla.
- **26.1 release note** ([papermc.io/news/26-1](https://papermc.io/news/26-1)): a world upgraded to 26.1 can't be downgraded.
  Paper 26.1+ imports old `world_nether` and `world_the_end` folders on first start, then deletes them.

**Paper's migration pause:** Paper 26.3's `io.papermc.paper.world.migration.WorldFolderMigration` reads the system property
`paper.disableMigrationDelay` ("Migration delay disabled by system property."). Found by scanning the jar's strings; not yet
run.

## What to build

### 1. Read the world's kind (`worlds/info.rs`, `worlds/service.rs`)
Add `origin` and `layout` to `World`, from files only:
- **`layout`:**
  - `modern_split`: Paper 26.1+, which has `dimensions/minecraft/overworld/data/minecraft/world_gen_settings.dat` and no
    top-level one.
  - `modern`: 26.1+ vanilla.
  - `bukkit`: before 26.1, with `<w>_nether` or `<w>_the_end` siblings, or no `DIM-1` inside while a sibling has one.
  - `legacy`: before 26.1, vanilla.
- **`origin`:** from `level.dat`. `ServerBrands`, `Bukkit.Version` or `paperSpawnDimension` mean Paper or Purpur;
  `WasModded` together with a fabric, quilt, forge or neoforge brand means modded. Keep the raw brands for display.
- **Seed:** also read `dimensions/minecraft/overworld/data/minecraft/world_gen_settings.dat` when the top-level one is
  missing. This is a small bug fix and can ship first.
- **`DataVersion`:** keep the world's from `level.dat`. Get the server's from the jar's `version.json` (`world_version`), or
  from the catalog, then compare (see 4).

### 2. Convert for the server's family (a job, after a `pre_delete`-style backup of that world)
The target family comes from the instance's loader: vanilla family (vanilla, Fabric, Quilt, Forge, NeoForge) or Paper family
(Paper, Purpur).
- **`modern_split` to vanilla family:**
  - **Move, not copy,** the five files from `dimensions/minecraft/overworld/data/minecraft/` to `data/minecraft/`. Moving
    keeps a later trip back to Paper clean: when both copies existed, Paper skipped its import (seen in the round trip).
  - Leave the nether's and end's copies, Paper's `data/paper/` and `paper-world.yml`: vanilla ignores them and Paper can
    use them again.
  - The nether's and end's own game rules are lost on vanilla, which has one set. Say so when they differ from the
    overworld's.
  - No need to edit `level.dat`: vanilla only warns about the `paper` pack, and drops it on save.
- **`bukkit` to vanilla family (worlds before 26.1):** move `<w>_nether/DIM-1` to `<w>/DIM-1` and `<w>_the_end/DIM1` to
  `<w>/DIM1`. Only do it when the target isn't already there; otherwise stop and say so. Then delete the empty parts.
  Vanilla 26.x then upgrades the world itself.
- **Anything to Paper family:** nothing to change; Paper converts it on start. Before that first start:
  - Take the backup that Paper's alert asks for.
  - Start once with `-Dpaper.disableMigrationDelay=true` (check this first; see Tests).
  - Show "Paper is converting the world" from its `[WorldFolderMigration]` and `[VanillaWorldMigration]` lines.
- **Modded to vanilla or Paper:** no file change. Warn that blocks, items and dimensions from mods are lost. A dimension
  folder outside `minecraft/` under `dimensions/` is a sign of mods (or of Paper's extra worlds).
- **Spigot/CraftBukkit 26.1+:** not a loader the app installs. If found (a `Bukkit.Version` without Paper), refuse with
  "convert it on vanilla first", as Paper's docs say.

### 3. Where it shows
- **On upload (`PUT /worlds/{folder}`):** after unpacking, compare the world's layout with the server's family.
  - A mismatch the app can fix: convert as part of the upload (it is a fresh copy; nothing to back up) and report it in the
    response, e.g. `converted: "paper_to_vanilla"`.
  - A mismatch it can't fix: keep the world, and attach a `warning` code and message.
- **On cards:** a chip such as "From Paper: vanilla can't load it as it is", with a **"Convert for this server…"** button.
  That calls a new `POST /instances/{id}/worlds/{folder}/convert` (`files.write`, stopped, 202 job, backup first).
- **Activate:** refuse, or ask first, when the active world needs converting. Saying it up front beats a crash on start.
- **Uploading a Bukkit set:** a zip holding `world/`, `world_nether/` and `world_the_end/` (a whole old Paper server's
  worlds) is refused today, because upload expects one world. Accept a zip whose top folders are one world and its parts:
  unpack them as `<folder>`, `<folder>_nether` and `<folder>_the_end`, then convert as in 2.

### 4. Version checks (same place)
- **A newer world on an older server:** the world's `DataVersion` is above the server's. The game refuses it, and
  downgrading isn't possible. Refuse activation and warn on upload: "made by Minecraft X; this server runs Y".
- **An older world on a newer server:** the game upgrades it, and that can't be undone. Warn and offer a backup, as change
  version already does (D-038).

### 5. Related checks to make while there
- **Game rules on Paper 26.x (D-054):** editing a stopped world writes `data/minecraft/game_rules.dat`. On Paper 26.x that
  file lives per dimension, so check whether Paper reads the edit. If not, write the overworld's (or every dimension's).
- **Backups and clone:** `world_folders` names `<w>_nether` and `<w>_the_end`, which is still right for pre-26.1 Paper.
  Nothing to change.
- **Linear region files** (`.linear`, from forks such as Leaf or LinearPaper): not a loader the app installs. If found,
  warn that vanilla and Paper can't read them.

## Tests to write
- **Unit (`worlds/`):** layout and origin detection from fixtures, built by copying real files:
  - A Paper 26.3 world (`level.dat` plus the overworld's `world_gen_settings.dat`).
  - A vanilla 26.3 world, a 1.20.1 world, and a Bukkit-split set.
  - The seed from the per-dimension file.
- **API (`crates/gsd-app/tests/worlds.rs`):**
  - Upload a Paper-split zip to a vanilla server: the files moved, `converted` reported.
  - Upload a Bukkit set as one zip.
  - Convert endpoint: backup first, rights, refused while running.
  - A newer `DataVersion` refused on activate.
- **Real servers (`tests/e2e/worlds.e2e.mjs`, as a normal user and as root):**
  - The Paper world on vanilla: it runs, and `seed` matches.
  - Back to Paper: it runs, and Paper's migration lines appear.
  - Vanilla to Paper with `-Dpaper.disableMigrationDelay=true`: no 30-second wait.
  - Purpur to vanilla.
  - A NeoForge world on vanilla: the warning shows.
  - For pre-26.1 Bukkit worlds, if practical: a 1.21.x Paper server's world on 1.21.x vanilla, checking that the nether is
    kept (e.g. `execute in minecraft:the_nether run ...` against a placed block, or the region files' sizes).
- **Browser:** the card's chip, the "Convert…" flow and the toast.

## How it was reproduced (for the next run)
1. Serve the app on the loaders' test data (`tests/e2e/serve.sh`).
2. Through the API, download "E2E paper"'s `world` and upload it to "E2E vanilla" as `paper_world` with `?activate=true`.
3. Start the server and read the console.
4. Hand conversion: copy those files to `data/minecraft/` (the test copied; the plan says move), then start again.
5. The research worlds were removed from the test servers afterwards.
