//! Linux enforcement: the user switch, Landlock, rlimits and the parent-death signal.
//!
//! These must apply to the server process alone, before Java starts, and the app has many threads
//! it can't restrict. So the app starts *itself* as a short-lived launcher ([`launch_command`]). The
//! launcher has one thread and no runtime ([`launch_if_asked`] runs first in `main`), applies
//! everything from ordinary safe code, then replaces itself with the server through `exec`. The
//! process keeps its pid, process group and pipes, so the app can't tell the difference afterwards.
//!
//! A launcher that fails prints why on stderr, which lands in the server's console, and exits
//! with [`LAUNCH_FAILED`]. Nothing runs unsandboxed in that case.

use std::{
    convert::Infallible,
    ffi::OsStr,
    io,
    os::unix::process::CommandExt,
    path::{Path, PathBuf},
};

use landlock::{
    ABI, Access, AccessFs, AccessNet, CompatLevel, Compatible, LandlockStatus, NetPort, PathBeneath, PathFd,
    Ruleset, RulesetAttr, RulesetCreated, RulesetCreatedAttr,
};
use nix::{
    errno::Errno,
    sys::{
        prctl,
        resource::{Resource, getrlimit, setrlimit},
        signal::Signal,
    },
    unistd::{Gid, Pid, Uid, getppid, setgid, setgroups, setuid},
};
use serde::{Deserialize, Serialize};

use super::{ResourceLimits, SandboxSupport, cgroup::CgroupSlot};
use crate::supervisor::process::ProcessSpec;

/// The newest ABI we know how to use; older kernels get a best-effort subset.
const TARGET_ABI: ABI = ABI::V5;

/// First argument that makes the app binary act as the launcher.
const LAUNCH_ARG: &str = "sandbox-launch";

/// The launcher's exit code when it could not start the server (126: "could not be run").
const LAUNCH_FAILED: i32 = 126;

/// The kernel's Landlock ABI version, or `None` when Landlock is unavailable or disabled.
///
/// The `landlock` crate only reports this after restricting the caller. So a thread of its own is
/// restricted, reads the answer and ends, and its restriction ends with it.
pub(super) fn landlock_abi() -> Option<u32> {
    let probe = std::thread::spawn(|| {
        Ruleset::default()
            .handle_access(AccessFs::Execute)?
            .create()?
            .restrict_self()
    });
    let status = probe.join().ok()?.ok()?;
    match status.landlock {
        LandlockStatus::Available {
            effective_abi,
            kernel_abi,
        } => u32::try_from(kernel_abi.unwrap_or(effective_abi as i32)).ok(),
        _ => None,
    }
}

/// Everything the launcher needs, passed as one JSON argument. It holds no secrets: the server's
/// environment travels as the launcher's own environment, not in here.
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
struct LaunchPlan {
    /// The app's pid, to notice that it died before the parent-death signal was set.
    parent: i32,
    program: PathBuf,
    args: Vec<String>,
    cwd: PathBuf,
    /// Absent for processes that run without a sandbox.
    sandbox: Option<SandboxPlan>,
}

/// A [`super::SandboxSpec`] narrowed to what this host can enforce.
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
struct SandboxPlan {
    /// Uid and gid to switch to.
    user: Option<(u32, u32)>,
    read_write: Vec<PathBuf>,
    read_only: Vec<PathBuf>,
    bind_tcp: Vec<u16>,
    connect_tcp: Vec<u16>,
    limits: ResourceLimits,
    filesystem_rules: bool,
    network_rules: bool,
    /// The `cgroup.procs` file of the control group to join.
    cgroup_procs: Option<PathBuf>,
}

/// A command that starts `spec` through the launcher, which is `launcher` (the app binary).
pub(crate) fn launch_command(
    launcher: &Path,
    spec: &ProcessSpec,
    support: &SandboxSupport,
    cgroup: Option<&CgroupSlot>,
) -> io::Result<tokio::process::Command> {
    // What the app can see for itself stays an error from starting, not a crash of the server.
    std::fs::metadata(&spec.cwd)?;
    if spec.program.components().count() > 1 {
        std::fs::metadata(&spec.program)?;
    }
    let plan = LaunchPlan {
        parent: Pid::this().as_raw(),
        program: spec.program.clone(),
        args: spec.args.clone(),
        cwd: spec.cwd.clone(),
        sandbox: spec.sandbox.as_ref().map(|sandbox| SandboxPlan {
            user: sandbox
                .user
                .filter(|_| support.separate_users)
                .map(|user| (user.uid, user.gid)),
            read_write: sandbox.read_write.clone(),
            read_only: sandbox.read_only.clone(),
            bind_tcp: sandbox.bind_tcp.clone(),
            connect_tcp: sandbox.connect_tcp.clone(),
            limits: sandbox.limits,
            filesystem_rules: support.filesystem,
            network_rules: support.network,
            cgroup_procs: cgroup.map(CgroupSlot::procs_path),
        }),
    };
    let mut command = tokio::process::Command::new(launcher);
    command
        .arg(LAUNCH_ARG)
        .arg(serde_json::to_string(&plan).map_err(io::Error::other)?);
    Ok(command)
}

