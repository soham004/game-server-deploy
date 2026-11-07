//! Hard resource caps per server through cgroup v2: a memory ceiling the whole JVM can't exceed
//! (heap, metaspace, native memory, anything it starts) and a cap on processes and threads.
//!
//! Layout, under the control group the app was started in:
//! ```text
//! <own>/                  delegated to the app (systemd `Delegate=yes`, or a container's private
//!                         and writable cgroup namespace)
//!   app/                  the app itself: cgroup v2 forbids processes in a group whose children
//!                         use controllers, so the app moves out of `<own>` first
//!   instances/            memory and pids controllers on
//!     <instance id>/      memory.max, memory.swap.max, pids.max; the server's processes
//! ```
//!
//! The app only builds this tree when `<own>` is clearly its own: writable, with the memory and
//! pids controllers, and holding no processes but the app (and a container's init that started
//! it). It never moves anyone else's processes. Otherwise ([`CgroupsUnavailable`]) servers still
//! run: `-Xmx` caps the heap and rlimits apply; only the hard ceiling is missing.

use std::{
    fs::{self, File, OpenOptions},
    io,
    path::{Path, PathBuf},
};

/// Where the unified (v2) hierarchy is mounted.
const CGROUP_MOUNT: &str = "/sys/fs/cgroup";
const CONTROLLERS: &str = "+memory +pids";
const MIB: u64 = 1024 * 1024;

/// Memory the JVM needs beyond its heap (metaspace, code cache, thread stacks, network buffers):
/// half the heap, and never less than this.
const MIN_JVM_OVERHEAD: u64 = 1024 * MIB;

/// Processes plus threads per server. Busy modded servers run a few hundred threads.
pub const DEFAULT_TASKS_MAX: u64 = 2048;

/// Hard caps for one server's control group.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CgroupLimits {
    /// `memory.max`, in bytes.
    pub memory_max: u64,
    /// `pids.max`: processes and threads.
    pub tasks_max: u64,
}

impl CgroupLimits {
    /// Limits for a JVM started with `-Xmx{heap_mb}M`: the heap plus room for the JVM itself.
    pub fn for_heap(heap_mb: u32) -> Self {
        let heap = u64::from(heap_mb) * MIB;
        Self {
            memory_max: heap + (heap / 2).max(MIN_JVM_OVERHEAD),
            tasks_max: DEFAULT_TASKS_MAX,
        }
    }

    pub fn memory_max_mb(&self) -> u64 {
        self.memory_max / MIB
    }
}

/// Which control group a process should run in, and its caps.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CgroupRequest {
    /// A single path component, unique per server (the instance id).
    pub name: String,
    pub limits: CgroupLimits,
}

/// Why hard limits are off on this host; the text is shown to admins.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct CgroupsUnavailable(pub String);

/// The `instances/` group this app manages.
#[derive(Debug)]
pub struct CgroupTree {
    instances: PathBuf,
}

/// The processes that may share the app's control group; anything else there means the group
/// isn't the app's to reorganise.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Membership {
    /// The app's own process.
    app: u32,
    /// A container init (tini) that started the app: PID 1 in a private cgroup namespace.
    init: Option<u32>,
}

impl Membership {
    fn current(own_path: &str) -> Self {
        #[cfg(unix)]
        let parent = std::os::unix::process::parent_id();
        #[cfg(not(unix))]
        let parent = 0;
        Self::new(std::process::id(), parent, own_path)
    }

    fn new(app: u32, parent: u32, own_path: &str) -> Self {
        // Only in a container's own namespace (`/`) is PID 1 an init that exists for the app.
        // On a host, PID 1 is systemd and must stay where it is.
        let init = (parent == 1 && own_path == "/").then_some(parent);
        Self { app, init }
    }

    fn allows(&self, pid: u32) -> bool {
        pid == self.app || Some(pid) == self.init
    }

    fn pids(&self) -> impl Iterator<Item = u32> {
        self.init.into_iter().chain([self.app])
    }
}

