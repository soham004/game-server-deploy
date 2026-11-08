//! The trash. Deleted servers and the backups deleted with them go to `<data>/trash/`; files
//! deleted in the file manager and what a restore set aside go to `<instance>/trash/`. Every
//! entry is named after the moment it was deleted (`20261108-031452-<name>`), and [`purge`]
//! removes the entries older than the retention. An entry whose time can't be read is kept:
//! the trash never guesses before deleting.

use std::{io, path::Path};

use chrono::{DateTime, NaiveDateTime, Utc};

/// How the deletion time is written at the start of an entry's name.
const STAMP_FORMAT: &str = "%Y%m%d-%H%M%S";
const STAMP_LEN: usize = "20261108-031452".len();

/// The trash name of `name` deleted at `deleted_at`.
pub fn entry_name(name: &str, deleted_at: DateTime<Utc>) -> String {
    format!("{}-{name}", deleted_at.format(STAMP_FORMAT))
}

/// When an entry was deleted, from its name: `<stamp>-<name>`, or `<name>-<stamp>` (how
/// servers were named before M3).
pub fn deleted_at(entry: &str) -> Option<DateTime<Utc>> {
    let parse = |stamp: &str| {
        NaiveDateTime::parse_from_str(stamp, STAMP_FORMAT)
            .ok()
            .map(|time| time.and_utc())
    };
    let prefix = entry
        .get(..STAMP_LEN)
        .filter(|_| {
            entry
                .get(STAMP_LEN..)
                .is_some_and(|rest| rest.is_empty() || rest.starts_with('-'))
        })
        .and_then(parse);
    let suffix = || {
        let start = entry.len().checked_sub(STAMP_LEN)?;
        entry
            .get(start..)
            .filter(|_| start == 0 || entry.get(..start).is_some_and(|rest| rest.ends_with('-')))
            .and_then(parse)
    };
    prefix.or_else(suffix)
}

/// What a purge did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Purged {
    pub removed: u64,
    /// Not old enough yet.
    pub kept: u64,
    /// No deletion time in the name: left alone.
    pub unknown: u64,
    /// Could not be removed (logged).
    pub failed: u64,
}

impl std::ops::AddAssign for Purged {
    fn add_assign(&mut self, other: Self) {
        self.removed += other.removed;
        self.kept += other.kept;
        self.unknown += other.unknown;
        self.failed += other.failed;
    }
}

/// Removes the entries of the trash folder `dir` deleted before `cutoff`. A missing folder is an
/// empty trash. Links are removed, never followed.
pub fn purge(dir: &Path, cutoff: DateTime<Utc>) -> io::Result<Purged> {
    let mut purged = Purged::default();
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(purged),
        Err(error) => return Err(error),
    };
    for entry in entries {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        match deleted_at(&name) {
            None => purged.unknown += 1,
            Some(at) if at >= cutoff => purged.kept += 1,
            Some(_) => match remove(&entry) {
                Ok(()) => purged.removed += 1,
                Err(error) => {
                    tracing::warn!(%error, path = %entry.path().display(), "could not empty this from the trash");
                    purged.failed += 1;
                }
            },
        }
    }
    Ok(purged)
}

fn remove(entry: &std::fs::DirEntry) -> io::Result<()> {
    if entry.file_type()?.is_dir() {
        // Doesn't follow symlinks inside (std's remove_dir_all is symlink-safe).
        std::fs::remove_dir_all(entry.path())
    } else {
        std::fs::remove_file(entry.path())
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use chrono::TimeDelta;

    use super::*;

    fn at(text: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(text).unwrap().to_utc()
    }

    #[test]
    fn names_carry_the_deletion_time_in_either_place() {
        let deleted = at("2026-11-08T03:14:52Z");
        assert_eq!(entry_name("survival", deleted), "20261108-031452-survival");
        assert_eq!(deleted_at("20261108-031452-survival"), Some(deleted));
        assert_eq!(
            deleted_at("20261108-031452-2-notes.txt"),
            Some(deleted),
            "a second file of that second"
        );
        assert_eq!(
            deleted_at("survival-20261108-031452"),
            Some(deleted),
            "servers before M3"
        );
        assert_eq!(deleted_at("20261108-031452"), Some(deleted));
        for unknown in [
            "notes.txt",
            "20261308-031452-x",
            "x20261108-031452",
            "2026110-031452-x",
            "ünïcode-ß",
        ] {
            assert_eq!(deleted_at(unknown), None, "{unknown}");
        }
    }

    #[test]
    fn removes_only_what_is_old_enough_and_keeps_the_unknown() {
        let dir = tempfile::tempdir().unwrap();
        let now = at("2026-11-08T12:00:00Z");
        let old = entry_name("old-server", now - TimeDelta::days(8));
        std::fs::create_dir_all(dir.path().join(&old).join("server/world")).unwrap();
        std::fs::write(dir.path().join(&old).join("server/world/level.dat"), "x").unwrap();
        std::fs::write(
            dir.path().join(entry_name("old.txt", now - TimeDelta::days(30))),
            "x",
        )
        .unwrap();
        std::fs::write(
            dir.path()
                .join(entry_name("recent.txt", now - TimeDelta::days(1))),
            "x",
        )
        .unwrap();
        std::fs::write(dir.path().join("put-here-by-hand.txt"), "x").unwrap();

        let purged = purge(dir.path(), now - TimeDelta::days(7)).unwrap();

        assert_eq!(
            purged,
            Purged {
                removed: 2,
                kept: 1,
                unknown: 1,
                failed: 0
            }
        );
        let mut left: Vec<String> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        left.sort();
        assert_eq!(left, ["20261107-120000-recent.txt", "put-here-by-hand.txt"]);
        assert_eq!(
            purge(&dir.path().join("missing"), now).unwrap(),
            Purged::default()
        );
    }

    #[cfg(unix)]
    #[test]
    fn links_in_the_trash_are_removed_without_following_them() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("keep.txt"), "precious").unwrap();
        let now = Utc::now();
        let stale = now - TimeDelta::days(10);
        std::os::unix::fs::symlink(outside.path(), dir.path().join(entry_name("link", stale))).unwrap();
        let folder = dir.path().join(entry_name("folder", stale));
        std::fs::create_dir(&folder).unwrap();
        std::os::unix::fs::symlink(outside.path(), folder.join("inner-link")).unwrap();

        assert_eq!(purge(dir.path(), now - TimeDelta::days(7)).unwrap().removed, 2);
        assert_eq!(
            std::fs::read_to_string(outside.path().join("keep.txt")).unwrap(),
            "precious"
        );
    }
}