/// When the app was started as the launcher, starts the server and never returns. Otherwise does
/// nothing. Call it first in `main`: Landlock and the signal setting apply to the calling thread,
/// and `exec` drops any others, so a runtime's threads would be pointless at best.
pub(super) fn launch_if_asked() {
    let mut args = std::env::args_os().skip(1);
    if args.next().as_deref() != Some(OsStr::new(LAUNCH_ARG)) {
        return;
    }
    let plan = args
        .next()
        .and_then(|plan| plan.into_string().ok())
        .ok_or_else(|| io::Error::other("no launch plan"))
        .and_then(|plan| serde_json::from_str(&plan).map_err(io::Error::other));
    // `launch` only comes back with an error: when it works, this process has become the server.
    let Err(error) = plan.and_then(|plan| launch(&plan));
    eprintln!("Could not start the server: {error}");
    std::process::exit(LAUNCH_FAILED);
}

/// Applies the plan and becomes the server. The order matters; each step says why it comes where it
/// does. Any failure stops here, so a server never runs with less than the plan asked for.
fn launch(plan: &LaunchPlan) -> io::Result<Infallible> {
    let sandbox = plan.sandbox.as_ref();

    // First, so everything after (including exec) is already counted against the caps.
    if let Some(procs) = sandbox.and_then(|sandbox| sandbox.cgroup_procs.as_ref()) {
        std::fs::write(procs, "0")?;
    }
    // While the app's rights still apply: the server's user may not be able to reach these paths.
    let ruleset = match sandbox {
        Some(sandbox) if sandbox.filesystem_rules => Some(build_ruleset(sandbox).map_err(io::Error::other)?),
        _ => None,
    };

    if let Some((uid, gid)) = sandbox.and_then(|sandbox| sandbox.user) {
        become_user(uid, gid)?;
    }
    // As the server's user, so a folder it can't enter is an error here and not a surprise later.
    std::env::set_current_dir(&plan.cwd)?;
    if let Some(sandbox) = sandbox {
        // After the user switch: limiting processes first could make the switch itself fail.
        PreparedLimits::new(sandbox).apply()?;
    }
    // After the user switch, which would reset it.
    stop_with_parent(plan.parent)?;
    // Last: it also limits what the steps above could do.
    if let Some(ruleset) = ruleset {
        ruleset.restrict_self().map_err(io::Error::other)?;
    }

    Err(std::process::Command::new(&plan.program).args(&plan.args).exec())
}

fn become_user(uid: u32, gid: u32) -> io::Result<()> {
    setgid(Gid::from_raw(gid))?;
    // Root's other groups must not stay with the server. Without the right to change them (EPERM)
    // this isn't root, and there are none to worry about.
    match setgroups(&[]) {
        Ok(()) | Err(Errno::EPERM) => {}
        Err(error) => return Err(error.into()),
    }
    setuid(Uid::from_raw(uid))?;
    Ok(())
}

/// Has the kernel send the process SIGTERM when the app dies, however it dies (a crash, the
/// out-of-memory killer, `kill -9`). Nothing else would tell it: Java ignores the broken pipes, keeps
/// running and holds `session.lock`, so no later start works.
///
/// SIGTERM rather than SIGKILL: Java's shutdown hook then saves the world. A server that hangs while
/// stopping stays up, with no app left to kill it, and the next start explains the locked world.
///
/// WHY this is safe here: the signal follows the *thread* that started the process, not the whole
/// app, so a server started on a thread that ends would be stopped for no reason. Servers are
/// started from tasks on tokio's worker threads, which last as long as the app. That stops being
/// true with `block_in_place`, which can end one, so don't use it in this app.
fn stop_with_parent(parent: i32) -> io::Result<()> {
    prctl::set_pdeathsig(Signal::SIGTERM)?;
    // The app may have died before the line above, when no signal would come any more.
    if getppid().as_raw() == parent {
        Ok(())
    } else {
        Err(io::Error::other("the app has gone"))
    }
}

fn build_ruleset(sandbox: &SandboxPlan) -> Result<RulesetCreated, landlock::RulesetError> {
    let all = AccessFs::from_all(TARGET_ABI);
    let read = AccessFs::from_read(TARGET_ABI);
    let file_only = AccessFs::from_file(TARGET_ABI);

    let mut ruleset = Ruleset::default()
        .set_compatibility(CompatLevel::BestEffort)
        .handle_access(all)?;
    if sandbox.network_rules {
        ruleset = ruleset.handle_access(AccessNet::BindTcp | AccessNet::ConnectTcp)?;
    }
    let mut created = ruleset.create()?;

    for (paths, access) in [(&sandbox.read_only, read), (&sandbox.read_write, all)] {
        for path in existing(paths) {
            // Directory-only rights can't be granted on a file.
            let access = if path.is_dir() { access } else { access & file_only };
            let fd = match PathFd::new(path) {
                Ok(fd) => fd,
                Err(error) => {
                    eprintln!("Skipping sandbox path {}: {error}", path.display());
                    continue;
                }
            };
            created = created.add_rule(PathBeneath::new(fd, access))?;
        }
    }
    if sandbox.network_rules {
        for port in &sandbox.bind_tcp {
            created = created.add_rule(NetPort::new(*port, AccessNet::BindTcp))?;
        }
        for port in &sandbox.connect_tcp {
            created = created.add_rule(NetPort::new(*port, AccessNet::ConnectTcp))?;
        }
    }
    Ok(created)
}

