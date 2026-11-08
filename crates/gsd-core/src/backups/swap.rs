//! Replacing a server's files with a backup's, once the backup is unpacked into a staging folder
//! beside the server folder (the same filesystem, so every move is a cheap, atomic rename).
//!
//! 1. Current top-level entries that the restore replaces move to `old/`.
//! 2. The staged entries move into the server folder.
//!
//! If a move fails, the moves done so far are undone and the server folder is as it was. On
//! success the caller deletes `old/`.

use std::{
    ffi::OsString,
    fs, io,
    path::{Path, PathBuf},
};

/// What a swap moved.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SwapSummary {
    /// Entries of the server folder that went to `old/`.
    pub replaced: usize,
    /// Entries from the backup that went into the server folder.
    pub restored: usize,
}

/// Moves the entries of `staged` into `server`. A current entry moves to the new folder `old`
/// first when the backup has an entry of the same name or `replaces(name)` says so.
pub fn swap_in(
    server: &Path,
    staged: &Path,
    old: &Path,
    replaces: impl Fn(&str) -> bool,
) -> io::Result<SwapSummary> {
    fs::create_dir(old)?;
    let staged_names = names(staged)?;
    let mut moves = Moves::default();
    let result = (|| {
        for name in names(server)? {
            if replaces(&name.to_string_lossy()) || staged_names.contains(&name) {
                moves.rename(server.join(&name), old.join(&name))?;
            }
        }
        let moved_out = moves.done.len();
        for name in &staged_names {
            moves.rename(staged.join(name), server.join(name))?;
        }
        Ok(SwapSummary {
            replaced: moved_out,
            restored: staged_names.len(),
        })
    })();
    if result.is_err() {
        moves.undo();
    }
    result
}

/// Renames done so far, so they can be undone in reverse order.
#[derive(Debug, Default)]
struct Moves {
    done: Vec<(PathBuf, PathBuf)>,
}

impl Moves {
    fn rename(&mut self, from: PathBuf, to: PathBuf) -> io::Result<()> {
        fs::rename(&from, &to)?;
        self.done.push((from, to));
        Ok(())
    }

    fn undo(self) {
        for (from, to) in self.done.into_iter().rev() {
            if let Err(error) = fs::rename(&to, &from) {
                tracing::error!(%error, from = %to.display(), to = %from.display(), "could not undo a restore step");
            }
        }
    }
}

fn names(dir: &Path) -> io::Result<Vec<OsString>> {
    let mut names = Vec::new();
    for entry in fs::read_dir(dir)? {
        names.push(entry?.file_name());
    }
    names.sort();
    Ok(names)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn write(root: &Path, files: &[(&str, &str)]) {
        for (path, contents) in files {
            fs::create_dir_all(root.join(path).parent().unwrap()).unwrap();
            fs::write(root.join(path), contents).unwrap();
        }
    }

    fn read(path: &Path) -> Option<String> {
        fs::read_to_string(path).ok()
    }

    /// `server/` with a world, config, libraries and logs; `staged/` with an older world and config.
    fn fixture() -> (tempfile::TempDir, PathBuf, PathBuf, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let (server, staged, old) = (
            dir.path().join("server"),
            dir.path().join("staged"),
            dir.path().join("old"),
        );
        write(
            &server,
            &[
                ("world/level.dat", "new world"),
                ("world/new-region.mca", "new"),
                ("plugins/added-later.jar", "plugin"),
                ("server.properties", "new config"),
                ("libraries/lib.jar", "lib"),
                ("logs/latest.log", "log"),
            ],
        );
        write(
            &staged,
            &[
                ("world/level.dat", "old world"),
                ("server.properties", "old config"),
            ],
        );
        (dir, server, staged, old)
    }

    #[test]
    fn a_full_restore_replaces_everything_but_the_kept_entries() {
        let (_dir, server, staged, old) = fixture();
        let kept = ["libraries", "logs"];

        let summary = swap_in(&server, &staged, &old, |name| !kept.contains(&name)).unwrap();

        assert_eq!(
            read(&server.join("world/level.dat")).as_deref(),
            Some("old world")
        );
        assert_eq!(
            read(&server.join("world/new-region.mca")),
            None,
            "the whole folder is replaced"
        );
        assert_eq!(
            read(&server.join("server.properties")).as_deref(),
            Some("old config")
        );
        assert_eq!(
            read(&server.join("plugins/added-later.jar")),
            None,
            "not in the backup"
        );
        assert_eq!(read(&server.join("libraries/lib.jar")).as_deref(), Some("lib"));
        assert_eq!(read(&server.join("logs/latest.log")).as_deref(), Some("log"));
        assert_eq!(
            read(&old.join("plugins/added-later.jar")).as_deref(),
            Some("plugin")
        );
        assert_eq!(
            summary,
            SwapSummary {
                replaced: 3,
                restored: 2
            }
        );
    }

    #[test]
    fn a_world_restore_replaces_only_the_worlds() {
        let (_dir, server, staged, old) = fixture();
        fs::remove_file(staged.join("server.properties")).unwrap();
        let worlds = ["world", "world_nether", "world_the_end"];

        swap_in(&server, &staged, &old, |name| worlds.contains(&name)).unwrap();

        assert_eq!(
            read(&server.join("world/level.dat")).as_deref(),
            Some("old world")
        );
        assert_eq!(
            read(&server.join("server.properties")).as_deref(),
            Some("new config")
        );
        assert_eq!(
            read(&server.join("plugins/added-later.jar")).as_deref(),
            Some("plugin")
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_failed_move_puts_everything_back() {
        use std::os::unix::fs::PermissionsExt;

        let (_dir, server, staged, old) = fixture();
        // A staged entry that can't be moved: its folder is read-only. (Root ignores modes.)
        if nix_is_root() {
            return;
        }
        fs::set_permissions(&staged, fs::Permissions::from_mode(0o555)).unwrap();

        let error = swap_in(&server, &staged, &old, |_| true).unwrap_err();
        fs::set_permissions(&staged, fs::Permissions::from_mode(0o755)).unwrap();

        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        assert_eq!(
            read(&server.join("world/level.dat")).as_deref(),
            Some("new world")
        );
        assert_eq!(read(&server.join("libraries/lib.jar")).as_deref(), Some("lib"));
        assert_eq!(
            read(&staged.join("world/level.dat")).as_deref(),
            Some("old world")
        );
        assert_eq!(fs::read_dir(&old).unwrap().count(), 0);
    }

    #[cfg(unix)]
    fn nix_is_root() -> bool {
        nix::unistd::geteuid().is_root()
    }
}
