//! [`Root`]: blocking file operations inside one server directory.
//!
//! Every path goes through a `cap_std::fs::Dir` opened on the server directory, which resolves
//! it beneath that directory and refuses to follow a symlink out of it, without races (Linux:
//! `openat2` with `RESOLVE_BENEATH`). The game server can plant symlinks and swap folders while we
//! work; the worst it gets is an error. Callers run these on a blocking thread (`service.rs`).

use std::{
    io::{self, Write},
    path::PathBuf,
    time::SystemTime,
};

use cap_std::{
    ambient_authority,
    fs::{Dir, OpenOptions},
};
use chrono::{DateTime, Utc};
use serde::Serialize;
use sha2::{Digest, Sha256};

use super::{FilesError, SafeRelPath};
use crate::instances::{InstanceLayout, SandboxIds, layout::can_change_ownership};

/// Largest file the text editor opens or saves.
pub const MAX_TEXT_BYTES: u64 = 5 * 1024 * 1024;

/// One entry of a folder listing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Entry {
    pub name: String,
    pub path: SafeRelPath,
    pub kind: EntryKind,
    /// Bytes, for files.
    pub size: Option<u64>,
    pub modified: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryKind {
    File,
    Directory,
    Symlink,
    Other,
}

/// A text file opened for editing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TextFile {
    pub path: SafeRelPath,
    pub content: String,
    /// Send back when saving: the save is refused if the file changed meanwhile.
    pub revision: String,
    pub size: u64,
    pub modified: Option<DateTime<Utc>>,
}

#[derive(Debug)]
pub(crate) struct Root {
    pub(crate) dir: Dir,
    /// Deleted files go here: the instance's `trash/`, outside the server's reach.
    trash: PathBuf,
    ids: SandboxIds,
}

impl Root {
    pub(crate) fn open(layout: &InstanceLayout, ids: SandboxIds) -> io::Result<Self> {
        Ok(Self {
            dir: Dir::open_ambient_dir(layout.server_dir(), ambient_authority())?,
            trash: layout.root().join("trash"),
            ids,
        })
    }

