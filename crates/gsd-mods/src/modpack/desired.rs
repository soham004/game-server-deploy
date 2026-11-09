//! Which files a version of a pack puts on a server, where each comes from, and putting them there.
//! An install and an update differ only in what they compare this with (see [`super::changes`]).

use std::{
    collections::BTreeMap,
    io::{self, Read},
    path::{Path, PathBuf},
};

use gsd_core::{
    files::{FilesError, SafeRelPath, ServerFiles},
    instances::Instance,
    jobs::JobContext,
};
use sha1::{Digest, Sha1};

use super::{
    ModpackError,
    changes::{FileChanges, KeptReason, LOCK_FILE, NewFile},
    download_allowed,
    format::{self, CurseForgeFile, MrpackFile, Pack, PackContents},
    keep_settings_of_this_server, place, server_pack_path, single_top_folder, unpack_folders,
};
use crate::{error::ContentError, install, model::ContentFile, service::ContentService};

/// Where one file comes from.
#[derive(Debug, Clone)]
pub(super) enum Origin {
    Download(ContentFile),
    /// Unpacked from the archive, here.
    Staged(PathBuf),
}

#[derive(Debug, Clone)]
pub(super) struct Wanted {
    pub(super) sha1: Option<String>,
    pub(super) origin: Origin,
}

/// Everything a version of a pack puts on a server, by path.
#[derive(Debug, Default)]
pub(super) struct Desired {
    pub(super) files: BTreeMap<String, Wanted>,
    /// What the pack lists and this can't install, worded for the user.
    pub(super) notes: Vec<String>,
}

impl Desired {
    pub(super) fn new_files(&self) -> Vec<NewFile<'_>> {
        self.files
            .iter()
            .map(|(path, wanted)| NewFile {
                path,
                sha1: wanted.sha1.as_deref(),
            })
            .collect()
    }

    /// Adds a file. The first check of a path from a pack, which is untrusted, happens here. Later
    /// additions win: overrides replace what was downloaded.
    fn add(&mut self, path: &str, sha1: Option<String>, origin: Origin) -> Result<(), ContentError> {
        let parsed = SafeRelPath::parse(path)?;
        // The lock is this app's record, and a pack doesn't get to write it.
        if parsed.as_str() != LOCK_FILE {
            self.files
                .insert(parsed.as_str().to_owned(), Wanted { sha1, origin });
        }
        Ok(())
    }
}

impl ContentService {
    /// What this version of the pack puts on `instance`. Its overrides, or its author's server
    /// version when one came along, are unpacked into `staging`, which the caller removes.
    pub(super) async fn desired(
        &self,
        pack: &Pack,
        archive: &Path,
        staging: &Path,
        instance: &Instance,
    ) -> Result<Desired, ModpackError> {
        let server_pack = server_pack_path(archive);
        let has_server_pack = tokio::fs::try_exists(&server_pack).await.unwrap_or(false);
        let mut desired = Desired::default();
        if !has_server_pack {
            match &pack.contents {
                PackContents::Mrpack(wanted) => self.add_mrpack_files(&mut desired, wanted)?,
                PackContents::Curseforge(wanted) => {
                    self.add_curseforge_files(&mut desired, wanted).await?;
                }
            }
        }
        let (from, overrides, to) = (archive.to_owned(), pack.overrides.clone(), staging.to_owned());
        let port = instance.definition.ports.game;
        let staged = tokio::task::spawn_blocking(move || {
            let _ = std::fs::remove_dir_all(&to);
            if has_server_pack {
                let top = single_top_folder(&server_pack)?;
                unpack_folders(&server_pack, std::slice::from_ref(&top), &to)?;
            } else {
                unpack_folders(&from, &overrides, &to)?;
            }
            keep_settings_of_this_server(&to, port)?;
            staged_files(&to)
        })
        .await
        .map_err(io::Error::other)??;
        for (path, sha1, file) in staged {
            desired.add(&path, Some(sha1), Origin::Staged(file))?;
        }
        Ok(desired)
    }

    fn add_mrpack_files(&self, desired: &mut Desired, wanted: &[MrpackFile]) -> Result<(), ModpackError> {
        let allowed = self.allowed_download_origin();
        for file in wanted {
            let name = file.path.rsplit('/').next().unwrap_or(&file.path).to_owned();
            let url = file
                .downloads
                .iter()
                .find(|url| download_allowed(url, allowed.as_deref()))
                .ok_or_else(|| {
                    format::PackError::Invalid(format!(
                        "{name} is hosted somewhere packs may not download from"
                    ))
                })?;
            let content = ContentFile {
                url: url.clone(),
                filename: name,
                size: file.size,
                sha1: file.sha1.clone(),
                sha256: None,
                sha512: file.sha512.clone(),
                manual_url: None,
            };
            desired.add(&file.path, file.sha1.clone(), Origin::Download(content))?;
        }
        Ok(())
    }

