//! Per-server isolation (see `claude/plan/features/isolation-and-sandboxing.md`).
//!
//! Layers, each applied when the host supports it:
//! 1. **Separate user**: every server runs as its own UID/GID (needs root / `CAP_SETUID`).
//! 2. **Landlock filesystem rules**: read/write only its own directories, read-only system and
//!    Java directories, nothing else.
//! 3. **Landlock network rules** (kernel ABI ≥ 4): bind only its own ports, connect only to
//!    web ports (Mojang authentication), so it can't reach other servers' admin ports.
//! 4. **Resource limits**: open files; processes per user when it has its own user.

pub mod check;
#[cfg(target_os = "linux")]
mod linux;

use std::path::PathBuf;

use serde::Serialize;

use crate::instances::SandboxIds;

/// What to restrict for one process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxSpec {
    pub user: Option<SandboxIds>,
    pub read_write: Vec<PathBuf>,
    /// Read and execute.
    pub read_only: Vec<PathBuf>,
    pub bind_tcp: Vec<u16>,
    pub connect_tcp: Vec<u16>,
    pub limits: ResourceLimits,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResourceLimits {
    pub open_files: u64,
    /// Only applied when the process has its own user (the limit is per user).
    pub processes: u64,
}

impl Default for ResourceLimits {
    fn default() -> Self {
        Self {
            open_files: 8192,
            processes: 4096,
        }
    }
}

/// Outbound TCP ports every server may use: DNS over TCP, HTTP and HTTPS (Mojang login, skins).
pub const DEFAULT_CONNECT_PORTS: [u16; 3] = [53, 80, 443];

/// Files under `/etc` that name resolution and TLS read. They are often symlinks to places
/// outside `/etc` (`/run/systemd/resolve/…` with systemd-resolved, `/mnt/wsl/…` on WSL), and
/// Landlock checks the *resolved* path, so their targets must be allowed explicitly.
const LINKED_CONFIG_FILES: [&str; 8] = [
    "/etc/resolv.conf",
    "/etc/hosts",
    "/etc/nsswitch.conf",
    "/etc/host.conf",
    "/etc/gai.conf",
    "/etc/localtime",
    "/etc/ssl/certs/ca-certificates.crt",
    "/etc/ssl/certs/java/cacerts",
];

/// System paths a JVM needs to read (missing ones are skipped).
pub fn system_read_only_paths() -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = [
        "/usr",
        "/lib",
        "/lib64",
        "/lib32",
        "/bin",
        "/sbin",
        "/etc",
        "/proc",
        "/sys",
        "/dev/urandom",
        "/dev/random",
        "/dev/zero",
    ]
    .iter()
    .map(PathBuf::from)
    .filter(|path| path.exists())
    .collect();

    for file in LINKED_CONFIG_FILES {
        if let Ok(target) = std::fs::canonicalize(file)
            && !paths.iter().any(|allowed| target.starts_with(allowed))
        {
            paths.push(target);
        }
    }
    paths
}

/// System paths a process may write to.
pub fn system_read_write_paths() -> Vec<PathBuf> {
    ["/dev/null"]
        .iter()
        .map(PathBuf::from)
        .filter(|path| path.exists())
        .collect()
}

/// Which isolation layers this host can enforce.
// WHY: a report of independent yes/no capabilities; an enum would hide combinations.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct SandboxSupport {
    pub separate_users: bool,
    /// Kernel Landlock ABI version, if Landlock is available.
    pub landlock_abi: Option<u32>,
    pub filesystem: bool,
    pub network: bool,
    pub resource_limits: bool,
}

/// Summary for the UI badge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IsolationLevel {
    /// No isolation (Windows / macOS development).
    None,
    /// Only one of: separate user, filesystem rules.
    Basic,
    /// Separate user and filesystem rules.
    Good,
    /// Separate user, filesystem and network rules.
    Strong,
}

impl SandboxSupport {
    /// Probes the running host.
    pub fn detect() -> Self {
        #[cfg(target_os = "linux")]
        {
            let landlock_abi = linux::landlock_abi();
            Self {
                separate_users: nix::unistd::geteuid().is_root(),
                landlock_abi,
                filesystem: landlock_abi.is_some(),
                network: landlock_abi.is_some_and(|abi| abi >= 4),
                resource_limits: true,
            }
        }
        #[cfg(not(target_os = "linux"))]
        {
            Self::none()
        }
    }

    /// No isolation at all.
    pub fn none() -> Self {
        Self {
            separate_users: false,
            landlock_abi: None,
            filesystem: false,
            network: false,
            resource_limits: false,
        }
    }

    pub fn level(&self) -> IsolationLevel {
        match (self.separate_users, self.filesystem, self.network) {
            (true, true, true) => IsolationLevel::Strong,
            (true, true, false) => IsolationLevel::Good,
            (true, false, _) | (false, true, _) => IsolationLevel::Basic,
            (false, false, _) => IsolationLevel::None,
        }
    }
}

/// Configures `command` to run inside the sandbox, as far as `support` allows.
pub(crate) fn apply(
    command: &mut tokio::process::Command,
    spec: &SandboxSpec,
    support: &SandboxSupport,
) -> std::io::Result<()> {
    #[cfg(target_os = "linux")]
    {
        linux::apply(command, spec, support)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (command, spec, support);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_follow_the_layers() {
        let mut support = SandboxSupport::none();
        assert_eq!(support.level(), IsolationLevel::None);
        support.filesystem = true;
        assert_eq!(support.level(), IsolationLevel::Basic);
        support.separate_users = true;
        assert_eq!(support.level(), IsolationLevel::Good);
        support.network = true;
        assert_eq!(support.level(), IsolationLevel::Strong);
    }

    #[test]
    fn system_paths_exist() {
        assert!(system_read_only_paths().iter().all(|p| p.exists()));
    }
}