    pub(crate) fn list(&self, path: &SafeRelPath) -> Result<Vec<Entry>, FilesError> {
        let io = |e| FilesError::io(path, e);
        let read_dir = if path.is_root() {
            self.dir.entries()
        } else {
            self.dir.read_dir(path.as_path())
        }
        .map_err(io)?;
        let mut entries = Vec::new();
        for entry in read_dir {
            let entry = entry.map_err(io)?;
            let Ok(name) = entry.file_name().into_string() else {
                continue; // not UTF-8: can't be addressed through the API
            };
            let Ok(child) = path.join(&name) else { continue };
            let file_type = entry.file_type().map_err(io)?;
            let metadata = entry.metadata().ok();
            let kind = if file_type.is_symlink() {
                EntryKind::Symlink
            } else if file_type.is_dir() {
                EntryKind::Directory
            } else if file_type.is_file() {
                EntryKind::File
            } else {
                EntryKind::Other
            };
            entries.push(Entry {
                name,
                path: child,
                kind,
                size: metadata
                    .as_ref()
                    .filter(|_| kind == EntryKind::File)
                    .map(cap_std::fs::Metadata::len),
                modified: metadata
                    .and_then(|m| m.modified().ok())
                    .map(|t| to_utc(t.into_std())),
            });
        }
        entries.sort_by(|a, b| {
            (a.kind != EntryKind::Directory)
                .cmp(&(b.kind != EntryKind::Directory))
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
        Ok(entries)
    }

    pub(crate) fn read_text(&self, path: &SafeRelPath) -> Result<TextFile, FilesError> {
        let metadata = self
            .dir
            .metadata(path.as_path())
            .map_err(|e| FilesError::io(path, e))?;
        if metadata.is_dir() {
            return Err(FilesError::IsADirectory(path.clone()));
        }
        if metadata.len() > MAX_TEXT_BYTES {
            return Err(FilesError::TooLarge {
                path: path.clone(),
                limit: MAX_TEXT_BYTES,
            });
        }
        let bytes = self
            .dir
            .read(path.as_path())
            .map_err(|e| FilesError::io(path, e))?;
        let modified = metadata.modified().ok().map(|t| to_utc(t.into_std()));
        text_file(path, bytes, modified)
    }

    /// Saves `content`. `expected_revision`: the revision the editor opened (`None` creates a
    /// new file and refuses to replace an existing one). Atomic: temporary file, then rename.
    pub(crate) fn write_text(
        &self,
        path: &SafeRelPath,
        content: &str,
        expected_revision: Option<&str>,
    ) -> Result<TextFile, FilesError> {
        let name = path
            .file_name()
            .ok_or_else(|| FilesError::InvalidOperation("choose a file name".into()))?;
        if content.len() as u64 > MAX_TEXT_BYTES {
            return Err(FilesError::TooLarge {
                path: path.clone(),
                limit: MAX_TEXT_BYTES,
            });
        }
        let current = match self.dir.read(path.as_path()) {
            Ok(bytes) => Some(bytes),
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(error) => return Err(FilesError::io(path, error)),
        };
        match (expected_revision, &current) {
            (Some(expected), Some(bytes)) if revision(bytes) != expected => {
                return Err(FilesError::Conflict(path.clone()));
            }
            (Some(_), None) => return Err(FilesError::Conflict(path.clone())),
            (None, Some(_)) => return Err(FilesError::AlreadyExists(path.clone())),
            _ => {}
        }
        let temp = temp_sibling(path, name)?;
        self.dir
            .write(temp.as_path(), content.as_bytes())
            .map_err(|e| FilesError::io(path, e))?;
        let replaced = self.own(&temp).and_then(|()| {
            self.dir
                .rename(temp.as_path(), &self.dir, path.as_path())
                .map_err(|e| FilesError::io(path, e))
        });
        if let Err(error) = replaced {
            let _ = self.dir.remove_file(temp.as_path());
            return Err(error);
        }
        self.read_text(path)
    }

    pub(crate) fn create_dir(&self, path: &SafeRelPath) -> Result<(), FilesError> {
        if path.is_root() {
            return Err(FilesError::InvalidOperation("choose a folder name".into()));
        }
        self.dir
            .create_dir(path.as_path())
            .map_err(|e| FilesError::io(path, e))?;
        self.own(path)
    }

    /// Moves or renames within the server folder; never replaces an existing entry.
    pub(crate) fn rename(&self, from: &SafeRelPath, to: &SafeRelPath) -> Result<(), FilesError> {
        self.check_transfer(from, to)?;
        self.dir
            .rename(from.as_path(), &self.dir, to.as_path())
            .map_err(|e| FilesError::io(from, e))
    }

    /// Copies a file or a folder (recursively; symlinks inside are skipped).
    pub(crate) fn copy(&self, from: &SafeRelPath, to: &SafeRelPath) -> Result<(), FilesError> {
        self.check_transfer(from, to)?;
        let metadata = self
            .dir
            .symlink_metadata(from.as_path())
            .map_err(|e| FilesError::io(from, e))?;
        if metadata.is_symlink() {
            return Err(FilesError::InvalidOperation(format!(
                "{from} is a link; copy what it points to instead"
            )));
        }
        if metadata.is_dir() {
            self.copy_dir(from, to)
        } else {
            self.copy_file(from, to)
        }
    }

    /// Moves entries to the instance's trash (restorable by hand; emptied separately).
    pub(crate) fn delete(&self, paths: &[SafeRelPath]) -> Result<(), FilesError> {
        let trash = self
            .trash_dir()
            .map_err(|e| FilesError::io(&SafeRelPath::root(), e))?;
        let stamp = Utc::now().format("%Y%m%d-%H%M%S");
        for path in paths {
            let name = path.file_name().ok_or_else(|| {
                FilesError::InvalidOperation("the server folder itself can't be deleted".into())
            })?;
            self.dir
                .symlink_metadata(path.as_path())
                .map_err(|e| FilesError::io(path, e))?;
            let mut target = format!("{stamp}-{name}");
            for n in 1.. {
                if !trash.exists(&target) {
                    break;
                }
                target = format!("{stamp}-{n}-{name}");
            }
            self.dir
                .rename(path.as_path(), &trash, &target)
                .map_err(|e| FilesError::io(path, e))?;
        }
        Ok(())
    }

    /// The listing entry for one path.
    pub(crate) fn entry(&self, path: &SafeRelPath) -> Result<Entry, FilesError> {
        let parent = path.parent().unwrap_or_default();
        self.list(&parent)?
            .into_iter()
            .find(|entry| &entry.path == path)
            .ok_or_else(|| FilesError::NotFound(path.clone()))
    }

    /// Whether `path` exists (without following a final symlink).
    pub(crate) fn exists(&self, path: &SafeRelPath) -> bool {
        self.dir.symlink_metadata(path.as_path()).is_ok()
    }

    /// Gives `path` (one entry, not following symlinks) to the instance's user. No-op without root.
    pub(crate) fn own(&self, path: &SafeRelPath) -> Result<(), FilesError> {
        if !can_change_ownership() {
            return Ok(());
        }
        let Some(name) = path.file_name() else {
            return Ok(());
        };
        let parent = path.parent().unwrap_or_default();
        let parent_dir = self.open_dir(&parent)?;
        chown_entry(&parent_dir, name, self.ids).map_err(|e| FilesError::io(path, e))
    }

    pub(crate) fn open_dir(&self, path: &SafeRelPath) -> Result<Dir, FilesError> {
        if path.is_root() {
            self.dir.try_clone().map_err(|e| FilesError::io(path, e))
        } else {
            self.dir
                .open_dir(path.as_path())
                .map_err(|e| FilesError::io(path, e))
        }
    }

    fn check_transfer(&self, from: &SafeRelPath, to: &SafeRelPath) -> Result<(), FilesError> {
        if from.is_root() || to.is_root() {
            return Err(FilesError::InvalidOperation(
                "the server folder itself can't be moved".into(),
            ));
        }
        if to.starts_with(from) {
            return Err(FilesError::InvalidOperation(format!(
                "{from} can't be moved or copied into itself"
            )));
        }
        if !self.exists(from) {
            return Err(FilesError::NotFound(from.clone()));
        }
        if self.exists(to) {
            return Err(FilesError::AlreadyExists(to.clone()));
        }
        Ok(())
    }

    fn copy_file(&self, from: &SafeRelPath, to: &SafeRelPath) -> Result<(), FilesError> {
        let mut source = self
            .dir
            .open(from.as_path())
            .map_err(|e| FilesError::io(from, e))?;
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        let mut target = self
            .dir
            .open_with(to.as_path(), &options)
            .map_err(|e| FilesError::io(to, e))?;
        io::copy(&mut source, &mut target).map_err(|e| FilesError::io(to, e))?;
        target.flush().map_err(|e| FilesError::io(to, e))?;
        self.own(to)
    }

    fn copy_dir(&self, from: &SafeRelPath, to: &SafeRelPath) -> Result<(), FilesError> {
        self.create_dir(to)?;
        for entry in self.list(from)? {
            let target = to.join(&entry.name)?;
            match entry.kind {
                EntryKind::Directory => self.copy_dir(&entry.path, &target)?,
                EntryKind::File => self.copy_file(&entry.path, &target)?,
                EntryKind::Symlink | EntryKind::Other => {}
            }
        }
        Ok(())
    }

    fn trash_dir(&self) -> io::Result<Dir> {
        std::fs::create_dir_all(&self.trash)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&self.trash, std::fs::Permissions::from_mode(0o700))?;
        }
        Dir::open_ambient_dir(&self.trash, ambient_authority())
    }
}

