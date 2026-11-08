//! Free disk space, checked before writing a backup or unpacking one, so a full disk fails the
//! job up front instead of leaving a server with half its files replaced.

use std::path::Path;

/// Headroom on top of the estimate: compression varies, and the disk must not end up full.
const MARGIN_PERCENT: u64 = 20;

/// Free bytes for unprivileged writes on the filesystem holding `path`; `None` where the
/// platform can't tell (the check is then skipped).
pub fn available_bytes(path: &Path) -> Option<u64> {
    #[cfg(unix)]
    {
        rustix::fs::statvfs(path)
            .ok()
            .map(|stats| stats.f_bavail.saturating_mul(stats.f_frsize))
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        None
    }
}

/// `estimate` plus the safety margin.
pub fn with_margin(estimate: u64) -> u64 {
    estimate.saturating_add(estimate / 100 * MARGIN_PERCENT)
}

/// Whether `needed` bytes fit; returns the free space when they don't.
pub fn shortfall(path: &Path, needed: u64) -> Option<u64> {
    available_bytes(path).filter(|available| *available < needed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_margin_is_a_fifth_and_never_overflows() {
        assert_eq!(with_margin(1000), 1200);
        assert_eq!(with_margin(u64::MAX), u64::MAX);
    }

    #[cfg(unix)]
    #[test]
    fn reports_free_space_and_shortfalls() {
        #![allow(clippy::unwrap_used)]
        let dir = tempfile::tempdir().unwrap();
        let free = available_bytes(dir.path()).unwrap();
        assert!(free > 0);
        assert_eq!(shortfall(dir.path(), 1), None);
        // Other processes write meanwhile, so only the presence of a shortfall is stable.
        assert!(shortfall(dir.path(), u64::MAX).is_some());
    }
}
