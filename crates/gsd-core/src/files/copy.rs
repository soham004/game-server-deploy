//! Copying one server folder into another (cloning an instance).
//!
//! The source belongs to a game server that may be compromised, so the copy goes through
//! `cap-std` handles and skips symlinks and special files: nothing outside the source folder can
//! be pulled into the copy.

use std::{io, path::Path};

use cap_std::{
    ambient_authority,
    fs::{Dir, OpenOptions},
};
use tokio_util::sync::CancellationToken;

use super::regular::open_regular;

/// What a copy did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CopySummary {
    pub files: u64,
    pub bytes: u64,
    /// Symlinks and special files left out.
    pub skipped: u64,
}

/// Copies the contents of `from` into the existing, empty folder `to`, leaving out the top-level
/// entries named in `skip` (e.g. logs, worlds). Stops with `Interrupted` when `cancel` fires.
pub fn copy_server_dir(
    from: &Path,
    to: &Path,
    skip: &[String],
    cancel: &CancellationToken,
) -> io::Result<CopySummary> {
    let source = Dir::open_ambient_dir(from, ambient_authority())?;
    let target = Dir::open_ambient_dir(to, ambient_authority())?;
    let mut summary = CopySummary::default();
    copy_dir(&source, &target, skip, cancel, &mut summary)?;
    Ok(summary)
}

fn copy_dir(
    source: &Dir,
    target: &Dir,
    skip: &[String],
    cancel: &CancellationToken,
    summary: &mut CopySummary,
) -> io::Result<()> {
    for entry in source.entries()? {
        if cancel.is_cancelled() {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
        }
        let entry = entry?;
        let name = entry.file_name();
        if name.to_str().is_some_and(|n| skip.iter().any(|s| s == n)) {
            continue;
        }
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            target.create_dir(&name)?;
            let child_source = source.open_dir(&name)?;
            let child_target = target.open_dir(&name)?;
            copy_dir(&child_source, &child_target, &[], cancel, summary)?;
        } else if file_type.is_file() {
            let mut input = match open_regular(source, Path::new(&name)) {
                Ok(input) => input,
                // Swapped for a FIFO or device since it was listed: left out like one.
                Err(error) if error.kind() == io::ErrorKind::InvalidInput => {
                    summary.skipped += 1;
                    continue;
                }
                Err(error) => return Err(error),
            };
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            let mut output = target.open_with(&name, &options)?;
            summary.bytes += io::copy(&mut input, &mut output)?;
            summary.files += 1;
        } else {
            summary.skipped += 1; // symlinks, sockets, devices
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn copies_files_and_folders_except_skipped_and_links() {
        let dir = tempfile::tempdir().unwrap();
        let (from, to) = (dir.path().join("from"), dir.path().join("to"));
        for (path, contents) in [
            ("server.jar", "jar"),
            ("world/level.dat", "level"),
            ("world/region/r.0.0.mca", "region"),
            ("config/a.yml", "a"),
            ("logs/latest.log", "log"),
        ] {
            let path = from.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, contents).unwrap();
        }
        std::fs::create_dir_all(&to).unwrap();
        let outside = dir.path().join("secret.txt");
        std::fs::write(&outside, "secret").unwrap();
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&outside, from.join("config/link")).unwrap();
            std::os::unix::fs::symlink(dir.path(), from.join("escape")).unwrap();
        }

        let summary = copy_server_dir(
            &from,
            &to,
            &["logs".into(), "world".into()],
            &CancellationToken::new(),
        )
        .unwrap();
        assert_eq!(summary.files, 2);
        assert_eq!(summary.bytes, 4);
        assert_eq!(std::fs::read_to_string(to.join("config/a.yml")).unwrap(), "a");
        assert!(to.join("server.jar").is_file());
        assert!(!to.join("logs").exists() && !to.join("world").exists());
        assert!(
            std::fs::symlink_metadata(to.join("config/link")).is_err(),
            "links are not copied"
        );
        assert!(std::fs::symlink_metadata(to.join("escape")).is_err());
        #[cfg(unix)]
        assert_eq!(summary.skipped, 2);
    }

    #[test]
    fn stops_when_cancelled() {
        let dir = tempfile::tempdir().unwrap();
        let (from, to) = (dir.path().join("from"), dir.path().join("to"));
        std::fs::create_dir_all(&from).unwrap();
        std::fs::write(from.join("a"), "a").unwrap();
        std::fs::create_dir_all(&to).unwrap();
        let cancel = CancellationToken::new();
        cancel.cancel();
        let error = copy_server_dir(&from, &to, &[], &cancel).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::Interrupted);
    }
}
