//! Linux enforcement: setuid/setgid, Landlock and rlimits, applied between fork and exec.
//!
//! WHY `unsafe`: `pre_exec` runs code in the forked child, where only async-signal-safe
//! operations are allowed. Everything that allocates (opening path fds, building the Landlock
//! ruleset, reading current limits) happens in the parent; the child only makes syscalls.

#![allow(unsafe_code)]

use std::{
    io::{self, Write},
    path::{Path, PathBuf},
    sync::Mutex,
};

use landlock::{
    ABI, Access, AccessFs, AccessNet, CompatLevel, Compatible, NetPort, PathBeneath, PathFd, Ruleset,
    RulesetAttr, RulesetCreated, RulesetCreatedAttr,
};
use nix::sys::resource::{Resource, getrlimit, setrlimit};

use super::{SandboxSpec, SandboxSupport, cgroup::CgroupSlot};

/// The newest ABI we know how to use; older kernels get a best-effort subset.
const TARGET_ABI: ABI = ABI::V5;

/// The kernel's Landlock ABI version, or `None` when Landlock is unavailable or disabled.
pub(super) fn landlock_abi() -> Option<u32> {
    const LANDLOCK_CREATE_RULESET_VERSION: libc::c_uint = 1;
    // SAFETY: with a NULL attribute, size 0 and the VERSION flag, this syscall only returns the
    // ABI version (or an error); it reads and writes no memory.
    let version = unsafe {
        libc::syscall(
            libc::SYS_landlock_create_ruleset,
            std::ptr::null::<libc::c_void>(),
            0_usize,
            LANDLOCK_CREATE_RULESET_VERSION,
        )
    };
    u32::try_from(version).ok().filter(|v| *v > 0)
}

pub(super) fn apply(
    command: &mut tokio::process::Command,
    spec: &SandboxSpec,
    support: &SandboxSupport,
    cgroup: Option<&CgroupSlot>,
) -> io::Result<()> {
    let own_user = spec.user.filter(|_| support.separate_users);
    if let Some(user) = own_user {
        // std drops supplementary groups (setgroups) before setuid when running as root.
        command.uid(user.uid);
        command.gid(user.gid);
    }

    let limits = PreparedLimits::new(spec, own_user.is_some());
    let ruleset = if support.filesystem {
        Some(build_ruleset(spec, support.network).map_err(io::Error::other)?)
    } else {
        None
    };
    let ruleset = Mutex::new(ruleset);
    let cgroup_procs = cgroup.map(CgroupSlot::procs_file).transpose()?;

    // SAFETY: runs in the child after fork. It only calls write (joining the control group:
    // "0" means the writing process), setrlimit and the Landlock syscalls (`restrict_self` =
    // prctl(NO_NEW_PRIVS) + landlock_restrict_self), all async-signal-safe. The mutex is private
    // to this closure, so it can't be held by another thread at fork time.
    unsafe {
        command.pre_exec(move || {
            // First, so everything after (including exec) is already counted against the caps.
            if let Some(mut procs) = cgroup_procs.as_ref() {
                procs.write_all(b"0")?;
            }
            limits.apply()?;
            let prepared = ruleset
                .lock()
                .map_err(|_| io::Error::other("ruleset lock poisoned"))?
                .take();
            if let Some(ruleset) = prepared {
                ruleset.restrict_self().map_err(io::Error::other)?;
            }
            Ok(())
        });
    }
    Ok(())
}

fn build_ruleset(spec: &SandboxSpec, network: bool) -> Result<RulesetCreated, landlock::RulesetError> {
    let all = AccessFs::from_all(TARGET_ABI);
    let read = AccessFs::from_read(TARGET_ABI);
    let file_only = AccessFs::from_file(TARGET_ABI);

    let mut ruleset = Ruleset::default()
        .set_compatibility(CompatLevel::BestEffort)
        .handle_access(all)?;
    if network {
        ruleset = ruleset.handle_access(AccessNet::BindTcp | AccessNet::ConnectTcp)?;
    }
    let mut created = ruleset.create()?;

    for (paths, access) in [(&spec.read_only, read), (&spec.read_write, all)] {
        for path in existing(paths) {
            // Directory-only rights can't be granted on a file.
            let access = if path.is_dir() { access } else { access & file_only };
            let fd = match PathFd::new(path) {
                Ok(fd) => fd,
                Err(error) => {
                    tracing::warn!(path = %path.display(), %error, "skipping sandbox path");
                    continue;
                }
            };
            created = created.add_rule(PathBeneath::new(fd, access))?;
        }
    }
    if network {
        for port in &spec.bind_tcp {
            created = created.add_rule(NetPort::new(*port, AccessNet::BindTcp))?;
        }
        for port in &spec.connect_tcp {
            created = created.add_rule(NetPort::new(*port, AccessNet::ConnectTcp))?;
        }
    }
    Ok(created)
}

