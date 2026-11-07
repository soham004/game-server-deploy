//! What the sandbox can do on this host, decided once at startup and shared by everything that
//! runs untrusted code: game servers (supervisors) and loader installers (install jobs).

use std::sync::Arc;

use super::{
    SandboxSupport,
    cgroup::{CgroupTree, CgroupsUnavailable},
};

/// The host's isolation layers, hard resource limits when available, and any problem that makes
/// sandboxed processes unusable here (see D-026: e.g. a data directory on a filesystem Landlock
/// can't handle). Cheap to clone.
#[derive(Debug, Clone)]
pub struct HostSandbox {
    support: SandboxSupport,
    problem: Option<Arc<str>>,
    cgroups: Result<Arc<CgroupTree>, CgroupsUnavailable>,
}

/// Sandboxed processes can't run on this host; the message says why and how to fix it.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct SandboxUnusable(pub String);

impl HostSandbox {
    /// Without hard resource limits; add them with [`Self::with_cgroups`].
    pub fn new(support: SandboxSupport, problem: Option<String>) -> Self {
        Self {
            support,
            problem: problem.map(Arc::from),
            cgroups: Err(CgroupsUnavailable("Hard memory limits were not set up.".into())),
        }
    }

    /// No isolation and no problem (tests, non-Linux development).
    pub fn disabled() -> Self {
        Self::new(SandboxSupport::none(), None)
    }

    #[must_use]
    pub fn with_cgroups(mut self, cgroups: Result<CgroupTree, CgroupsUnavailable>) -> Self {
        self.cgroups = cgroups.map(Arc::new);
        self
    }

    pub fn support(&self) -> SandboxSupport {
        self.support
    }

    pub fn problem(&self) -> Option<&str> {
        self.problem.as_deref()
    }

    /// The control groups for hard memory and process limits, when this host allows them.
    pub fn cgroups(&self) -> Option<&CgroupTree> {
        self.cgroups.as_deref().ok()
    }

    /// Why hard memory limits are off, if they are.
    pub fn cgroups_problem(&self) -> Option<&str> {
        self.cgroups.as_ref().err().map(|reason| reason.0.as_str())
    }

    /// `Err` when sandboxed processes can't run here, so callers fail early with the explanation
    /// instead of starting a process that is doomed to fail cryptically.
    pub fn ensure_usable(&self) -> Result<(), SandboxUnusable> {
        match &self.problem {
            Some(problem) => Err(SandboxUnusable(problem.to_string())),
            None => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_problem_makes_the_host_unusable() {
        assert!(HostSandbox::disabled().ensure_usable().is_ok());
        let host = HostSandbox::new(SandboxSupport::none(), Some("move the data folder".into()));
        assert_eq!(host.problem(), Some("move the data folder"));
        assert_eq!(
            host.ensure_usable(),
            Err(SandboxUnusable("move the data folder".into()))
        );
    }

    #[test]
    fn hard_limits_are_off_until_set_up() {
        let host = HostSandbox::disabled();
        assert!(host.cgroups().is_none());
        assert!(host.cgroups_problem().is_some());
        let host = host.with_cgroups(Err(CgroupsUnavailable("read-only".into())));
        assert_eq!(host.cgroups_problem(), Some("read-only"));
    }
}
