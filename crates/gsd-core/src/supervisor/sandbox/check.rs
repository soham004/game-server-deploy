//! Isolation self-test: proves the sandbox works on *this* host.
//!
//! Two fake instances (`own`, `other`) are created in a scratch directory. A probe program runs
//! inside `own`'s sandbox and tries things a compromised server would try. Each check reports
//! whether the sandbox behaved as intended. The probe is the app binary itself
//! (`<binary> isolation-probe <action> <target>`), exiting 0 when the action succeeded.

use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use serde::Serialize;

use super::{
    HostSandbox, ResourceLimits, SandboxSpec, SandboxSupport,
    cgroup::{CgroupLimits, CgroupRequest, CgroupsUnavailable},
    system_read_only_paths, system_read_write_paths,
};
use crate::{
    instances::SandboxIds,
    supervisor::process::{self, ProcessSpec},
};

/// UID used for the probe (outside the range given to real instances).
const PROBE_IDS: SandboxIds = SandboxIds {
    uid: 30999,
    gid: 30999,
};

/// Control group used by the probe (instances use their ULIDs, which never look like this).
const PROBE_CGROUP: &str = "isolation-probe";

#[derive(Debug, Clone, Serialize)]
pub struct CheckResult {
    pub name: &'static str,
    /// The sandbox behaved as intended.
    pub passed: bool,
    /// The layer isn't available on this host, so the result is informational.
    pub enforced: bool,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct IsolationReport {
    pub support: SandboxSupport,
    pub level: super::IsolationLevel,
    pub checks: Vec<CheckResult>,
}

impl IsolationReport {
    /// Every enforced check passed.
    pub fn ok(&self) -> bool {
        self.checks
            .iter()
            .filter(|check| check.enforced)
            .all(|check| check.passed)
    }
}

/// Runs all checks. `probe` is the app binary; `scratch` an empty directory it may use.
pub async fn run(probe: &Path, scratch: &Path, host: &HostSandbox) -> std::io::Result<IsolationReport> {
    let support = host.support();
    let (own, other) = probe_folders(scratch, support)?;
    std::fs::write(other.join("secret.txt"), "another server's data")?;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let admin_port = listener.local_addr()?.port();

    let read_other = probe_action(
        probe,
        &own,
        host,
        "read",
        &other.join("secret.txt").display().to_string(),
    )
    .await;
    let write_other = probe_action(
        probe,
        &own,
        host,
        "write",
        &other.join("planted.txt").display().to_string(),
    )
    .await;
    let write_own = probe_action(
        probe,
        &own,
        host,
        "write",
        &own.join("ok.txt").display().to_string(),
    )
    .await;
    let connect = probe_action(probe, &own, host, "connect", &format!("127.0.0.1:{admin_port}")).await;
    let whoami = probe_output(probe, &own, host, "uid", "-").await;
    let cgroup = probe_cgroup(probe, &own, host).await;
    drop(listener);

    let fs_enforced = support.filesystem || support.separate_users;
    let checks = vec![
        CheckResult {
            name: "Cannot read another server's files",
            passed: !read_other,
            enforced: fs_enforced,
            detail: outcome(read_other, "read", "was blocked"),
        },
        CheckResult {
            name: "Cannot write outside its own folder",
            passed: !write_other,
            enforced: fs_enforced,
            detail: outcome(write_other, "wrote", "was blocked"),
        },
        CheckResult {
            name: "Can write its own folder",
            passed: write_own,
            enforced: true,
            detail: if write_own {
                "works".into()
            } else {
                "failed: the sandbox blocks a server's own folder on this filesystem".into()
            },
        },
        CheckResult {
            name: "Cannot reach another server's admin port",
            passed: !connect,
            enforced: support.network,
            detail: outcome(connect, "connected", "was blocked"),
        },
        CheckResult {
            name: "Runs as its own user",
            passed: whoami.as_deref() == Some(&PROBE_IDS.uid.to_string()),
            enforced: support.separate_users,
            detail: whoami.map_or_else(|| "unknown".into(), |uid| format!("runs as uid {uid}")),
        },
        CheckResult {
            name: "Runs under a hard memory limit",
            passed: cgroup.is_ok(),
            enforced: host.cgroups().is_some(),
            detail: match (&cgroup, host.cgroups_problem()) {
                (Ok(path), _) => format!("runs in control group {path}"),
                (Err(_), Some(reason)) => format!("unavailable: {reason}"),
                (Err(problem), None) => problem.0.clone(),
            },
        },
    ];
    Ok(IsolationReport {
        support,
        level: support.level(),
        checks,
    })
}

/// Creates `dir` (traversable by sandbox users) and an empty scratch directory inside it.
///
/// Checks should run where instances live: the data directory's filesystem and its parents'
/// permissions decide whether the sandbox works, not `/tmp`'s.
pub fn scratch_in(dir: &Path) -> std::io::Result<tempfile::TempDir> {
    std::fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o711))?;
    }
    tempfile::Builder::new().prefix("check-").tempdir_in(dir)
}