impl CgroupTree {
    /// Builds the tree under the app's own control group (moving the app into `app/`), or says
    /// why hard limits are unavailable. Call it once at startup, before any child process runs.
    pub fn detect() -> Result<Self, CgroupsUnavailable> {
        if !cfg!(target_os = "linux") {
            return Err(unavailable("Hard memory limits need Linux (cgroup v2)."));
        }
        let mount = Path::new(CGROUP_MOUNT);
        if !mount.join("cgroup.controllers").exists() {
            return Err(unavailable(
                "This host doesn't use cgroup v2, so hard memory limits are off.",
            ));
        }
        let text = fs::read_to_string("/proc/self/cgroup")
            .map_err(|error| unavailable(format!("Can't read the app's control group: {error}")))?;
        let own = parse_own_cgroup(&text)
            .ok_or_else(|| unavailable("The app isn't in a cgroup v2 control group."))?;
        let base = match own.trim_start_matches('/') {
            "" => mount.to_owned(), // the root of a container's own namespace
            relative => mount.join(relative),
        };
        Self::setup(&base, &Membership::current(own))
    }

    /// Builds the tree in `base`, which `membership` must own.
    fn setup(base: &Path, membership: &Membership) -> Result<Self, CgroupsUnavailable> {
        let shown = base.display();
        // The real root has no `cgroup.type`; the app shouldn't run there, and its rules differ.
        if !base.join("cgroup.type").exists() {
            return Err(unavailable(format!(
                "The app runs in the root control group ({shown}); run it as a service to get hard memory limits."
            )));
        }
        let controllers = fs::read_to_string(base.join("cgroup.controllers")).unwrap_or_default();
        let available: Vec<&str> = controllers.split_whitespace().collect();
        if !(available.contains(&"memory") && available.contains(&"pids")) {
            return Err(unavailable(format!(
                "The memory and pids controllers aren't delegated to the app's control group ({shown})."
            )));
        }
        if let Err(error) = OpenOptions::new()
            .write(true)
            .open(base.join("cgroup.subtree_control"))
        {
            let hint = if error.kind() == io::ErrorKind::ReadOnlyFilesystem {
                "Docker mounts it read-only by default."
            } else {
                "Run the app as root in a systemd service with Delegate=yes."
            };
            return Err(unavailable(format!(
                "The app's control group ({shown}) isn't writable ({error}). {hint}"
            )));
        }
        let others: Vec<u32> = read_pids(base)
            .map_err(|error| unavailable(format!("Can't list {shown}/cgroup.procs: {error}")))?
            .into_iter()
            .filter(|pid| !membership.allows(*pid))
            .collect();
        if !others.is_empty() {
            return Err(unavailable(format!(
                "Other processes share the app's control group ({shown}), so it isn't the app's to manage. \
                 Run the app as a systemd service with Delegate=yes."
            )));
        }

        let failed = |what: &str, error: io::Error| unavailable(format!("Can't {what} in {shown}: {error}"));
        let app = base.join("app");
        create_dir(&app).map_err(|e| failed("create app/", e))?;
        for pid in membership.pids() {
            fs::write(app.join("cgroup.procs"), pid.to_string())
                .map_err(|e| failed("move the app into app/", e))?;
        }
        fs::write(base.join("cgroup.subtree_control"), CONTROLLERS)
            .map_err(|e| failed("enable controllers", e))?;
        let instances = base.join("instances");
        create_dir(&instances).map_err(|e| failed("create instances/", e))?;
        fs::write(instances.join("cgroup.subtree_control"), CONTROLLERS)
            .map_err(|e| failed("enable controllers in instances/", e))?;
        Ok(Self { instances })
    }

