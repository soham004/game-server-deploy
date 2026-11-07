# Open Questions

Ask the user, or verify during implementation. Move answered items into `decisions.md`.

## Answered by the user (2026-11-07) → see decisions D-014 … D-020
- ~~Product name / port~~ → build-time configurable, declared once (`gsd-brand`). The real name is still TBD; placeholders until then.
- ~~Primary OS~~ → Linux first, Docker image.
- ~~Remote access~~ → nginx reverse proxy, documented.
- ~~Multi-user~~ → RBAC in v1.
- ~~CurseForge key~~ → user pastes their own.
- ~~Plugins vs mods~~ → equal priority.
- ~~Desktop wrapper~~ → browser-only.
- ~~Docker model~~ → same container with per-server isolation; container-per-server acceptable later.

## Still open for the user
- None. The last three were answered on 2026-11-07 (D-021 … D-023). The real product name is deferred: placeholders stay until the user picks one.

## To verify technically (before writing the code that depends on them)
- [x] NeoForge installer server flag: both exist; `--install-server` used (D-028).
- [x] Forge maven-metadata version format for 26.x: `26.3-66.1.5` (D-028).
- [x] Purpur API v2 endpoints and checksum fields: MD5 only (D-028).
- [x] Quilt meta v3 loader endpoint shape: `[{loader:{version,…},launcherMeta}]`, unsorted (D-028).
- [ ] Fill v3: is there a `builds/latest` route, and the exact `channel` values.
- [ ] Modrinth search facet syntax + `POST /version_files/update` body against the live API.
- [ ] CurseForge class ids (`GET /categories?gameId=432&classesOnly=true`).
- [ ] The data generator command on 26.x still `-DbundlerMainClass=net.minecraft.data.Main --reports`.
- [x] `server.properties` defaults for 26.3: `white-list=true`, no `pvp` key (D-029).
- [ ] MCSMP notification method names via `rpc.discover` on a real 26.3 server.
- [ ] Generational ZGC flag status on Java 25.
- [ ] Sandbox: Landlock ABI on target hosts (Debian 12 kernel 6.1 = no net rules; Ubuntu 24.04 = yes). Verified on the dev
      machine: Docker's default seccomp allows `landlock_*` (isolation level Strong in the container), the JVM runs with a
      numeric UID without a passwd entry, and Docker's cgroupfs is read-only (hard limits report themselves off, D-034).
- [ ] Hard limits in Docker: document an opt-in (private cgroup namespace with a writable cgroupfs) once someone needs it;
      the code already accepts a container init (tini) sharing the namespace root.
- [ ] `sysinfo` process-tree RSS accuracy (or read cgroup `memory.current` when cgroups are available).

## Environment (dev machine, checked 2026-11-07)
- **Build and test in WSL2**: Ubuntu 26.04 LTS, kernel `6.18.33.2-microsoft-standard-WSL2`, **rustc/cargo 1.99.1**.
  The kernel has `CONFIG_SECURITY_LANDLOCK=y` with `landlock` first in `CONFIG_LSM`, so Landlock fs **and** net rules (ABI ≥ 4) are available,
  and the full sandbox can be tested locally. cgroup v2 is mounted (`cgroup2fs`).
- Run cargo from WSL against the repo at `/mnt/e/Projects/Rust/game-server-deploy`. If builds on `/mnt/e` are slow, set `CARGO_TARGET_DIR`
  to a Linux-native path (e.g. `~/.cache/gsd-target`).
- Docker Desktop's CLI is reachable from WSL (`/mnt/c/Program Files/Docker/…/docker`), for building and testing the image.
- WSL also has a `java` at `/usr/bin/java` (version not checked). Not required, since the app manages its own runtimes.
- Windows host: no Rust toolchain; only Java 8 (Oracle) on PATH. Windows-native builds are secondary (D-014).
