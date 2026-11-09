//! What a server has installed: every jar in its content folder (`x.jar`, or `x.jar.disabled`
//! when turned off, as Prism does), joined with the packwiz index (where it came from) and with
//! what the jar says about itself. Reading and hashing a jar is cached by name, size and time,
//! so listing a big modpack again is cheap.

use std::{
    collections::HashMap,
    io::Cursor,
    sync::{Arc, Mutex, PoisonError},
};

use chrono::{DateTime, Utc};
use gsd_core::files::{Entry, EntryKind, FilesError, SafeRelPath, ServerFiles};
use serde::Serialize;
use sha1::{Digest, Sha1};
use tokio::io::AsyncReadExt;

use crate::{
    index::{self, INDEX_FOLDER, IndexEntry, Origin},
    metadata::{self, JarInfo},
    model::SourceKind,
    resolver::{Installed, InstalledVersion},
};

/// The suffix of a turned-off jar.
pub const DISABLED: &str = ".disabled";
/// Jars bigger than this are listed but not read (no real mod is this big).
const MAX_READ_BYTES: u64 = 256 * 1024 * 1024;

/// One installed file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct InstalledItem {
    /// The jar's name, without `.disabled`.
    pub file: String,
    pub enabled: bool,
    pub size: u64,
    pub modified: Option<DateTime<Utc>>,
    /// The name to show: the index's, the jar's, or the file name.
    pub name: String,
    pub version: Option<String>,
    /// The mod id or plugin name the loader knows it by.
    pub mod_id: Option<String>,
    pub source: Option<Origin>,
    pub sha1: Option<String>,
    /// The index entry's file name (inside `.index/`), when there is one.
    #[serde(skip)]
    pub index_file: Option<String>,
}

/// Remembers what was read from each jar (its metadata and SHA-1).
#[derive(Debug, Clone, Default)]
pub struct ScanCache {
    jars: Arc<Mutex<HashMap<JarKey, (JarInfo, String)>>>,
}

/// A jar as last seen: which server, its name, size and time. A changed file is read again.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct JarKey {
    server: String,
    name: String,
    size: u64,
    modified: Option<DateTime<Utc>>,
}

