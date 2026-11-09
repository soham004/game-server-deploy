//! What installing or updating a pack does to a server's files, worked out without touching them.
//!
//! The lock file records what the pack last installed, with each file's hash. A file whose hash no
//! longer matches was changed after that, by the user or by the server itself (mods rewrite their
//! configs), so an update keeps it and says so, unless told to replace such files. A file the lock
//! doesn't list isn't the pack's, and an update leaves it alone.
//!
//! The lock lives in the server's folder so a backup and its restore carry it along: after a restore
//! it still describes the files that are there. The server can edit it, which gains nothing: every
//! path in it goes through the file manager, and only reaches the server's own files.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

/// The lock's name in the server's folder. A pack can't bring its own.
pub const LOCK_FILE: &str = ".modpack-files.json";

/// What a pack last installed.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackLock {
    /// The version on its source; absent for an upload.
    pub version_id: Option<String>,
    /// As the pack names itself.
    pub version: Option<String>,
    /// Path in the server's folder, and SHA-1.
    pub files: BTreeMap<String, String>,
}

/// One file of the version being installed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NewFile<'a> {
    pub path: &'a str,
    /// Absent when the pack doesn't say, which counts as different from anything.
    pub sha1: Option<&'a str>,
}

/// A file as it is on the server now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Local {
    Missing,
    /// Turned off on the Mods tab (`.disabled`).
    TurnedOff,
    /// There, with this SHA-1.
    File(String),
}

/// Why an update left a file as it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum KeptReason {
    /// It differs from what the pack installed.
    Changed,
    /// The pack installed it, and it has since been removed.
    Removed,
    TurnedOff,
    /// The new version brings a file of this name, and the one there didn't come from the pack.
    NotFromPack,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Kept {
    pub path: String,
    pub reason: KeptReason,
}

/// What an install or update does.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct FileChanges {
    pub add: Vec<String>,
    pub replace: Vec<String>,
    pub remove: Vec<String>,
    pub kept: Vec<Kept>,
}