    /// A blocked file is named and left out, and the rest still comes: the server's Mods tab then
    /// shows what is there, and the file can be added once downloaded.
    async fn add_curseforge_files(
        &self,
        desired: &mut Desired,
        wanted: &[CurseForgeFile],
    ) -> Result<(), ModpackError> {
        let ids: Vec<u64> = wanted.iter().map(|file| file.file).collect();
        let versions = self
            .inner
            .sources
            .curseforge
            .files(&ids)
            .await
            .map_err(ContentError::from)?;
        for version in versions {
            let name = version.file.filename.clone();
            if version.file.url.is_empty() {
                desired.notes.push(format!(
                    "Not installed: {name}: its author only allows downloading it by hand"
                ));
                continue;
            }
            // Packs also list resource packs and the like, which a server has no use for.
            if !name.to_ascii_lowercase().ends_with(".jar") {
                desired.notes.push(format!("Left out: {name} (not a mod)"));
                continue;
            }
            desired.add(
                &format!("mods/{name}"),
                version.file.sha1.clone(),
                Origin::Download(version.file),
            )?;
        }
        Ok(())
    }

    /// Carries out `changes`. Returns the new version's files and their hashes, for the lock.
    pub(super) async fn apply_changes(
        &self,
        files: &ServerFiles,
        desired: &Desired,
        changes: &FileChanges,
        job: &JobContext,
    ) -> Result<BTreeMap<String, String>, ModpackError> {
        let mut hashes: BTreeMap<String, String> = desired
            .files
            .iter()
            .map(|(path, wanted)| (path.clone(), wanted.sha1.clone().unwrap_or_default()))
            .collect();
        // A file left in place because it isn't the pack's stays that way: listing it would make it
        // the pack's to remove later.
        for kept in changes
            .kept
            .iter()
            .filter(|kept| kept.reason == KeptReason::NotFromPack)
        {
            hashes.remove(&kept.path);
        }
        let placing: Vec<&String> = changes.add.iter().chain(&changes.replace).collect();
        for (position, path) in placing.iter().enumerate() {
            job.check_cancelled().map_err(|_| ContentError::Cancelled)?;
            let Some(wanted) = desired.files.get(*path) else {
                continue;
            };
            let source = match &wanted.origin {
                Origin::Staged(file) => file.clone(),
                Origin::Download(content) => {
                    let client = self.inner.sources.client();
                    let cached = install::download(client, content, &self.inner.cache_dir, job).await?;
                    job.log(format!("{}/{} {path}", position + 1, placing.len()));
                    cached
                }
            };
            place(files, path, &source).await?;
            // A pack need not say a file's SHA-1, and the next update compares with it.
            if wanted.sha1.is_none() {
                let hash = tokio::task::spawn_blocking(move || sha1_of(&source))
                    .await
                    .map_err(io::Error::other)??;
                hashes.insert((*path).clone(), hash);
            }
        }
        for note in &desired.notes {
            job.log(note);
        }
        for path in &changes.remove {
            let parsed = SafeRelPath::parse(path).map_err(ContentError::from)?;
            match files.delete(vec![parsed]).await {
                Ok(()) | Err(FilesError::NotFound(_)) => {}
                Err(error) => return Err(ContentError::from(error).into()),
            }
        }
        job.log(format!(
            "Put {} files in place, removed {}",
            placing.len(),
            changes.remove.len()
        ));
        Ok(hashes)
    }
}

/// The files under `staging`: path inside it (with `/`), SHA-1, and where the file is.
fn staged_files(staging: &Path) -> Result<Vec<(String, String, PathBuf)>, ModpackError> {
    let mut found = Vec::new();
    let mut folders = vec![staging.to_owned()];
    while let Some(folder) = folders.pop() {
        for entry in std::fs::read_dir(&folder)? {
            let entry = entry?;
            let path = entry.path();
            if entry.file_type()?.is_dir() {
                folders.push(path);
                continue;
            }
            let Some(relative) = path.strip_prefix(staging).ok().and_then(Path::to_str) else {
                continue;
            };
            let relative = relative.replace('\\', "/");
            found.push((relative, sha1_of(&path)?, path));
        }
    }
    Ok(found)
}

fn sha1_of(path: &Path) -> io::Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha1::new();
    let mut buffer = vec![0; 64 * 1024];
    loop {
        match file.read(&mut buffer)? {
            0 => return Ok(hex::encode(hasher.finalize())),
            read => hasher.update(&buffer[..read]),
        }
    }
}