/// Lists `folder` (e.g. `mods`) of a server, newest index data joined in.
pub async fn list(
    files: &ServerFiles,
    folder: &str,
    cache: &ScanCache,
    cache_key: &str,
) -> Result<Vec<InstalledItem>, FilesError> {
    let folder_path = SafeRelPath::parse(folder)?;
    let entries = match files.list(folder_path.clone()).await {
        Ok(entries) => entries,
        Err(FilesError::NotFound(_)) => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let index = read_index(files, &folder_path).await;
    let mut items = Vec::new();
    for entry in entries.iter().filter(|entry| entry.kind == EntryKind::File) {
        let Some((file, enabled)) = jar_name(&entry.name) else {
            continue;
        };
        let (info, sha1) = read_jar(files, entry, cache, cache_key).await;
        let indexed = index.get(file);
        items.push(InstalledItem {
            name: indexed
                .map(|(_, entry)| entry.name.clone())
                .or_else(|| info.name.clone())
                .unwrap_or_else(|| file.to_owned()),
            file: file.to_owned(),
            enabled,
            size: entry.size.unwrap_or(0),
            modified: entry.modified,
            version: info.version,
            mod_id: info.id,
            source: indexed.and_then(|(_, entry)| entry.origin()),
            sha1,
            index_file: indexed.map(|(name, _)| name.clone()),
        });
    }
    items.sort_by_key(|item| item.name.to_lowercase());
    Ok(items)
}

/// What the resolver needs, as `source` sees the server: what it tracks (from the index) by id,
/// and everything else by name.
pub fn by_project(items: &[InstalledItem], source: SourceKind) -> Installed {
    let mut installed = Installed::new();
    for item in items {
        let mut names = vec![item.name.as_str()];
        names.extend(item.mod_id.as_deref());
        match &item.source {
            Some(origin) if origin.source == source => installed.add_tracked(
                origin.project_id.clone(),
                InstalledVersion {
                    filename: item.file.clone(),
                    version_id: Some(origin.version_id.clone()),
                },
                &names,
            ),
            _ => installed.add_untracked(&item.file, &names),
        }
    }
    installed
}

/// `x.jar` → (`x.jar`, enabled); `x.jar.disabled` → (`x.jar`, disabled); anything else → none.
pub fn jar_name(name: &str) -> Option<(&str, bool)> {
    let (file, enabled) = match name.strip_suffix(DISABLED) {
        Some(file) => (file, false),
        None => (name, true),
    };
    file.to_ascii_lowercase()
        .ends_with(".jar")
        .then_some((file, enabled))
}

/// The index entries of `folder`, by the jar file they describe.
async fn read_index(files: &ServerFiles, folder: &SafeRelPath) -> HashMap<String, (String, IndexEntry)> {
    let Ok(index_folder) = folder.join(INDEX_FOLDER) else {
        return HashMap::new();
    };
    read_entries(files, &index_folder).await
}

/// The entries of the index folder, by the jar file they describe, with their own file names.
pub(crate) async fn read_entries(
    files: &ServerFiles,
    index_folder: &SafeRelPath,
) -> HashMap<String, (String, IndexEntry)> {
    let Ok(entries) = files.list(index_folder.clone()).await else {
        return HashMap::new();
    };
    let mut index = HashMap::new();
    for entry in entries.iter().filter(|entry| index::is_entry(&entry.name)) {
        let Ok(text) = files.read_text(entry.path.clone()).await else {
            continue;
        };
        if let Some(parsed) = IndexEntry::parse(&text.content) {
            index.insert(parsed.filename.clone(), (entry.name.clone(), parsed));
        }
    }
    index
}

/// What the jar says about itself and its SHA-1 (empty when it can't be read).
async fn read_jar(
    files: &ServerFiles,
    entry: &Entry,
    cache: &ScanCache,
    cache_key: &str,
) -> (JarInfo, Option<String>) {
    let key = JarKey {
        server: cache_key.to_owned(),
        name: entry.name.clone(),
        size: entry.size.unwrap_or(0),
        modified: entry.modified,
    };
    if let Some((info, sha1)) = cache
        .jars
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(&key)
    {
        return (info.clone(), Some(sha1.clone()));
    }
    let Some(bytes) = read_bytes(files, entry).await else {
        return (JarInfo::default(), None);
    };
    let (info, sha1) = tokio::task::spawn_blocking(move || {
        let sha1 = hex::encode(Sha1::digest(&bytes));
        (metadata::read(Cursor::new(bytes)), sha1)
    })
    .await
    .unwrap_or_default();
    cache
        .jars
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(key, (info.clone(), sha1.clone()));
    (info, Some(sha1))
}

/// `CurseForge`'s fingerprint of an installed file (`None` when it can't be read). Only identifying
/// needs it, so unlike the SHA-1 it isn't worked out for every listing.
pub(crate) async fn fingerprint_of(files: &ServerFiles, folder: &str, item: &InstalledItem) -> Option<u32> {
    let name = if item.enabled {
        item.file.clone()
    } else {
        format!("{}{DISABLED}", item.file)
    };
    let path = SafeRelPath::parse(folder).ok()?.join(&name).ok()?;
    let bytes = read_path(files, path).await?;
    tokio::task::spawn_blocking(move || crate::fingerprint::fingerprint(&bytes))
        .await
        .ok()
}

async fn read_bytes(files: &ServerFiles, entry: &Entry) -> Option<Vec<u8>> {
    read_path(files, entry.path.clone()).await
}

async fn read_path(files: &ServerFiles, path: SafeRelPath) -> Option<Vec<u8>> {
    let (file, size) = files.open_file(path).await.ok()?;
    if size > MAX_READ_BYTES {
        return None;
    }
    let mut bytes = Vec::with_capacity(usize::try_from(size).unwrap_or(0));
    file.take(MAX_READ_BYTES).read_to_end(&mut bytes).await.ok()?;
    Some(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jar_names_and_their_switch() {
        assert_eq!(jar_name("lithium.jar"), Some(("lithium.jar", true)));
        assert_eq!(jar_name("lithium.jar.disabled"), Some(("lithium.jar", false)));
        assert_eq!(jar_name("Upper.JAR"), Some(("Upper.JAR", true)));
        assert_eq!(jar_name("config.toml"), None);
        assert_eq!(jar_name("notes.disabled"), None);
    }
}