/// Whether a sandboxed server can use its own folder on the filesystem that holds `dir`.
///
/// Landlock identifies files by inode. Some filesystems hand out a fresh inode on every lookup:
/// WSL's Windows drives (`/mnt/c`, 9p), and some network and FUSE mounts. The rule granting a
/// server its own folder then never matches, and Java can't even open the server jar.
/// Returns `Ok(true)` without probing when there are no filesystem rules to test.
pub async fn own_folder_usable(probe: &Path, dir: &Path, support: SandboxSupport) -> std::io::Result<bool> {
    if !support.filesystem {
        return Ok(true);
    }
    let scratch = scratch_in(dir)?;
    let (own, _) = probe_folders(scratch.path(), support)?;
    let target = own.join("ok.txt").display().to_string();
    let host = HostSandbox::new(support, None).with_launcher(probe.to_owned());
    Ok(probe_action(probe, &own, &host, "write", &target).await)
}

/// Keeps `host`'s hard limits only if a sandboxed process (as its own user, when the app is
/// root) really ends up in its control group here; otherwise turns them off with the reason.
/// Better to find out now than to fail every server start later.
pub async fn verify_cgroups(probe: &Path, dir: &Path, host: HostSandbox) -> HostSandbox {
    if host.cgroups().is_none() {
        return host;
    }
    let result = async {
        let scratch = scratch_in(dir).map_err(|error| error.to_string())?;
        let (own, _) = probe_folders(scratch.path(), host.support()).map_err(|error| error.to_string())?;
        probe_cgroup(probe, &own, &host)
            .await
            .map_err(|problem| problem.0)
    }
    .await;
    match result {
        Ok(_) => host,
        Err(reason) => host.with_cgroups(Err(CgroupsUnavailable(format!(
            "A server process could not join its control group ({reason}), so hard memory limits are off."
        )))),
    }
}

/// `own/` (owned by the probe's user) and `other/` in `scratch`: two servers' folders.
fn probe_folders(scratch: &Path, support: SandboxSupport) -> std::io::Result<(PathBuf, PathBuf)> {
    let own = scratch.join("own");
    let other = scratch.join("other");
    std::fs::create_dir_all(&own)?;
    std::fs::create_dir_all(&other)?;
    prepare_ownership(&own, &other, support)?;
    Ok((own, other))
}

fn outcome(succeeded: bool, verb: &str, blocked: &str) -> String {
    if succeeded {
        format!("the probe {verb} it: NOT isolated")
    } else {
        blocked.to_owned()
    }
}

fn prepare_ownership(own: &Path, other: &Path, support: SandboxSupport) -> std::io::Result<()> {
    #[cfg(unix)]
    if support.separate_users {
        use std::os::unix::fs::PermissionsExt;
        std::os::unix::fs::chown(own, Some(PROBE_IDS.uid), Some(PROBE_IDS.gid))?;
        std::fs::set_permissions(own, std::fs::Permissions::from_mode(0o700))?;
        std::fs::set_permissions(other, std::fs::Permissions::from_mode(0o700))?;
        if let Some(scratch) = own.parent() {
            std::fs::set_permissions(scratch, std::fs::Permissions::from_mode(0o711))?;
        }
    }
    #[cfg(not(unix))]
    let _ = (own, other, support);
    Ok(())
}

fn probe_spec(probe: &Path, own: &Path, action: &str, target: &str) -> ProcessSpec {
    let mut read_only = system_read_only_paths();
    if let Some(dir) = probe.parent() {
        read_only.push(dir.to_owned());
    }
    let mut read_write = vec![own.to_owned()];
    read_write.extend(system_read_write_paths());
    ProcessSpec {
        program: probe.to_owned(),
        args: vec!["isolation-probe".into(), action.into(), target.into()],
        cwd: own.to_owned(),
        env: vec![("PATH".into(), "/usr/bin:/bin".into())],
        sandbox: Some(SandboxSpec {
            user: Some(PROBE_IDS),
            read_write,
            read_only,
            bind_tcp: vec![],
            connect_tcp: vec![443],
            limits: ResourceLimits::default(),
            cgroup: None,
        }),
    }
}

