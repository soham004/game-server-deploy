# Feature: Players, Gamerules and Worlds

## Players (built, D-053)
- **Changes go one way per state:** the management protocol (running, 1.21.9+), console commands (running, older, or when the
  protocol can't be reached), the server's files (stopped). Nothing while it starts or stops. Every change is confirmed in the
  server's own lists before it counts.
- **Reads come from the files** in every state (`ops.json`, `whitelist.json`, `banned-players.json`, `banned-ips.json`,
  `usercache.json`): the server saves them as soon as they change. Who is on comes from the console's join and leave lines.
- **Directory:** online now (with how long), operators (level), allowlist (on/off), bans (reason, source, end), address bans, and
  everyone seen (`usercache.json` and the visit history), with a search.
- **Adding by name** on a stopped server: the server's name cache, else Mojang (online mode), else the offline UUID. A running
  server looks names up itself.
- **Heads:** cut from the skin server-side and kept a day (`/players/{uuid}/head.png`), a letter tile of our own otherwise.
- **A player's card:** head, name, UUID (copy), first and last seen, play time (the game's statistics, else the visits), visits,
  and actions: operator (level where it can be chosen), allowlist, ban (reason, how long: forever, an hour, a day, 7 or 30 days),
  unban, kick, message.
- Bans that end are the game's own (`expires`), through the protocol or the files. On older running servers only bans for good.
- Later: teleport to spawn (P2), temporary bans on older running servers, play time per world.

## Gamerules (built, D-054)
- The server's own list: live from the management protocol (running 1.21.9+), else from the world's files
  (`data/minecraft/game_rules.dat` since 1.21.11, `level.dat` `Data.GameRules` before). A catalog adds labels, groups,
  defaults and descriptions; unknown rules (mods, newer versions) show under Other.
- Changes: the protocol; the console then a save (older running servers); the world's file while stopped. Confirmed in
  the server's values afterwards.
- UI: grouped (Players, Mobs, Spawning, Drops, World, Chat and messages, Commands, Other), switches and number fields,
  search, "only changed from the default", a Changed mark with Reset, one Save for all edits.

## Worlds (built, D-055, D-059)
- Every top-level folder with a `level.dat` (or `level.dat_old` or `session.lock`, left by a world that broke), and the
  `level-name` folder, with what its files say: version (`Data.Version.Name`), last played, game mode,
  difficulty, hardcore, seed (`WorldGenSettings.seed`, or `data/minecraft/world_gen_settings.dat` since 1.21.11), datapacks,
  size, and whether it is the active one (`level-name`). Older Paper's `_nether`/`_the_end` folders are its parts.
- Make active (`level-name`), download a zip (stopped), upload a zip as a new world (a folder inside it is found; no
  `level.dat`, no world), delete any world (the active one too: the next start makes a new one) or reset the active one (stopped; a `pre_delete` backup first; a reset may
  set a new seed).
- Later: datapacks managed (P2), a map preview (P3): link to BlueMap/Dynmap/squaremap when detected.