fn existing(paths: &[PathBuf]) -> impl Iterator<Item = &Path> {
    paths.iter().map(PathBuf::as_path).filter(|path| path.exists())
}

/// The limits to set, capped by what is allowed now.
#[derive(Debug, Clone, Copy)]
struct PreparedLimits {
    open_files: Option<(u64, u64)>,
    processes: Option<(u64, u64)>,
}

impl PreparedLimits {
    fn new(sandbox: &SandboxPlan) -> Self {
        // Never raise above the current hard limit (that would need privileges).
        let capped = |resource: Resource, wanted: u64| {
            getrlimit(resource).ok().map(|(_, hard)| {
                let value = wanted.min(hard);
                (value, value)
            })
        };
        Self {
            open_files: capped(Resource::RLIMIT_NOFILE, sandbox.limits.open_files),
            // RLIMIT_NPROC counts every process of the user, so it's only meaningful (and safe)
            // when the server has a user of its own.
            processes: sandbox
                .user
                .and_then(|_| capped(Resource::RLIMIT_NPROC, sandbox.limits.processes)),
        }
    }

    fn apply(self) -> io::Result<()> {
        if let Some((soft, hard)) = self.open_files {
            setrlimit(Resource::RLIMIT_NOFILE, soft, hard)?;
        }
        if let Some((soft, hard)) = self.processes {
            setrlimit(Resource::RLIMIT_NPROC, soft, hard)?;
        }
        Ok(())
    }
}

// The launcher itself is tested with the real binary in `gsd-app/tests/sandbox.rs`.
#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::{instances::SandboxIds, supervisor::sandbox::SandboxSpec};

    fn spec() -> ProcessSpec {
        ProcessSpec {
            program: "java".into(),
            args: vec!["-jar".into(), "server.jar".into()],
            cwd: std::env::temp_dir(),
            env: vec![("SECRET".into(), "not for the plan".into())],
            sandbox: Some(SandboxSpec {
                user: Some(SandboxIds {
                    uid: 20000,
                    gid: 20000,
                }),
                read_write: vec!["/srv/own".into()],
                read_only: vec!["/usr".into()],
                bind_tcp: vec![25565],
                connect_tcp: vec![443],
                limits: ResourceLimits::default(),
                cgroup: None,
            }),
        }
    }

    fn plan_for(support: &SandboxSupport) -> LaunchPlan {
        let command = launch_command(Path::new("/app"), &spec(), support, None).unwrap();
        let args: Vec<_> = command.as_std().get_args().collect();
        assert_eq!(args[0], LAUNCH_ARG);
        serde_json::from_str(args[1].to_str().unwrap()).unwrap()
    }

    #[test]
    fn the_plan_carries_the_process_but_not_its_environment() {
        let command = launch_command(Path::new("/app"), &spec(), &SandboxSupport::none(), None).unwrap();
        let text = format!("{:?}", command.as_std().get_args().collect::<Vec<_>>());
        assert!(!text.contains("not for the plan"), "{text}");

        let plan = plan_for(&SandboxSupport::none());
        assert_eq!(plan.program, Path::new("java"));
        assert_eq!(plan.args, ["-jar", "server.jar"]);
        assert_eq!(plan.parent, Pid::this().as_raw());
    }

    #[test]
    fn the_plan_asks_only_for_what_this_host_enforces() {
        let plan = plan_for(&SandboxSupport::none());
        let sandbox = plan.sandbox.unwrap();
        assert_eq!(sandbox.user, None, "no user switch without root");
        assert!(!sandbox.filesystem_rules && !sandbox.network_rules);

        let plan = plan_for(&SandboxSupport {
            separate_users: true,
            landlock_abi: Some(5),
            filesystem: true,
            network: true,
            resource_limits: true,
        });
        let sandbox = plan.sandbox.unwrap();
        assert_eq!(sandbox.user, Some((20000, 20000)));
        assert!(sandbox.filesystem_rules && sandbox.network_rules);
    }

    #[test]
    fn a_missing_program_or_folder_is_an_error_from_starting() {
        let support = SandboxSupport::none();
        let mut missing = spec();
        missing.program = "/nonexistent/java".into();
        assert!(launch_command(Path::new("/app"), &missing, &support, None).is_err());

        let mut missing = spec();
        missing.cwd = "/nonexistent/folder".into();
        assert!(launch_command(Path::new("/app"), &missing, &support, None).is_err());
    }

    /// The probe restricts a thread of its own; this thread must stay free to write anywhere.
    #[test]
    fn reading_the_version_leaves_the_caller_unrestricted() {
        let _ = landlock_abi();
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("still-allowed"), "yes").unwrap();
    }
}