/// Whether the action succeeded inside the sandbox.
async fn probe_action(probe: &Path, own: &Path, host: &HostSandbox, action: &str, target: &str) -> bool {
    run_probe(probe_spec(probe, own, action, target), host)
        .await
        .is_ok_and(|(success, _)| success)
}

async fn probe_output(
    probe: &Path,
    own: &Path,
    host: &HostSandbox,
    action: &str,
    target: &str,
) -> Option<String> {
    run_probe(probe_spec(probe, own, action, target), host)
        .await
        .ok()
        .filter(|(success, _)| *success)
        .map(|(_, stdout)| stdout.trim().to_owned())
}

/// The control group a probe placed in [`PROBE_CGROUP`] reports for itself, if that is the one.
async fn probe_cgroup(probe: &Path, own: &Path, host: &HostSandbox) -> Result<String, CgroupsUnavailable> {
    let Some(tree) = host.cgroups() else {
        return Err(CgroupsUnavailable("not available on this host".into()));
    };
    let mut spec = probe_spec(probe, own, "cgroup", "-");
    if let Some(sandbox) = spec.sandbox.as_mut() {
        sandbox.cgroup = Some(CgroupRequest {
            name: PROBE_CGROUP.into(),
            limits: CgroupLimits {
                memory_max: 256 * 1024 * 1024,
                tasks_max: 64,
            },
        });
    }
    let result = run_probe(spec, host).await;
    tree.remove(PROBE_CGROUP);
    let (success, output) = result.map_err(|error| CgroupsUnavailable(error.to_string()))?;
    let path = output.trim();
    if !success {
        Err(CgroupsUnavailable("the probe failed".into()))
    } else if path.ends_with(&format!("/instances/{PROBE_CGROUP}")) {
        Ok(path.to_owned())
    } else {
        Err(CgroupsUnavailable(format!("the probe ran in {path} instead")))
    }
}

/// Spawns a probe and collects (exited successfully, stdout).
async fn run_probe(spec: ProcessSpec, host: &HostSandbox) -> std::io::Result<(bool, String)> {
    let mut spawned = process::spawn(&spec, host)?;
    drop(spawned.stdin); // the probe never reads stdin
    let mut stdout = String::new();
    let read = async {
        use tokio::io::AsyncReadExt;
        let _ = spawned.stdout.read_to_string(&mut stdout).await;
        spawned.child.wait().await
    };
    let status = tokio::time::timeout(Duration::from_secs(10), read)
        .await
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::TimedOut, "the probe did not finish"))??;
    Ok((status.success(), stdout))
}

/// The probe's side: perform one action and exit 0 on success, 1 on failure.
pub fn probe_main(action: &str, target: &str) -> i32 {
    let succeeded = match action {
        "read" => std::fs::read(target).is_ok(),
        "write" => std::fs::write(target, b"probe").is_ok(),
        "connect" => target
            .parse::<std::net::SocketAddr>()
            .is_ok_and(|addr| std::net::TcpStream::connect_timeout(&addr, Duration::from_secs(2)).is_ok()),
        "uid" => {
            #[cfg(unix)]
            println!("{}", nix::unistd::getuid());
            cfg!(unix)
        }
        // Prints the probe's cgroup v2 group.
        "cgroup" => std::fs::read_to_string("/proc/self/cgroup").is_ok_and(|text| {
            text.lines()
                .find_map(|line| line.strip_prefix("0::"))
                .map(|path| println!("{path}"))
                .is_some()
        }),
        _ => false,
    };
    i32::from(!succeeded)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[tokio::test]
    async fn without_filesystem_rules_there_is_nothing_to_test() {
        let dir = tempfile::tempdir().unwrap();
        // A probe that doesn't exist proves nothing was spawned.
        let usable = own_folder_usable(
            Path::new("/nonexistent/probe"),
            dir.path(),
            SandboxSupport::none(),
        )
        .await
        .unwrap();
        assert!(usable);
    }

    #[test]
    fn scratch_dirs_live_inside_the_given_folder() {
        let data = tempfile::tempdir().unwrap();
        let dir = data.path().join("sandbox-check");
        let scratch = scratch_in(&dir).unwrap();
        assert_eq!(scratch.path().parent(), Some(dir.as_path()));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o711, "sandbox users must be able to traverse it");
        }
        let path = scratch.path().to_owned();
        drop(scratch);
        assert!(!path.exists());
    }
}