    /// Creates (or reuses) the group `request.name` with `request.limits`, ready for a process
    /// to join. Processes left in it by an earlier run are killed first, so they can't eat into
    /// the new server's budget.
    pub fn prepare(&self, request: &CgroupRequest) -> io::Result<CgroupSlot> {
        let dir = self.instances.join(single_component(&request.name)?);
        create_dir(&dir)?;
        if populated(&dir) {
            write_if_present(&dir.join("cgroup.kill"), "1")?;
        }
        fs::write(dir.join("memory.max"), request.limits.memory_max.to_string())?;
        // Swapping makes a game server unplayable and drags the whole host down; a clean stop
        // with an explanation is better. Missing when the kernel doesn't account swap.
        write_if_present(&dir.join("memory.swap.max"), "0")?;
        // Out of memory stops the whole server, not one of its processes (a half-killed server
        // is broken anyway, and the diagnostic can say plainly what happened).
        write_if_present(&dir.join("memory.oom.group"), "1")?;
        fs::write(dir.join("pids.max"), request.limits.tasks_max.to_string())?;
        let procs = OpenOptions::new().write(true).open(dir.join("cgroup.procs"))?;
        Ok(CgroupSlot {
            oom_kills_before: oom_kills(&dir),
            dir,
            procs,
            limits: request.limits,
        })
    }

    /// Removes the group of a deleted server (best effort: it must be empty).
    pub fn remove(&self, name: &str) {
        if let Ok(name) = single_component(name) {
            let _ = fs::remove_dir(self.instances.join(name));
        }
    }
}

/// One server's prepared control group.
#[derive(Debug)]
pub struct CgroupSlot {
    dir: PathBuf,
    /// Opened by the app, written by the child before exec (`0` = "move me"). The kernel checks
    /// permissions with the opener's credentials, so this works after the child dropped root.
    procs: File,
    oom_kills_before: u64,
    limits: CgroupLimits,
}

impl CgroupSlot {
    /// A handle for the child process (closed on exec).
    pub(crate) fn procs_file(&self) -> io::Result<File> {
        self.procs.try_clone()
    }

    pub fn limits(&self) -> CgroupLimits {
        self.limits
    }

    /// Whether the kernel killed a process here for exceeding `memory.max` since [`CgroupTree::prepare`].
    pub fn memory_limit_hit(&self) -> bool {
        oom_kills(&self.dir) > self.oom_kills_before
    }

    #[cfg(test)]
    fn dir(&self) -> &Path {
        &self.dir
    }
}

/// The path of the process's cgroup v2 group, from `/proc/<pid>/cgroup` (`0::/some/path`).
fn parse_own_cgroup(text: &str) -> Option<&str> {
    text.lines().find_map(|line| line.strip_prefix("0::"))
}

fn read_pids(group: &Path) -> io::Result<Vec<u32>> {
    Ok(fs::read_to_string(group.join("cgroup.procs"))?
        .lines()
        .filter_map(|line| line.trim().parse().ok())
        .collect())
}

/// `oom_kill` from `memory.events` (0 if unreadable).
fn oom_kills(group: &Path) -> u64 {
    let events = fs::read_to_string(group.join("memory.events")).unwrap_or_default();
    events
        .lines()
        .find_map(|line| line.strip_prefix("oom_kill "))
        .and_then(|count| count.trim().parse().ok())
        .unwrap_or(0)
}

fn populated(group: &Path) -> bool {
    read_pids(group).is_ok_and(|pids| !pids.is_empty())
}

fn create_dir(path: &Path) -> io::Result<()> {
    match fs::create_dir(path) {
        Err(error) if error.kind() != io::ErrorKind::AlreadyExists => Err(error),
        _ => Ok(()),
    }
}

/// Writes a control file that older kernels or some configurations don't have.
fn write_if_present(path: &Path, value: &str) -> io::Result<()> {
    if !path.exists() {
        return Ok(());
    }
    fs::write(path, value)
}

fn single_component(name: &str) -> io::Result<&str> {
    let valid = !name.is_empty() && name != "." && name != ".." && !name.contains(['/', '\0']);
    if valid {
        Ok(name)
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("invalid control group name {name:?}"),
        ))
    }
}

fn unavailable(message: impl Into<String>) -> CgroupsUnavailable {
    CgroupsUnavailable(message.into())
}

#[cfg(test)]
pub(crate) mod tests;