/// Every path whose state on the server matters for this comparison.
pub fn paths_involved<'a>(old: &'a BTreeMap<String, String>, new: &[NewFile<'a>]) -> BTreeSet<&'a str> {
    old.keys()
        .map(String::as_str)
        .chain(new.iter().map(|file| file.path))
        .collect()
}

/// Compares what the pack installed (`old`) with the new version, given what is there now.
/// `replace_changed` overrides what was changed or isn't the pack's, but never brings back a file
/// that was turned off: that is a choice about the server, not about a file's contents.
pub fn compare(
    old: &BTreeMap<String, String>,
    new: &[NewFile<'_>],
    local: impl Fn(&str) -> Local,
    replace_changed: bool,
) -> FileChanges {
    let mut changes = FileChanges::default();
    let keep = |changes: &mut FileChanges, path: &str, reason| {
        changes.kept.push(Kept {
            path: path.to_owned(),
            reason,
        });
    };
    for file in new {
        let installed = old.get(file.path).map(String::as_str);
        if installed.is_some() && installed == file.sha1 {
            continue; // the pack didn't change it, whatever has happened to it since
        }
        match local(file.path) {
            Local::File(hash) if Some(hash.as_str()) == file.sha1 => {} // already this version
            Local::File(hash) if Some(hash.as_str()) == installed => {
                changes.replace.push(file.path.to_owned());
            }
            Local::File(_) if replace_changed => changes.replace.push(file.path.to_owned()),
            Local::File(_) => {
                let reason = if installed.is_some() {
                    KeptReason::Changed
                } else {
                    KeptReason::NotFromPack
                };
                keep(&mut changes, file.path, reason);
            }
            Local::TurnedOff => keep(&mut changes, file.path, KeptReason::TurnedOff),
            Local::Missing if installed.is_none() || replace_changed => {
                changes.add.push(file.path.to_owned());
            }
            Local::Missing => keep(&mut changes, file.path, KeptReason::Removed),
        }
    }
    let wanted: BTreeSet<&str> = new.iter().map(|file| file.path).collect();
    for (path, installed) in old {
        if wanted.contains(path.as_str()) {
            continue;
        }
        match local(path) {
            Local::File(hash) if hash == *installed || replace_changed => {
                changes.remove.push(path.clone());
            }
            Local::File(_) => keep(&mut changes, path, KeptReason::Changed),
            Local::TurnedOff => keep(&mut changes, path, KeptReason::TurnedOff),
            Local::Missing => {}
        }
    }
    changes
}

#[cfg(test)]
mod tests {
    use super::*;

    fn old(files: &[(&str, &str)]) -> BTreeMap<String, String> {
        files
            .iter()
            .map(|(path, hash)| ((*path).to_owned(), (*hash).to_owned()))
            .collect()
    }

    fn new<'a>(files: &[(&'a str, &'a str)]) -> Vec<NewFile<'a>> {
        files
            .iter()
            .map(|(path, sha1)| NewFile {
                path,
                sha1: Some(sha1),
            })
            .collect()
    }

    fn on_server(files: &[(&str, &str)]) -> impl Fn(&str) -> Local {
        let files = old(files);
        move |path| match files.get(path) {
            Some(hash) if hash == "off" => Local::TurnedOff,
            Some(hash) => Local::File(hash.clone()),
            None => Local::Missing,
        }
    }

    fn kept(path: &str, reason: KeptReason) -> Kept {
        Kept {
            path: path.into(),
            reason,
        }
    }

    #[test]
    fn untouched_files_follow_the_pack() {
        let changes = compare(
            &old(&[("mods/same.jar", "a"), ("mods/old.jar", "b"), ("config/x", "c")]),
            &new(&[("mods/same.jar", "a"), ("mods/new.jar", "d"), ("config/x", "e")]),
            on_server(&[("mods/same.jar", "a"), ("mods/old.jar", "b"), ("config/x", "c")]),
            false,
        );
        assert_eq!(changes.add, ["mods/new.jar"]);
        assert_eq!(changes.replace, ["config/x"]);
        assert_eq!(changes.remove, ["mods/old.jar"]);
        assert_eq!(changes.kept, []);
    }

    #[test]
    fn what_was_changed_since_is_kept_and_named() {
        let changes = compare(
            &old(&[("config/x", "a"), ("config/gone", "b"), ("mods/removed.jar", "c")]),
            &new(&[("config/x", "new"), ("mods/removed.jar", "new")]),
            on_server(&[("config/x", "mine"), ("config/gone", "mine")]),
            false,
        );
        assert_eq!(
            changes,
            FileChanges {
                kept: vec![
                    kept("config/x", KeptReason::Changed),
                    kept("mods/removed.jar", KeptReason::Removed),
                    kept("config/gone", KeptReason::Changed),
                ],
                ..FileChanges::default()
            }
        );
    }

    #[test]
    fn replacing_changed_files_overrides_them_but_not_what_was_turned_off() {
        let changes = compare(
            &old(&[
                ("config/x", "a"),
                ("config/gone", "b"),
                ("mods/removed.jar", "c"),
                ("mods/off.jar", "d"),
            ]),
            &new(&[
                ("config/x", "new"),
                ("mods/removed.jar", "new"),
                ("mods/off.jar", "new"),
                ("mine", "new"),
            ]),
            on_server(&[
                ("config/x", "mine"),
                ("config/gone", "mine"),
                ("mods/off.jar", "off"),
                ("mine", "x"),
            ]),
            true,
        );
        assert_eq!(changes.replace, ["config/x", "mine"]);
        assert_eq!(changes.add, ["mods/removed.jar"]);
        assert_eq!(changes.remove, ["config/gone"]);
        assert_eq!(changes.kept, [kept("mods/off.jar", KeptReason::TurnedOff)]);
    }

    #[test]
    fn a_file_the_pack_did_not_install_is_not_overwritten() {
        let changes = compare(
            &old(&[]),
            &new(&[("mods/mine.jar", "pack"), ("mods/same.jar", "same")]),
            on_server(&[
                ("mods/mine.jar", "mine"),
                ("mods/same.jar", "same"),
                ("mods/other.jar", "x"),
            ]),
            false,
        );
        assert_eq!(
            changes,
            FileChanges {
                kept: vec![kept("mods/mine.jar", KeptReason::NotFromPack)],
                ..FileChanges::default()
            }
        );
    }

    #[test]
    fn a_file_with_no_hash_always_counts_as_new() {
        let file = NewFile {
            path: "mods/x.jar",
            sha1: None,
        };
        let changes = compare(
            &old(&[("mods/x.jar", "a")]),
            &[file],
            on_server(&[("mods/x.jar", "a")]),
            false,
        );
        assert_eq!(changes.replace, ["mods/x.jar"]);
    }

    #[test]
    fn an_install_from_nothing_adds_everything_and_may_replace_what_the_loader_seeded() {
        let changes = compare(
            &old(&[]),
            &new(&[("mods/a.jar", "a"), ("server.properties", "b")]),
            on_server(&[("server.properties", "seeded")]),
            true,
        );
        assert_eq!(changes.add, ["mods/a.jar"]);
        assert_eq!(changes.replace, ["server.properties"]);
    }
}
