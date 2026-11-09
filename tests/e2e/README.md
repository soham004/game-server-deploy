# End-to-end tests

These run the real app against real Minecraft servers, a real browser, Docker or systemd. They are
slower than `cargo test` and need the internet. CI runs `docker.e2e.mjs` on every push, against the
image it builds; the others run by hand before a release, or after changing anything the API tests'
fake server can't stand in for: installers, the sandbox, file ownership, console parsing of real
output. Each skips itself (and says why) when what it needs is missing. No dependencies: Node 22+ only.

Build the app first: `cargo build -p gsd-app`. The scripts find it at `$GSD_BIN`, or
`$CARGO_TARGET_DIR/debug/gsd` (default `target/debug/gsd`).

| File | Needs | Checks |
|---|---|---|
| `loaders.e2e.mjs` | Linux | Installs every loader for 26.3 (real downloads and installers, sandboxed), starts it, sends `list`, stops it. Keeps the servers in `$E2E_DATA` (default `~/.cache/gsd-e2e`), so later runs only start them. `E2E_LOADERS="paper neoforge"` limits the run. |
| `content.e2e.mjs` | Linux, `loaders.e2e.mjs` run once | Real mods and plugins from live Modrinth and Hangar: Lithium and AppleSkin (with Fabric API) load in "E2E fabric", an older Lithium updates after a full backup and an untracked Fabric API is identified; Chunky enables in "E2E paper"; from Hangar, Chunky replaces the Modrinth copy, ViaBackwards brings ViaVersion, all enable, and an older Chunky updates; removed again afterwards. |
| `curseforge.e2e.mjs` | A key (`CURSEFORGE_API_KEY`, or in `.env`), Linux, `loaders.e2e.mjs` run once | The real CurseForge API, with your key: it is checked and saved (removed again at the end, never printed), a mod with its dependencies loads in "E2E fabric", an update is offered, a file whose index entry is gone is identified, a Bukkit plugin enables in "E2E paper", and a blocked file says where to get it, if one turns up. It picks projects from search results and prints them. |
| `modpack.e2e.mjs` | Linux | A real pack: the smallest Fabric `.mrpack` Modrinth has for 26.3 (`E2E_MC` to change it, `lib/small-modpack.mjs` picks it) is uploaded, the server and all its files installed, the server started, then deleted. |
| `version-change.e2e.mjs` | Linux, `loaders.e2e.mjs` run once | Moves "E2E neoforge" (`E2E_LOADER=forge` for Forge) to another build and back with the real installer: safety backup, old build folder removed, server starts on the new build. |
| `ui.e2e.mjs` + `cdp.mjs` | Chrome or Edge, an app from `serve.sh` | The web UI in a headless browser: sign-in, console, properties, files, settings, backups and restore, schedules, mods from Modrinth (install, update, turn off and on, remove), plugins from Hangar and Modrinth on "E2E paper" (source picker, ticking from both), CurseForge against a local stand-in (settings page, wrong key, install, a file to download by hand and upload), a real `.mrpack` uploaded and made into a server, phone width, clone, import and version change, users. Screenshots with `E2E_SCREENSHOTS=<folder>`. |
| `app-killed.e2e.mjs` | Linux | The app gets SIGKILL while a real vanilla server runs (makes its own, deletes it afterwards): the server saves its world and exits by itself, and starts again under a new app. Run it as root too, where the server has its own user. |
| `serve.sh` | Linux | Serves the app on `$E2E_DATA` (port 18739) for `ui.e2e.mjs`, with `lib/mock-curseforge.mjs` standing in for CurseForge (port 18740). |
| `docker.e2e.mjs` | Docker, an image (`E2E_IMAGE`, default `gsd:dev`) | The image as root with the compose file's capabilities: hard limits off with the reason; every file written into a server belongs to its own user (install, upload, extract, clone, import, restore, version change); root-only backups of a running server; delete with backups. |
| `cgroup-systemd.sh` + `cgroup.e2e.mjs` | root, systemd, cgroup v2 | The app as a systemd service with `Delegate=yes`: a real server runs in its own control group with the expected memory and task limits, as its own user, and the isolation check passes. |

```bash
# Linux (or WSL2, with the data folder on the Linux side)
cargo build -p gsd-app
node --test tests/e2e/loaders.e2e.mjs
node --test tests/e2e/version-change.e2e.mjs
node --test tests/e2e/content.e2e.mjs
node --test tests/e2e/curseforge.e2e.mjs      # key from CURSEFORGE_API_KEY, or else from .env
node --test tests/e2e/modpack.e2e.mjs
node --test tests/e2e/app-killed.e2e.mjs
sudo bash tests/e2e/cgroup-systemd.sh

# The browser suite: the app on Linux, the browser wherever Chrome or Edge is installed
bash tests/e2e/serve.sh &
E2E_URL=http://localhost:18739 E2E_SCREENSHOTS=/tmp/shots node --test tests/e2e/ui.e2e.mjs

# Docker
docker build -t gsd:dev . && node --test tests/e2e/docker.e2e.mjs
```

The servers sign in as `E2E_USER` / `E2E_PASSWORD` (default `admin` / `correct horse battery`); a fresh
data folder gets that owner on first use. Shared helpers: `lib/client.mjs` (the API client) and
`lib/app.mjs` (starts the binary).

On WSL2, a browser on Windows reaches the app in WSL at `localhost`. Keep `$E2E_DATA` on the Linux
filesystem: the sandbox's file rules don't work on `/mnt/c` and the like (D-026).
