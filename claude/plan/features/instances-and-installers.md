# Feature: Instances, Version Picker and Installers

## Goal
Create, import, clone, change the version of, and delete server instances, with a Prism-style picker. See
`research/03-minecraft-ecosystem-apis.md` for every endpoint.

## Provider abstraction (gsd-minecraft/providers)
```rust
#[async_trait]
pub trait LoaderProvider: Send + Sync {
    fn kind(&self) -> LoaderKind;
    /// MC versions this loader supports (for disabling combos in the picker).
    async fn supported_game_versions(&self) -> Result<Vec<McVersionId>, ProviderError>;
    /// Loader versions for one MC version, newest first, with stable/recommended flags.
    async fn loader_versions(&self, mc: &McVersion) -> Result<Vec<LoaderVersion>, ProviderError>;
    /// Steps to install; executed by the install job.
    fn install_plan(&self, req: &InstallRequest) -> InstallPlan;
}
pub struct LoaderVersion { id: String, released: Option<DateTime<Utc>>, stable: bool, recommended: bool }
```
- `ProviderRegistry` holds one provider per `LoaderKind`. Responses are cached in `cache/meta/` with expiry, plus ETag
  where supported. Offline → serve stale cache with a "stale" flag.
- `InstallPlan` is **data**, a list of steps, so it is testable and shown in the job log:
  ```rust
  enum InstallStep {
      Download { url, dest, hash: Option<Hash> },          // content-addressed cache first
      RunJava  { java_major, args, cwd, label },           // e.g. forge installer --installServer
      WriteFile { path, contents },
      SetLaunch(LaunchSpec),                               // jar or args files
  }
  ```

## Per-loader install summary
| Loader | Steps | LaunchSpec |
|---|---|---|
| Vanilla | download server.jar (sha1) | `Jar("server.jar")` |
| Paper / Folia | Fill v3 builds → newest STABLE → download (sha256) | `Jar("server.jar")` |
| Purpur | API → download (md5/sha?) | `Jar` |
| Fabric | download launcher jar from meta | `Jar("fabric-server-launch.jar")` (first run fetches vanilla) |
| Quilt | download quilt-installer → `install server <mc> <loader> --download-server --install-dir=.` | `Jar("quilt-server-launch.jar")` |
| Forge ≥1.17 | download installer → `--installServer .` → delete installer + `run.*` | `ArgsFiles{ user_jvm_args.txt, libraries/…/{win,unix}_args.txt }` |
| Forge ≤1.16 | installer → produces `forge-*.jar` | `Jar(…)` |
| NeoForge | installer → `--install-server .` (verify flag) | `ArgsFiles` |

- The installer runs with the **correct Java** for that MC version (the installer itself may need a newer Java).
- Before writing anything we check: free disk, the target dir is empty (or importing), the port is free.
- `eula.txt` is written only from recorded consent.
- Defaults written into a new `server.properties`: `server-port` (first free from 25565), `motd` = instance name,
  `enable-rcon` + loopback port + random password (if the setting is on), `management-server-*` for ≥1.21.9 (see the console doc).
  **Don't write a full properties file**: let the server generate it on first start, then patch our keys. (Alternative:
  pre-seed only the keys we need. The server keeps unknown and missing keys and fills in defaults.)

## Version picker data flow
1. UI loads `/catalog/minecraft/versions` (all types) once and filters client-side (Releases/Snapshots/Old beta/Old alpha + search).
2. Selecting an MC version loads `/catalog/loaders?mc=X` to enable or disable the loader radios.
3. Selecting a loader loads `/catalog/loaders/{loader}/versions?mc=X`, defaulting the selection to the recommended
   (⭐) or else newest stable.
4. Columns: Version · Released · Type (matching the screenshot). Unstable builds are greyed with a "beta" tag.

## Import
- **Existing folder**: detect the loader by files (`fabric-server-launch.jar`, `libraries/net/neoforged`, `libraries/net/minecraftforge`,
  `paper-*.jar` / `cache/mojang_*.jar`, `quilt-server-launch.jar`, `server.jar` + version.json inside the bundler). Ask the user to confirm the MC version.
  Option: **copy** into `instances/` (safe) or **adopt in place** (instance.toml stores an external `server_root`). Default is copy.
- **Zip / server pack**: extract into a new instance, then the same detection.
- **`.mrpack` / CurseForge pack**: see `mods-and-modpacks.md`.
- After import: hash `mods/*.jar` → identify via Modrinth/CurseForge → write `.pw.toml` index (background job).

## Change version (upgrade or downgrade)
1. Warn: upgrades convert worlds irreversibly; downgrades are **unsupported** and may corrupt worlds (red warning, typed confirmation).
2. Mandatory backup (`pre_version_change`, full scope).
3. Stop the server, run the new install plan into the same root (remove the old loader libraries carefully: only paths owned
   by the old loader), update instance.toml.
4. For modded servers: run a mod compatibility check against the new MC version and list mods with no compatible
   version (offer to update or disable them).

## Clone / delete
- Clone: copy the folder (excluding `logs/`, `crash-reports/`, optionally the world), new id/slug/port, reset RCON/MCSMP secrets.
- Delete: typed confirmation of the instance name. Offer "keep backups". Moves the folder to `<data>/trash/` first and purges after 7 days (setting).