/// A unique hidden name next to `path`, for writing before an atomic rename.
pub(crate) fn temp_sibling(path: &SafeRelPath, name: &str) -> Result<SafeRelPath, FilesError> {
    let token = crate::random::random_token::<6>()
        .map_err(|e| FilesError::InvalidOperation(format!("no randomness available: {e}")))?;
    let short: String = name.chars().take(40).collect();
    Ok(path
        .parent()
        .unwrap_or_default()
        .join(&format!(".{short}.{token}.gsd-tmp"))?)
}

/// The revision of a file's content (what an editor sends back to save).
pub fn revision(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn text_file(
    path: &SafeRelPath,
    bytes: Vec<u8>,
    modified: Option<DateTime<Utc>>,
) -> Result<TextFile, FilesError> {
    // NUL bytes mean binary even when they happen to be valid UTF-8.
    if bytes.contains(&0) {
        return Err(FilesError::NotText(path.clone()));
    }
    let size = bytes.len() as u64;
    let revision = revision(&bytes);
    let content = String::from_utf8(bytes).map_err(|_| FilesError::NotText(path.clone()))?;
    Ok(TextFile {
        path: path.clone(),
        content,
        revision,
        size,
        modified,
    })
}

fn to_utc(time: SystemTime) -> DateTime<Utc> {
    DateTime::<Utc>::from(time)
}

#[cfg(unix)]
fn chown_entry(parent: &Dir, name: &str, ids: SandboxIds) -> io::Result<()> {
    use rustix::fs::{AtFlags, Gid, Uid, chownat};
    chownat(
        parent,
        name,
        Some(Uid::from_raw(ids.uid)),
        Some(Gid::from_raw(ids.gid)),
        AtFlags::SYMLINK_NOFOLLOW,
    )
    .map_err(io::Error::from)
}

#[cfg(not(unix))]
fn chown_entry(_parent: &Dir, _name: &str, _ids: SandboxIds) -> io::Result<()> {
    Ok(())
}