fn existing(paths: &[PathBuf]) -> impl Iterator<Item = &Path> {
    paths.iter().map(PathBuf::as_path).filter(|path| path.exists())
}

/// Limits computed in the parent, applied in the child.
#[derive(Debug, Clone, Copy)]
struct PreparedLimits {
    open_files: Option<(u64, u64)>,
    processes: Option<(u64, u64)>,
}

impl PreparedLimits {
    fn new(spec: &SandboxSpec, own_user: bool) -> Self {
        // Never raise above the current hard limit (that would need privileges).
        let capped = |resource: Resource, wanted: u64| {
            getrlimit(resource).ok().map(|(_, hard)| {
                let value = wanted.min(hard);
                (value, value)
            })
        };
        Self {
            open_files: capped(Resource::RLIMIT_NOFILE, spec.limits.open_files),
            // RLIMIT_NPROC counts every process of the user, so it's only meaningful (and safe)
            // when the server has a user of its own.
            processes: if own_user {
                capped(Resource::RLIMIT_NPROC, spec.limits.processes)
            } else {
                None
            },
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

#[cfg(test)]
mod tests {
    use std::process::Stdio;

    use super::*;
    use crate::supervisor::sandbox::{ResourceLimits, system_read_only_paths, system_read_write_paths};

    /// Runs `script` with `sh` inside a sandbox that may only write `allowed`.
    async fn run_sandboxed(allowed: &Path, script: &str) -> (bool, String) {
        let support = SandboxSupport::detect();
        let mut read_write = vec![allowed.to_owned()];
        read_write.extend(system_read_write_paths());
        let spec = SandboxSpec {
            user: None,
            read_write,
            read_only: system_read_only_paths(),
            bind_tcp: vec![],
            connect_tcp: vec![],
            limits: ResourceLimits::default(),
            cgroup: None,
        };
        let mut command = tokio::process::Command::new("/bin/sh");
        command
            .arg("-c")
            .arg(script)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        apply(&mut command, &spec, &support, None).unwrap();
        let output = command.output().await.unwrap();
        let text =
            String::from_utf8_lossy(&output.stdout).into_owned() + &String::from_utf8_lossy(&output.stderr);
        (output.status.success(), text)
    }

    #[tokio::test]
    async fn landlock_confines_the_filesystem() {
        if landlock_abi().is_none() {
            eprintln!("skipped: Landlock is not available on this kernel");
            return;
        }
        let root = tempfile::tempdir().unwrap();
        let own = root.path().join("own");
        let sibling = root.path().join("sibling");
        std::fs::create_dir_all(&own).unwrap();
        std::fs::create_dir_all(&sibling).unwrap();
        std::fs::write(sibling.join("secret.txt"), "top secret").unwrap();

        let (ok, _) = run_sandboxed(&own, &format!("echo hi > {}/mine.txt", own.display())).await;
        assert!(ok, "writing its own directory is allowed");
        assert!(own.join("mine.txt").exists());

        let (ok, output) = run_sandboxed(&own, &format!("cat {}/secret.txt", sibling.display())).await;
        assert!(!ok, "reading a sibling must fail, got: {output}");
        assert!(!output.contains("top secret"));

        let (ok, _) = run_sandboxed(&own, &format!("echo x > {}/planted.txt", sibling.display())).await;
        assert!(!ok);
        assert!(!sibling.join("planted.txt").exists());

        let (ok, _) = run_sandboxed(&own, "ls /usr/bin > /dev/null").await;
        assert!(ok, "system directories stay readable");
    }

    /// Regression: `/etc/resolv.conf` is usually a symlink out of `/etc` (systemd-resolved, WSL);
    /// without its target in the rules, every DNS lookup in the sandbox failed.
    #[tokio::test]
    async fn name_resolution_config_stays_readable() {
        if landlock_abi().is_none() {
            eprintln!("skipped: Landlock is not available on this kernel");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        for file in ["/etc/resolv.conf", "/etc/hosts", "/etc/nsswitch.conf"] {
            if std::path::Path::new(file).exists() {
                let (ok, output) = run_sandboxed(dir.path(), &format!("cat {file} > /dev/null")).await;
                assert!(ok, "{file} must be readable in the sandbox: {output}");
            }
        }
        let (ok, output) = run_sandboxed(dir.path(), "getent hosts localhost").await;
        assert!(ok, "resolving a name must work: {output}");
    }

    #[tokio::test]
    async fn open_file_limit_is_applied() {
        let dir = tempfile::tempdir().unwrap();
        let (ok, output) = run_sandboxed(dir.path(), "ulimit -n").await;
        assert!(ok);
        let limit: u64 = output.trim().parse().unwrap();
        assert!(limit <= ResourceLimits::default().open_files, "limit {limit}");
    }

    #[test]
    fn abi_probe_does_not_panic() {
        let _ = landlock_abi();
    }
}
