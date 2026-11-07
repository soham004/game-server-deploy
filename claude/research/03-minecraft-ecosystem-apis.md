# Research 03: Minecraft Ecosystem APIs (versions, loaders, mods, Java)

> Researched 2026-11-07. Endpoints marked **(verified)** were fetched live while researching. Others come from docs
> or third-party code and must be checked with a real request before implementing. Every external client must
> send a descriptive `User-Agent`, for example `game-server-deploy/<version> (+repo url)`. Paper and Modrinth
> reject or block generic agents.

## 0. Important context: the 2026 version numbering
- Mojang switched to **year-based versions in 2026**: `26.1`, `26.1.1`, `26.1.2`, `26.2`, `26.3`. Snapshots look like
  `26.4-snapshot-7`. Before that came `1.21.11`. Version parsing must handle **both** `1.x.y` and `26.x[.y]`, plus
  snapshots (`25w35a`, `26.4-snapshot-N`), pre-releases (`-pre1`, `-rc1`) and old ids (`b1.7.3`, `a1.2.6`, `rd-132211`).
  **Never sort by string.** Sort by the manifest's `releaseTime`, which is authoritative.
- As of today: latest release **26.3** (2026-09-15), latest snapshot **26.4-snapshot-7** (verified).

## 1. Minecraft versions (Mojang)
| What | Endpoint |
|---|---|
| Version manifest **(verified)** | `GET https://piston-meta.mojang.com/mc/game/version_manifest_v2.json` |
| Shape | `{ latest: {release, snapshot}, versions: [{ id, type, url, time, releaseTime, sha1, complianceLevel }] }` |
| `type` values | `release`, `snapshot`, `old_beta`, `old_alpha` (Prism's "Experiments" filter = special experimental snapshots, a P3 concern) |
| Per-version JSON | `versions[i].url`, which contains `downloads.server.{url, sha1, size}` and `javaVersion.{component, majorVersion}` |

Notes:
- Very old versions have **no `downloads.server`**. Hide them in the picker for vanilla.
- `javaVersion.majorVersion` gives the required Java, so we never hard-code a table. Known values: 26.x → **25**,
  1.20.5–1.21.11 → 21, 1.18–1.20.4 → 17, 1.17 → 16, ≤1.16 → 8.
- Cache the manifest with a TTL (~10 min) and keep a disk copy so the app works offline.
- Since 1.18 the server jar is a **bundler** jar (it unpacks libraries into `versions/` and `libraries/` on first run).

## 2. Server software / loaders

### 2.1 Vanilla
Download `downloads.server.url`, verify `sha1`, and launch with `java -Xmx.. -jar server.jar nogui`.

### 2.2 Paper / Folia / Velocity, via the PaperMC "Fill" v3 API
| What | Endpoint |
|---|---|
| Project **(verified)** | `GET https://fill.papermc.io/v3/projects/paper` → `{ project:{id,name}, versions: { "<group>": ["<newest>", …] } }` (groups: 26.3, 26.2, 26.1, 1.21, 1.20 … 1.7) |
| Builds | `GET https://fill.papermc.io/v3/projects/paper/versions/{version}/builds` → `[{ id, channel, downloads: { "server:default": { name, url, checksums:{sha256}, size } } }]` |
| Docs | Swagger at `https://fill.papermc.io/swagger-ui/index.html` (check for a `builds/latest` route) |

Rules:
- The v2 API (`api.papermc.io/v2`) was **shut down on 2026-07-01**. Use only Fill v3.
- **Use the download URL from the response**, never build it (it points at `fill-data.papermc.io`).
- Prefer `channel == "STABLE"`. Show experimental builds behind a toggle.
- A `User-Agent` is mandatory.
- Folia (regionised multithreading) uses the same API with `projects/folia`. Mark it "advanced".

### 2.3 Purpur (Paper fork)
- `GET https://api.purpurmc.org/v2/purpur` (versions), `GET …/v2/purpur/{version}` (builds),
  `GET …/v2/purpur/{version}/{build|latest}/download`. *Unverified: check before implementing.*

### 2.4 Fabric
| What | Endpoint |
|---|---|
| Game versions | `GET https://meta.fabricmc.net/v2/versions/game` (`[{version, stable}]`) |
| Loaders for a game version | `GET https://meta.fabricmc.net/v2/versions/loader/{game}` (`[{loader:{version,stable}, intermediary, launcherMeta}]`) |
| Installers | `GET https://meta.fabricmc.net/v2/versions/installer` |
| **Server launcher jar** (route verified) | `GET https://meta.fabricmc.net/v2/versions/loader/{game}/{loader}/{installer}/server/jar` |

The launcher jar (`fabric-server-launch.jar`) downloads the vanilla server and libraries itself on first start.
That makes Fabric the **simplest modded install**: download one jar, run it with `-jar`.

### 2.5 Quilt (beta)
- Loader versions: `GET https://meta.quiltmc.org/v3/versions/loader/{game}` *(verify shape)*.
- Install via the **installer CLI**:
  `java -jar quilt-installer.jar install server <mcVersion> [loaderVersion] --download-server --install-dir=<dir>`.
  It produces `quilt-server-launch.jar` plus `libraries/`. The installer jar comes from
  `https://maven.quiltmc.org/repository/release/org/quiltmc/quilt-installer/` (maven-metadata.xml for the latest).

### 2.6 Forge
| What | Endpoint |
|---|---|
| Promotions **(verified)** | `GET https://files.minecraftforge.net/net/minecraftforge/forge/promotions_slim.json` → `{ homepage, promos: { "26.3-latest": "66.1.5", "26.3-recommended": "66.1.0", … } }` |
| All versions | `GET https://maven.minecraftforge.net/net/minecraftforge/forge/maven-metadata.xml` (entries look like `1.21.11-61.2.5`; **check the 26.x format**) |
| Installer | `https://maven.minecraftforge.net/net/minecraftforge/forge/{mc}-{forge}/forge-{mc}-{forge}-installer.jar` |

Install: `java -jar forge-…-installer.jar --installServer <dir>`.
- **1.17+**: produces `run.sh`/`run.bat`, `user_jvm_args.txt` and `libraries/net/minecraftforge/forge/<v>/{unix,win}_args.txt`.
  Launch with `java @user_jvm_args.txt @libraries/.../win_args.txt nogui` (we build this command ourselves; we do not
  run their `run.bat`).
- **≤1.16**: produces `forge-<v>.jar` (or `-universal`), launched with `-jar`.
- ⭐ in the picker = the `-recommended` promo (matches the user's screenshot: 66.1.0 is starred for 26.3).

### 2.7 NeoForge
| What | Endpoint |
|---|---|
| Versions **(verified)** | `GET https://maven.neoforged.net/api/maven/versions/releases/net/neoforged/neoforge` → `{ isSnapshot, versions: [...] }`, oldest first |
| Installer | `https://maven.neoforged.net/releases/net/neoforged/neoforge/{v}/neoforge-{v}-installer.jar` |
| 1.20.1 (legacy) | `net/neoforged/forge` artifact, versions like `1.20.1-47.1.x` |

Mapping NeoForge version to MC version:
- 1.20.2 – 1.21.x: drop the leading `1.`, so `21.1.77` → MC `1.21.1`, `21.0.x` → MC `1.21`, `20.4.x` → `1.20.4`.
- **26.x**: the MC version is the prefix, so `26.3.0.101` → MC `26.3`, `26.1.2.71` → MC `26.1.2`.
- Suffixes: `-beta`, `-alpha.N+snapshot-N`, `+pre-N`. Treat anything with a suffix as unstable.

Install: `java -jar neoforge-…-installer.jar --install-server <dir>` (**verify the flag with `--help`**; Forge
historically used `--installServer`). The output layout is the same as modern Forge (args files).

### 2.8 Not in v1
- **Spigot/CraftBukkit**: no official binaries; requires compiling with BuildTools. P3.
- **Hybrids** (Mohist, Arclight, Magma, Youer): P3, often unstable.
- **SpongeVanilla/SpongeForge**: P3.
- **Velocity/BungeeCord proxies**: P3 (Velocity uses Fill v3).

## 3. Mod platforms

### 3.1 Modrinth API v2 (primary, open, no key needed)
- Base `https://api.modrinth.com/v2`, staging at `https://staging-api.modrinth.com/v2`.
- **Rate limit 300 req/min per IP**. Read the `X-Ratelimit-Limit`, `-Remaining` and `-Reset` headers.
- **Unique User-Agent required** (`owner/project/version (contact)`).
- IDs are 8-character base62. **Store IDs, not slugs** (slugs can change).
- Old API versions eventually return **HTTP 410**. Handle it with a clear "update the app" error.

Key endpoints (confirm parameter details in the docs at implementation time):
| Purpose | Request |
|---|---|
| Search | `GET /search?query=&facets=[["project_type:mod"],["categories:fabric"],["versions:26.3"],["server_side:required","server_side:optional"]]&index=relevance\|downloads\|follows\|newest\|updated&offset=&limit=` (facets: AND between inner arrays, OR within one) |
| Project(s) | `GET /project/{id\|slug}`, `GET /projects?ids=[...]` |
| Versions of a project | `GET /project/{id}/version?loaders=["fabric"]&game_versions=["26.3"]` |
| Dependencies | `GET /project/{id}/dependencies` (version deps carry `dependency_type`: required, optional, incompatible, embedded) |
| Identify files | `POST /version_files` `{hashes:[sha1...], algorithm:"sha1"}` |
| **Bulk update check** | `POST /version_files/update` `{hashes, algorithm, loaders, game_versions}` → newest compatible version per hash |
| Tags | `GET /tag/game_version`, `GET /tag/loader`, `GET /tag/category` |

Project fields we rely on: `client_side` and `server_side` (`required` / `optional` / `unsupported` / `unknown`),
`project_type` (`mod`, `modpack`, `plugin`, `datapack`, `resourcepack`, `shader`), `loaders`, `game_versions`, `icon_url`.
Plugin loaders on Modrinth: `paper`, `spigot`, `bukkit`, `purpur`, `folia`, `velocity`.

**`.mrpack` format** (modpacks): a zip with `modrinth.index.json` (`formatVersion`, `game`, `versionId`, `name`,
`files[{path, hashes{sha1,sha512}, env{client,server}, downloads[], fileSize}]`,
`dependencies{minecraft, fabric-loader | quilt-loader | forge | neoforge}`), plus `overrides/`, `server-overrides/` and
`client-overrides/` folders. Server install: skip files with `env.server == "unsupported"`, apply `overrides/` then
`server-overrides/`. A missing `env` is ambiguous: install the file but flag it for review.

### 3.2 CurseForge API (secondary, needs an API key)
- Base `https://api.curseforge.com/v1`, header `x-api-key: <key>`. **Every** request needs a key, even for public data.
- Keys come from the CurseForge for Studios console (an application is required). Decision: the user can paste
  their own key in Settings, and we may apply for an app key later. **The key must never reach the browser**: all
  CurseForge calls are proxied through the Rust backend.
- Constants: `gameId = 432` (Minecraft). `classId`: 6 = mods, 4471 = modpacks, 5 = Bukkit plugins, 12 = resource
  packs, 17 = worlds *(verify the IDs via `GET /categories?gameId=432&classesOnly=true`)*.
- `modLoaderType`: 0 Any, 1 Forge, 2 Cauldron, 3 LiteLoader, 4 Fabric, 5 Quilt, 6 NeoForge.

| Purpose | Request |
|---|---|
| Search | `GET /mods/search?gameId=432&classId=6&gameVersion=26.3&modLoaderType=6&searchFilter=&sortField=&sortOrder=desc&index=&pageSize=` |
| Mod | `GET /mods/{modId}` (has `allowModDistribution`) |
| Files | `GET /mods/{modId}/files?gameVersion=&modLoaderType=` |
| Download URL | `GET /mods/{modId}/files/{fileId}/download-url` |
| Identify by fingerprint | `POST /fingerprints/432` `{fingerprints:[u32...]}` |
| Batch files | `POST /mods/files` `{fileIds:[...]}` |

- **Fingerprint** = MurmurHash2 (seed 1) over the file bytes **with whitespace bytes (9, 10, 13, 32) removed**.
- **Distribution opt-out**: if `allowModDistribution == false` or the file's `downloadUrl == null`, we must not
  download it automatically. Show Prism's "blocked mods" dialog (links to the website, drag-and-drop the downloaded
  jars, match by fingerprint).
- File relations `relationType`: 1 EmbeddedLibrary, 2 Optional, 3 Required, 4 Tool, 5 Incompatible, 6 Include.
- Modpack files can carry `serverPackFileId`. **Prefer the official server pack** when one exists.
- Respect the ToS: no mirroring or re-hosting of files, show attribution and a link back.

### 3.3 Hangar (PaperMC plugin repository), P2
`https://hangar.papermc.io/api/v1/projects?...`, versions per platform (`PAPER`, `VELOCITY`, `WATERFALL`).

### 3.4 Packwiz metadata (`mods/.index/*.pw.toml`), used by Prism
```toml
name = "Sodium"
filename = "sodium-fabric-0.6.0+mc1.21.1.jar"
side = "both"            # client | server | both
[download]
url = "https://cdn.modrinth.com/..."
hash-format = "sha512"
hash = "..."
[update.modrinth]
mod-id = "AANobbMI"
version = "..."
# or [update.curseforge] project-id = 123, file-id = 456
```
We use the same format so instances stay interoperable with Prism and packwiz.

## 4. Java runtimes (Adoptium / Eclipse Temurin)
- Latest asset: `GET https://api.adoptium.net/v3/assets/latest/{feature}/hotspot?os={windows|linux|mac}&architecture={x64|aarch64}&image_type=jre&vendor=eclipse`
  → `[{ binary: { package: { link, checksum, size, name } }, version }]`.
- Direct redirect: `https://api.adoptium.net/v3/binary/latest/{feature}/ga/{os}/{arch}/jre/hotspot/normal/eclipse`
  (the checksum is at `<url>.sha256.txt`).
- Alternative: Mojang's own runtime manifest (what the vanilla launcher uses). Temurin is simpler and covers
  arm64 Linux (Raspberry Pi).
- **Detection** of existing installs: `JAVA_HOME`, `PATH`, the Windows registry
  (`HKLM\SOFTWARE\JavaSoft`, `Eclipse Adoptium`, `Microsoft`, `Azul`), `/usr/lib/jvm/*`,
  `/Library/Java/JavaVirtualMachines/*`, `~/.sdkman/candidates/java/*`. Read `<home>/release` (`JAVA_VERSION=`) or
  run `java -XshowSettings:properties -version` and parse `java.version` and `os.arch`.
- On this dev machine only Java 8 (Oracle) is on PATH, so the auto-download path matters.

## 5. Player identity
- Name → UUID: `GET https://api.mojang.com/users/profiles/minecraft/{name}` → `{id, name}` (204/404 if not found).
- UUID → profile and skin: `GET https://sessionserver.mojang.com/session/minecraft/profile/{uuid}` (base64 textures).
  Render heads ourselves from the skin PNG (crop 8×8 at (8,8) plus the hat layer at (40,8)), cached on disk.
- Offline-mode UUID = name-based UUID v3 of `"OfflinePlayer:" + name` (MD5). Needed when `online-mode=false`.
- Rate limits are aggressive (~600 req / 10 min). Cache results, and use `usercache.json` from the server first.

## 6. Misc services
- **mclo.gs** log sharing: `POST https://api.mclo.gs/1/log` (form field `content`) → `{success, url, raw}`. P2.

## Sources
- [Mojang version manifest](https://piston-meta.mojang.com/mc/game/version_manifest_v2.json) · [Java requirements (Wepwawet)](https://docs.wepwawet.net/games/minecraft/setup/java-version) · [GameServerKings: 2026 numbering](https://www.gameserverkings.com/knowledge-base/minecraft/updating-your-server-version/)
- [PaperMC Downloads Service](https://docs.papermc.io/misc/downloads-service/) · [Fill v3 project](https://fill.papermc.io/v3/projects/paper) · [itzg issue on Fill v3 migration](https://github.com/itzg/docker-minecraft-server/issues/3517)
- [Fabric Meta](https://meta.fabricmc.net/) · [Quilt server install](https://quiltmc-org.pages.dev/en/install/server) · [MSCS Quilt notes](https://minecraftservercontrol.github.io/docs/mscs/adjusting-world-server-properties/quiltmc)
- [Forge promotions](https://files.minecraftforge.net/net/minecraftforge/forge/promotions_slim.json) · [NeoForge maven API](https://maven.neoforged.net/api/maven/versions/releases/net/neoforged/neoforge) · [rmcl NeoForge mapping source](https://docs.rs/crate/rmcl/0.4.0/source/src/net/neoforge.rs)
- [Modrinth API docs](https://docs.modrinth.com/api/) · [.mrpack format](https://docs.modrinth.com/modpacks/format)
- [CurseForge download-opt-out discussion](https://github.com/itzg/docker-minecraft-server/issues/2067) · [Aternos CurseForge API client](https://root.packagist.org/packages/aternos/curseforge-api) · [curseforge-to-modrinth-cli](https://github.com/Arimodu/curseforge-to-modrinth-cli)
- [Adoptium CI scripts / API](https://adoptium.net/installation/ci-scripts/) · [nitro_core Java download code](https://docs.rs/crate/nitro_core/0.32.1/source/src/net/java.rs)
