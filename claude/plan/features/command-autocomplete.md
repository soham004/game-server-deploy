# Feature: Minecraft Command Autocomplete

**Feasible.** Here is how, in three layers of data.

## Layer 1: Vanilla Brigadier tree (exact, per MC version)
- Generate once per MC version (background job, ~10–30 s) from the **vanilla** server jar, even for modded instances
  (we cache the vanilla jar anyway):
  `java -DbundlerMainClass=net.minecraft.data.Main -jar server.jar --reports --output <tmp>`
  → `<tmp>/reports/commands.json` (+ `registries.json`, `items.json`, `blocks.json`).
  For <1.18 (no bundler): `java -cp server.jar net.minecraft.data.Main --reports`.
- Store a compacted copy in `cache/commands/<mc>.json` along with registry id lists (items, blocks, entities, effects,
  enchantments, biomes, dimensions, gamerules, sounds, particles).
- Fallback when generation fails: download the matching tree from `misode/mcmeta` (GitHub raw) if the user allows network.

## Layer 2: Mod and plugin commands (approximate)
- When the server reaches Running, send `help` (silently, via RCON if available so it doesn't spam the console; otherwise
  through stdin while tagging and hiding the lines in the UI) and parse the usage lines, for example
  `/chunky <radius|center|start|pause|...>` or `/spark profiler [start|stop|open]`.
- `help_parser.rs` converts usage grammar → tree nodes: `<x>` = argument (type unknown → free text), `[x]` = optional,
  `(a|b)` / `<a|b>` = literal alternatives, `...` = greedy.
- Merge into the vanilla tree: commands not present in vanilla are added from help; vanilla nodes win on conflict.
- Paper `help` output is paginated and plugin-oriented (`/help <plugin>`). Handle pagination with `help <n>`, or use
  `bukkit:help` per plugin. Spend limited effort here: complete command **names** for sure, arguments when parseable.

## Layer 3: Dynamic values
Argument parser → suggestion source:
| Parser | Source |
|---|---|
| `minecraft:entity`, `minecraft:game_profile`, `minecraft:score_holder` | online players (supervisor/MCSMP) + `@a @p @r @s @e @n` |
| `minecraft:item_stack`, `item_predicate` | items registry |
| `minecraft:block_state`, `block_predicate` | blocks registry |
| `minecraft:resource`/`resource_key` (with `registry` property) | the matching registry list |
| `minecraft:dimension` | dimensions |
| `minecraft:gamemode` | survival, creative, adventure, spectator |
| gamerule literals | already literals in the tree |
| `brigadier:bool` | true/false |
| `brigadier:integer/float` (with min/max) | show a hint "<int 0..100>" only |
| `minecraft:vec3`/`block_pos` | `~ ~ ~` and `^ ^ ^` hints |

## Completer algorithm (gsd-minecraft/commands/completer.rs)
1. Tokenise the input up to the cursor (respect quotes; greedy strings consume the rest).
2. Walk the tree from the root, following literals exactly and arguments with a lightweight per-parser "can consume"
   check, and following `redirect` (e.g. `execute … run`) back to the root.
3. At the cursor token, collect children: literals filtered by prefix, plus argument suggestions from sources (prefix
   and fuzzy match), plus a hint for unknown arguments.
4. Return `[{ text, replace_range, kind: literal|player|item|…, detail }]`, max ~50, ranked: exact prefix > fuzzy; literals before values.
- Pure function over `(tree, dynamic_sources, input, cursor)`, heavily unit-tested with tables of cases.
- Served by `GET /instances/{id}/complete` (debounced 80 ms client-side). Could later run in the browser by shipping
  the tree JSON; the server-side approach keeps one implementation.

## UI
Popup under the console input: ↑/↓ to move, Tab/Enter to accept, Esc to close. Show argument signature hints
(`/give <targets> <item> [<count>]`) above the input with the current argument bold.
