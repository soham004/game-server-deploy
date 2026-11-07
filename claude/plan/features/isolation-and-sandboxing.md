# Feature: Server Isolation and Sandboxing

## Requirement (user, 2026-11-07)
> Admins must be able to run multiple servers at the same time, but a rogue vulnerability (malicious mod or plugin, Log4Shell-style
> RCE) in one server must not affect the other servers. Preference: **same container if possible**; container-per-server is
> acceptable if required.

## Threat model
- **Attacker**: code running inside one Minecraft server process (a malicious mod, an exploited plugin, a hostile JVM arg).
- **Must not be able to**: read or write another instance's files (worlds, configs, RCON/MCSMP secrets), read app data
  (`gsd.sqlite`, sessions, the CurseForge key, backups of other instances), signal or ptrace other servers or the app, reach
  other instances' admin ports (RCON/MCSMP), or starve the others of all memory, PIDs or file descriptors.
- **Accepted (v1)**: shared kernel (container escape-class kernel bugs), shared network namespace (it can see that other game
  ports exist; game ports are public anyway), CPU contention beyond what limits give us.
- Also in scope: **privileged UI users**. Editing JVM args, uploading mods or setting a custom Java path is equivalent to
  code execution *inside that instance's sandbox*. The sandbox is what makes delegating those permissions safe. A custom
  Java path outside managed runtimes is owner-only.

## Decision: layered isolation in one container (Linux), with a backend seam for per-server containers later
```
                  app process (root in container, minimal caps; or root on host via systemd)
                    │ spawn via ProcessBackend::Native + Sandbox
   ┌────────────────┼──────────────────────────┐
   ▼                ▼                          ▼
 uid 20001         uid 20002                 uid 20003         ← one dedicated UID/GID per instance
 Landlock: rw /data/instances/a/server       ← filesystem allow-list per process
           ro /data/runtimes/java/temurin-25, /usr, /lib, /etc/ssl, /tmp/gsd-a (private tmp)
 Landlock net (ABI ≥ 4): connect only to 80/443 (+ allow-listed ports); bind only own game/query/rcon/mgmt ports
 rlimits: NOFILE, NPROC, CORE=0 · no_new_privs · cgroup v2 (if writable): memory.max, pids.max, cpu.weight
```

### Layer 1: unique UID/GID per instance (mandatory on Linux)
- At instance creation, allocate a UID/GID from a configured range (default `20000–29999`), stored in `instance.toml`
  (`[sandbox] uid = 20001`). No `/etc/passwd` entry needed (the JVM tolerates numeric-only users; set `HOME=/data/instances/<slug>/home`
  and `-Duser.home=` to that path).
- Ownership: `instances/<slug>/server` is `chown uid:gid` and `chmod 0700` (dirs) / `0600` (files written by us). The app
  (root with `CAP_CHOWN`/`CAP_DAC_OVERRIDE`/`CAP_FOWNER`) can still manage the files; the other instances cannot.
- The process is spawned with `Command::uid()/gid()` + `setgroups([])` (via `pre_exec`), so it cannot signal or ptrace other
  servers (different UIDs), cannot read the app's `0600 root` files (`gsd.sqlite`, `config.toml`, `backups/`), and cannot read
  other instances' dirs.
- Every file the app writes into an instance (uploads, installs, extracted mods, restored backups) is chowned to the instance
  UID by a single helper, `files::ownership::assign(instance, path)`. Easy to forget, so it is centralised and tested.

### Layer 2: Landlock (best effort, kernel ≥ 5.13; network rules need ABI v4 / kernel ≥ 6.7)
- Applied in the child between fork and exec (`pre_exec`) with the `landlock` crate: `set_no_new_privs()` + a ruleset
  allowing only the paths above. Uses the **best-effort** compatibility mode: on older kernels, apply what's supported and
  report the achieved level.
- Docker's default seccomp profile **allows** the Landlock syscalls (moby commit 2022), so this works inside the container
  without extra privileges. If Landlock is disabled at boot (`EOPNOTSUPP`), fall back to Layers 1 and 3 and show a warning badge.
- **Filesystem compatibility (D-026):** Landlock rules don't match on filesystems that hand out a new inode per lookup
  (WSL's 9p Windows drives, some network/FUSE mounts): a server is then blocked from its own folder. At startup the app
  runs one sandboxed probe in `<data>/sandbox-check/`; if it can't write its own folder there, every start is refused
  (`503 sandbox_unusable`) with an explanation, logged at startup and shown on the dashboard. `isolation-check` also runs
  in the data dir, not `/tmp`.
- Network (ABI ≥ 4): `bind` only on the instance's own allocated ports (game, query, rcon, management); `connect` only to 80
  and 443 (Mojang auth, mod update checks) plus per-instance extra ports the admin allow-lists (e.g. a database for a plugin,
  or a Velocity proxy). This blocks one server from reaching another's loopback RCON/MCSMP ports, in addition to their random secrets.
- The UI shows an **isolation level** per instance: `Strong` (UID + Landlock fs + net), `Good` (UID + Landlock fs), `Basic` (UID only),
  `None` (Windows/macOS dev mode).

### Layer 3: resource limits
- Memory: the JVM heap via `-Xmx` (always) **plus** a hard cap when cgroups are available.
- **cgroups v2** (built, D-034; `supervisor/sandbox/cgroup.rs`). At startup `CgroupTree::detect` looks at the app's own
  group (`/proc/self/cgroup`) and uses it only if it is clearly the app's: writable, `memory` and `pids` delegated, and no
  processes in it but the app (plus a container init such as tini, only in a private cgroup namespace). It never moves
  anyone else's processes. Layout: `<own>/app/` (the app moves here; v2 forbids processes next to child groups),
  `<own>/instances/<instance id>/` with `memory.max` = heap + max(heap/2, 1 GiB), `memory.swap.max` = 0,
  `memory.oom.group` = 1, `pids.max` = 2048. Not per-instance settings yet; `cpu.weight` is not used.
  - host install under systemd with `Delegate=yes` in our unit file → on (verified with `systemd-run -p Delegate=yes`);
  - Docker: `/sys/fs/cgroup` is read-only by default → off, with the reason in `GET /system` and the startup log;
  - a shared group (a shell, a desktop session, `init.scope`) → off.
  A sandboxed probe proves at startup that a child joins its group after dropping root (the kernel checks the opener's
  credentials on `cgroup.procs`, Linux ≥ 5.16 or backports); if not, limits turn off instead of every start failing.
  The child joins in `pre_exec` by writing `0` to a `cgroup.procs` fd the app opened. After an exit, a new `oom_kill` in
  `memory.events` becomes the `memory_limit` diagnostic. Installers don't use the server's group (preparing it kills
  leftovers).
- rlimits for every child: `RLIMIT_NOFILE` (default 8192), `RLIMIT_NPROC` (per UID, so it only limits that instance's
  threads; default 4096; JVMs are thread-heavy), `RLIMIT_CORE=0`.

### Layer 4: secrets and app-side hygiene
- RCON passwords and MCSMP secrets are random per instance and bound to 127.0.0.1. Combined with Landlock net, other instances can't use them.
- Child environment is minimal (`PATH`, `HOME`, `LANG`, `TZ`, `JAVA_HOME`). No app env vars leak (no API keys).
- `/proc`: other processes are visible (no PID namespace without `CAP_SYS_ADMIN`), but their environments and memory are
  unreadable across UIDs. Never pass secrets on command lines (they would be visible in `/proc/*/cmdline`).
- Backups are owned by root, `0600`, outside instance dirs.

## Process backend seam (for container-per-server later)
```rust
#[async_trait]
pub trait ProcessBackend: Send + Sync {
    async fn spawn(&self, spec: &LaunchSpec, sandbox: &SandboxSpec) -> Result<Box<dyn ServerProcess>, SpawnError>;
    fn isolation_level(&self) -> IsolationLevel;
}
pub trait ServerProcess: Send {
    fn stdin(&mut self) -> &mut (dyn AsyncWrite + Unpin + Send);
    fn stdout(&mut self) -> ...; fn stderr(&mut self) -> ...;
    async fn wait(&mut self) -> ExitInfo;
    async fn kill(&mut self) -> io::Result<()>;
    fn pid(&self) -> Option<u32>;
}
```
- `NativeBackend` (v1): Linux sandbox (Layers 1–4); Windows/macOS = plain process (Job Object on Windows) for development and
  single-user home use, clearly labelled.
- `DockerBackend` (P2, optional): one container per instance via the Docker API (`bollard` crate), mounting only the instance
  dir + runtime, with `--memory`, `--pids-limit`, a separate network, and stdin attached. Requires mounting the Docker socket into the
  app container (which is itself root-equivalent on the host), so it is opt-in and documented as such.
- The supervisor only talks to `ServerProcess`, so the console, backups and the scheduler don't care which backend runs the server.

## Implementation notes
- All sandbox setup lives in `gsd-core/supervisor/sandbox/` (`linux.rs`, `fallback.rs`), behind `#[cfg(target_os = "linux")]`.
  `pre_exec` closures must be async-signal-safe: precompute everything (paths as `CString`, the ruleset) **before** fork, and
  only do syscalls in the closure.
- The `landlock` crate's ruleset is created before fork and `restrict_self()` is called in `pre_exec`.
- Installers (Forge/NeoForge/Quilt `java -jar installer`) also run sandboxed as the instance UID (they execute downloaded code).
- Self-test endpoint and page: "Isolation check" spawns a tiny probe (a `java` program, or a shell probe) as an instance UID and tries to
  read a sibling dir, connect to a sibling's RCON port, and write outside its root. It reports pass/fail for each check.

## To verify
- [ ] Landlock ABI available on common hosts (Debian 12 = 6.1 → ABI 3, no net rules; Ubuntu 24.04 = 6.8 → ABI 4+).
- [ ] The Docker default seccomp profile still allows `landlock_*` (moby commit af819bf6).
- [ ] The JVM runs fine with a numeric UID that has no passwd entry (`user.name` = "?" can break some mods; may need
      `-Duser.name=mc-<slug>`, or writing `/etc/passwd` entries at container start).
- [ ] Container caps needed: `SETUID`, `SETGID`, `CHOWN`, `DAC_OVERRIDE`, `FOWNER`, `KILL` (all in Docker's default set). Drop the rest.

## Sources
- [Docker seccomp allows Landlock (moby commit)](https://git.causa-arcana.com/kotovalexarian-likes-github/moby--moby/commit/af819bf623a8d414289b24f9817c6317cf3f96d5) · [landlock_create_ruleset(2)](https://man.archlinux.org/man/landlock_create_ruleset.2.en)
- [Docker read-only cgroupfs in private cgroupns (gist)](https://Gist.github.com/pinkeen/bba0a6790fec96d6c8de84bd824ad933) · [KEP-5474 writable cgroups](https://www.kubernetes.dev/resources/keps/5474/) · [systemd-devel on cgroups in containers](https://lists.freedesktop.org/archives/systemd-devel/2022-May/047977.html)
