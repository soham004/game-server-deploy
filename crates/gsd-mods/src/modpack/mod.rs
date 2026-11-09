//! Modpacks: an uploaded archive becomes a server.
//!
//! - An `.mrpack` or a `CurseForge` pack names a Minecraft version and a loader, so a server is
//!   created for it, and then, in the same job, its files are downloaded (each checked against
//!   its hash) and its override folders copied over. What the sources recognise gets tracked.
//! - Any other zip is taken for a server folder: it is unpacked into the imports folder, where the
//!   usual import takes over (loader detection, confirming, copying into a new server).
//!
//! An archive is untrusted. Paths go through [`SafeRelPath`] and the file manager, links are left
//! out, unpacking is capped, and an `.mrpack` can only make this app download from the hosts its
//! format allows, since otherwise an upload could aim the app's requests anywhere.

pub mod browse;
pub mod format;

use std::{
    collections::HashSet,
    io::{self, Read},
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

use bytes::Bytes;
use futures_util::{Stream, StreamExt};
use gsd_core::{
    files::{FilesError, SafeRelPath, ServerFiles},
    ids::JobId,
    instances::{Instance, ModpackRecord},
    jobs::{JobContext, JobFailure, error_chain},
    random,
};
use gsd_minecraft::{
    CreateError, CreateRequest, MinecraftService, install::point_at_port, settings::MinecraftServer,
};
use serde::Serialize;
use tokio::io::AsyncWriteExt;
use tokio_util::io::ReaderStream;

use crate::{error::ContentError, install, model::ContentFile, service::ContentService, target::Target};
use format::{CurseForgeFile, MrpackFile, Pack, PackContents, PackInfo};

/// The largest archive accepted.
const MAX_UPLOAD_BYTES: u64 = 4 * 1024 * 1024 * 1024;
/// How much one archive may unpack to, and into how many entries (zip bombs).
const MAX_UNPACKED_BYTES: u64 = 16 * 1024 * 1024 * 1024;
const MAX_UNPACKED_ENTRIES: usize = 200_000;
/// An upload nobody installed goes after this long.
const UPLOAD_LIFETIME: Duration = Duration::from_secs(24 * 60 * 60);
/// Where the `.mrpack` format allows downloads from.
const ALLOWED_HOSTS: [&str; 4] = [
    "cdn.modrinth.com",
    "github.com",
    "raw.githubusercontent.com",
    "gitlab.com",
];

/// What an uploaded archive turned out to be.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Uploaded {
    /// A modpack, waiting to be installed.
    Pack {
        upload_id: String,
        pack: PackInfo,
        /// Its author's server version came along, and is what gets installed.
        server_pack: bool,
    },
    /// A server folder, now in the imports folder under this name.
    ServerFolder { folder: String },
}

/// What the user confirmed about an uploaded pack.
#[derive(Debug, Clone)]
pub struct InstallPack {
    pub upload_id: String,
    pub name: String,
    pub max_memory_mb: u32,
    pub eula_accepted: bool,
}

/// Why a pack can't become a server.
#[derive(Debug, thiserror::Error)]
pub enum ModpackError {
    #[error("the upload is bigger than the limit of {} GB", MAX_UPLOAD_BYTES / (1024 * 1024 * 1024))]
    TooBig,
    #[error("that upload isn't here any more; upload the pack again")]
    UnknownUpload,
    #[error("that source has no modpacks")]
    NoPacksThere,
    #[error("{name} can only be downloaded by hand: get it from {url}, then upload it here")]
    ByHand { name: String, url: String },
    #[error(transparent)]
    Pack(#[from] format::PackError),
    #[error(transparent)]
    Create(#[from] CreateError),
    #[error(transparent)]
    Content(#[from] ContentError),
    #[error("could not store the upload")]
    Io(#[from] io::Error),
}

impl ContentService {
    /// Where uploads wait: next to the content cache, and never served.
    fn uploads_dir(&self) -> PathBuf {
        self.inner.cache_dir.with_file_name("modpacks")
    }

    /// Where an upload's archive is. The id comes from a client, and becomes a file name.
    fn upload_path(&self, upload_id: &str) -> Result<PathBuf, ModpackError> {
        if upload_id.is_empty() || !upload_id.chars().all(|c| c.is_ascii_alphanumeric()) {
            return Err(ModpackError::UnknownUpload);
        }
        Ok(self.uploads_dir().join(format!("{upload_id}.zip")))
    }

    /// Stores an uploaded archive and says what it is. A plain zip goes to `imports_dir`.
    pub async fn upload_modpack<S, E>(
        &self,
        file_name: &str,
        mut body: S,
        imports_dir: &Path,
    ) -> Result<Uploaded, ModpackError>
    where
        S: Stream<Item = Result<Bytes, E>> + Unpin + Send,
        E: std::fmt::Display,
    {
        let dir = self.uploads_dir();
        tokio::fs::create_dir_all(&dir).await?;
        remove_old_uploads(&dir).await;
        let upload_id = new_upload_id()?;
        let path = self.upload_path(&upload_id)?;
        let mut file = tokio::fs::File::create(&path).await?;
        let mut written: u64 = 0;
        while let Some(chunk) = body.next().await {
            let chunk = chunk.map_err(|error| io::Error::other(error.to_string()))?;
            written += chunk.len() as u64;
            if written > MAX_UPLOAD_BYTES {
                drop(file);
                let _ = tokio::fs::remove_file(&path).await;
                return Err(ModpackError::TooBig);
            }
            file.write_all(&chunk).await?;
        }
        file.flush().await?;
        drop(file);
        classify(&upload_id, &path, file_name, imports_dir).await
    }

    /// Creates a server for an uploaded pack; one job installs the server and then the pack.
    pub async fn install_modpack(
        &self,
        request: InstallPack,
        minecraft: &MinecraftService,
    ) -> Result<(Instance, JobId), ModpackError> {
        let path = self.upload_path(&request.upload_id)?;
        if !tokio::fs::try_exists(&path).await.unwrap_or(false) {
            return Err(ModpackError::UnknownUpload);
        }
        let pack = read_pack(path.clone())
            .await?
            .ok_or(ModpackError::UnknownUpload)?;
        if matches!(pack.contents, PackContents::Curseforge(_)) && !self.has_curseforge_key() {
            return Err(ContentError::Source(crate::SourceError::NoKey).into());
        }
        let origin = browse::FetchNote::read(&path).await.map(|note| note.origin);
        let record = modpack_record(&pack.info, origin);
        let create = CreateRequest {
            name: request.name,
            mc_version: pack.info.mc_version.clone(),
            loader: pack.info.loader,
            loader_version: pack.info.loader_version.clone(),
            max_memory_mb: request.max_memory_mb,
            eula_accepted: request.eula_accepted,
        };
        let service = self.clone();
        Ok(minecraft
            .create_then(create, move |instance, job| async move {
                let result = service.fill(&instance, &pack, &path, &job).await;
                let _ = tokio::fs::remove_file(&path).await;
                let _ = tokio::fs::remove_file(browse::FetchNote::path(&path)).await;
                let _ = tokio::fs::remove_file(server_pack_path(&path)).await;
                result.map_err(|error| JobFailure::msg(error_chain(&error)))?;
                // Only for showing and updating it: the server works either way.
                let saved = service
                    .inner
                    .instances
                    .registry()
                    .update(instance.id(), |definition| definition.modpack = Some(record))
                    .await;
                if let Err(error) = saved {
                    job.log(format!("Could not record which pack this is: {error}"));
                }
                Ok(())
            })
            .await?)
    }

    /// The pack's part of the job: its files and overrides, or instead its author's server version
    /// when there is one; then what the sources recognise.
    async fn fill(
        &self,
        instance: &Instance,
        pack: &Pack,
        archive: &Path,
        job: &JobContext,
    ) -> Result<(), ModpackError> {
        let files = ServerFiles::open(instance).await.map_err(ContentError::from)?;
        let server_pack = server_pack_path(archive);
        let has_server_pack = tokio::fs::try_exists(&server_pack).await.unwrap_or(false);
        if !has_server_pack {
            job.step(format!(
                "Downloading {} files for {}",
                pack.info.files, pack.info.name
            ));
            match &pack.contents {
                PackContents::Mrpack(wanted) => self.fetch_mrpack(&files, wanted, job).await?,
                PackContents::Curseforge(wanted) => self.fetch_curseforge(&files, wanted, job).await?,
            }
        }
        job.step(if has_server_pack {
            "Copying the server pack"
        } else {
            "Copying the pack's own files"
        });
        let staging = archive.with_extension("staging");
        let to = staging.clone();
        let port = instance.definition.ports.game;
        let (from, overrides) = (archive.to_owned(), pack.overrides.clone());
        tokio::task::spawn_blocking(move || {
            if has_server_pack {
                let top = single_top_folder(&server_pack)?;
                unpack_folders(&server_pack, std::slice::from_ref(&top), &to)?;
            } else {
                unpack_folders(&from, &overrides, &to)?;
            }
            keep_settings_of_this_server(&to, port)
        })
        .await
        .map_err(io::Error::other)??;
        let copied = copy_tree(&files, &staging).await;
        let _ = tokio::fs::remove_dir_all(&staging).await;
        job.log(format!("Copied {} files from the pack", copied?));

        job.step("Looking up what was installed");
        let server = MinecraftServer::from_table(&instance.definition.server).ok();
        if let Some(target) = server.as_ref().and_then(Target::for_server) {
            // Only tracking: a lookup that fails leaves working files, not a failed install.
            match self.identify_on(instance, &target).await {
                Ok(found) => job.log(format!(
                    "Tracking {}; not found on any source: {}",
                    found.identified.len(),
                    found.still_unknown.len()
                )),
                Err(error) => job.log(format!("Could not look the files up: {error}")),
            }
        }
        Ok(())
    }

    async fn fetch_mrpack(
        &self,
        files: &ServerFiles,
        wanted: &[MrpackFile],
        job: &JobContext,
    ) -> Result<(), ModpackError> {
        let allowed = self.allowed_download_origin();
        for (position, file) in wanted.iter().enumerate() {
            job.check_cancelled().map_err(|_| ContentError::Cancelled)?;
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
            let cached =
                install::download(self.inner.sources.client(), &content, &self.inner.cache_dir, job).await?;
            place(files, &file.path, &cached).await?;
            job.log(format!("{}/{} {}", position + 1, wanted.len(), file.path));
        }
        Ok(())
    }

    /// A blocked file is logged and left out, and the rest still comes: the server's Mods tab then
    /// shows what is there, and the file can be added once downloaded.
    async fn fetch_curseforge(
        &self,
        files: &ServerFiles,
        wanted: &[CurseForgeFile],
        job: &JobContext,
    ) -> Result<(), ModpackError> {
        let ids: Vec<u64> = wanted.iter().map(|file| file.file).collect();
        let versions = self
            .inner
            .sources
            .curseforge
            .files(&ids)
            .await
            .map_err(ContentError::from)?;
        for version in &versions {
            job.check_cancelled().map_err(|_| ContentError::Cancelled)?;
            let name = &version.file.filename;
            if version.file.url.is_empty() {
                job.log(format!(
                    "Not installed: {name}: its author only allows downloading it by hand"
                ));
                continue;
            }
            // Packs also list resource packs and the like, which a server has no use for.
            if !name.to_ascii_lowercase().ends_with(".jar") {
                job.log(format!("Left out: {name} (not a mod)"));
                continue;
            }
            let cached = install::download(
                self.inner.sources.client(),
                &version.file,
                &self.inner.cache_dir,
                job,
            )
            .await?;
            place(files, &format!("mods/{name}"), &cached).await?;
            job.log(format!("Installed {name}"));
        }
        Ok(())
    }

    /// The configured Modrinth API's own origin may serve downloads too, which is where tests' stand-in lives.
    fn allowed_download_origin(&self) -> Option<String> {
        reqwest::Url::parse(self.inner.sources.modrinth.api())
            .ok()
            .map(|url| url.origin().ascii_serialization())
    }
}

/// Whether an `.mrpack` may make this app fetch `url`.
fn download_allowed(url: &str, extra_origin: Option<&str>) -> bool {
    let Ok(parsed) = reqwest::Url::parse(url) else {
        return false;
    };
    if extra_origin.is_some_and(|origin| parsed.origin().ascii_serialization() == origin) {
        return true;
    }
    parsed.scheme() == "https"
        && parsed
            .host_str()
            .is_some_and(|host| ALLOWED_HOSTS.contains(&host))
}

/// Where the author's server version of a fetched pack waits, next to the pack.
fn server_pack_path(archive: &Path) -> PathBuf {
    archive.with_extension("server.zip")
}

/// A pack's files must not undo what is settled for this server. The EULA was accepted when it was
/// created. And `server.properties` often comes along naming the default port, which the sandbox
/// wouldn't let this server bind, so it keeps the rest of that file but not the port.
fn keep_settings_of_this_server(staging: &Path, port: u16) -> Result<(), ModpackError> {
    match std::fs::remove_file(staging.join("eula.txt")) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    if staging.join("server.properties").is_file() {
        point_at_port(staging, port).map_err(|error| io::Error::other(error.to_string()))?;
    }
    Ok(())
}

fn new_upload_id() -> Result<String, ModpackError> {
    let bytes = random::random_bytes::<16>().map_err(|error| io::Error::other(error.to_string()))?;
    Ok(hex::encode(bytes))
}

/// What goes in `instance.toml`: where the pack came from, or `file` for an upload.
fn modpack_record(info: &PackInfo, origin: Option<browse::PackOrigin>) -> ModpackRecord {
    let (source, project_id, version_id) = match origin {
        Some(origin) => (
            origin.source.as_str(),
            Some(origin.project_id),
            Some(origin.version_id),
        ),
        None => ("file", None, None),
    };
    ModpackRecord {
        source: source.to_owned(),
        project_id,
        version_id,
        name: info.name.clone(),
        version: info.version.clone(),
    }
}

/// Decides what an archive is. A pack stays where it is, waiting to be installed. Anything else is a
/// server folder and goes to `imports_dir`. Only a pack's archive is left behind.
async fn classify(
    upload_id: &str,
    path: &Path,
    file_name: &str,
    imports_dir: &Path,
) -> Result<Uploaded, ModpackError> {
    match read_pack(path.to_owned()).await {
        Ok(Some(pack)) => Ok(Uploaded::Pack {
            upload_id: upload_id.to_owned(),
            pack: pack.info,
            server_pack: false,
        }),
        Ok(None) => {
            let folder = import_folder_name(file_name);
            let imports = imports_dir.to_owned();
            let source = path.to_owned();
            let unpacked = tokio::task::spawn_blocking(move || unpack_server_zip(&source, &imports, &folder))
                .await
                .map_err(io::Error::other)?;
            let _ = tokio::fs::remove_file(path).await;
            Ok(Uploaded::ServerFolder { folder: unpacked? })
        }
        Err(error) => {
            let _ = tokio::fs::remove_file(path).await;
            Err(error)
        }
    }
}

async fn read_pack(path: PathBuf) -> Result<Option<Pack>, ModpackError> {
    let pack = tokio::task::spawn_blocking(move || {
        let file = std::fs::File::open(&path)?;
        Ok::<_, ModpackError>(format::read(io::BufReader::new(file))?)
    })
    .await
    .map_err(io::Error::other)??;
    Ok(pack)
}

async fn remove_old_uploads(dir: &Path) {
    let Ok(mut entries) = tokio::fs::read_dir(dir).await else {
        return;
    };
    while let Ok(Some(entry)) = entries.next_entry().await {
        let old = entry
            .metadata()
            .await
            .and_then(|metadata| metadata.modified())
            .ok()
            .and_then(|modified| SystemTime::now().duration_since(modified).ok())
            .is_some_and(|age| age > UPLOAD_LIFETIME);
        if old {
            let path = entry.path();
            let _ = tokio::fs::remove_file(&path).await;
            let _ = tokio::fs::remove_dir_all(&path).await;
        }
    }
}

/// `My Server (1).zip` becomes `My-Server--1-`: one plain folder name.
fn import_folder_name(file_name: &str) -> String {
    let stem = Path::new(file_name)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("server");
    let safe: String = stem
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    if safe.is_empty() { "server".into() } else { safe }
}

/// Unpacks a server zip into a new folder in `imports`, without a single top folder if it has one
/// (server packs often wrap everything in one). Returns the folder's name.
fn unpack_server_zip(archive: &Path, imports: &Path, wanted: &str) -> Result<String, ModpackError> {
    std::fs::create_dir_all(imports)?;
    let folder = free_name(imports, wanted);
    let target = imports.join(&folder);
    let top = single_top_folder(archive)?;
    unpack_folders(archive, std::slice::from_ref(&top), &target)?;
    Ok(folder)
}

/// `name`, or `name-2`, `name-3`… whichever isn't taken.
fn free_name(dir: &Path, name: &str) -> String {
    let mut candidate = name.to_owned();
    let mut number = 1;
    while dir.join(&candidate).exists() {
        number += 1;
        candidate = format!("{name}-{number}");
    }
    candidate
}

/// The one folder everything in the archive is inside, or `""` when there isn't one.
fn single_top_folder(archive: &Path) -> Result<String, ModpackError> {
    let mut archive = open_zip(archive)?;
    let mut tops = HashSet::new();
    for index in 0..archive.len() {
        let entry = archive.by_index(index).map_err(invalid)?;
        let name = entry.name().map_err(invalid)?.into_owned();
        match name.split_once('/') {
            Some((top, _)) => tops.insert(top.to_owned()),
            None => return Ok(String::new()),
        };
    }
    Ok(if tops.len() == 1 {
        tops.into_iter().next().unwrap_or_default()
    } else {
        String::new()
    })
}

fn open_zip(path: &Path) -> Result<zip::ZipArchive<io::BufReader<std::fs::File>>, ModpackError> {
    let file = std::fs::File::open(path)?;
    zip::ZipArchive::new(io::BufReader::new(file)).map_err(|_| format::PackError::NotAnArchive.into())
}

fn invalid(error: zip::result::ZipError) -> ModpackError {
    format::PackError::Invalid(error.to_string()).into()
}

/// Unpacks what is inside the folders `prefixes` (`""`: everything) into `target`, later ones
/// over earlier ones. Names are checked, links left out, and the bytes actually written are capped.
fn unpack_folders(archive: &Path, prefixes: &[String], target: &Path) -> Result<(), ModpackError> {
    std::fs::create_dir_all(target)?;
    let mut archive = open_zip(archive)?;
    if archive.len() > MAX_UNPACKED_ENTRIES {
        return Err(format::PackError::Invalid("it has too many files".into()).into());
    }
    let mut left = MAX_UNPACKED_BYTES;
    for prefix in prefixes {
        let start = if prefix.is_empty() {
            String::new()
        } else {
            format!("{}/", prefix.trim_end_matches('/'))
        };
        for index in 0..archive.len() {
            let mut entry = archive.by_index(index).map_err(invalid)?;
            let is_link = entry
                .unix_mode()
                .is_some_and(|mode| mode & 0o170_000 == 0o120_000);
            let name = entry.name().map_err(invalid)?.replace('\\', "/");
            let Some(inside) = name.strip_prefix(&start) else {
                continue;
            };
            if is_link || inside.is_empty() {
                continue;
            }
            let Ok(relative) = SafeRelPath::parse(inside) else {
                continue;
            };
            let destination = target.join(relative.as_path());
            if entry.is_dir() {
                std::fs::create_dir_all(&destination)?;
                continue;
            }
            if let Some(parent) = destination.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let mut output = std::fs::File::create(&destination)?;
            let written = io::copy(&mut (&mut entry).take(left.saturating_add(1)), &mut output)?;
            if written > left {
                return Err(format::PackError::Invalid("it unpacks to too much".into()).into());
            }
            left -= written;
        }
    }
    Ok(())
}

/// Copies a file from the cache to `path` in the server, through the file manager (so it can't be
/// redirected by a link, and belongs to the server's user).
async fn place(files: &ServerFiles, path: &str, cached: &Path) -> Result<(), ContentError> {
    let destination = SafeRelPath::parse(path)?;
    ensure_parents(files, &destination).await?;
    let source = tokio::fs::File::open(cached)
        .await
        .map_err(|source| ContentError::Cache {
            path: cached.to_owned(),
            source,
        })?;
    files
        .upload(destination, ReaderStream::new(source), true, u64::MAX)
        .await?;
    Ok(())
}

async fn ensure_parents(files: &ServerFiles, path: &SafeRelPath) -> Result<(), FilesError> {
    let mut built = SafeRelPath::root();
    let segments: Vec<&str> = path.as_str().split('/').collect();
    for segment in &segments[..segments.len().saturating_sub(1)] {
        built = built.join(segment)?;
        match files.create_dir(built.clone()).await {
            Ok(()) | Err(FilesError::AlreadyExists(_)) => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

/// Copies everything under `staging` into the server, replacing what is there. Returns how many files.
async fn copy_tree(files: &ServerFiles, staging: &Path) -> Result<u64, ContentError> {
    let mut count = 0;
    let mut folders = vec![staging.to_owned()];
    while let Some(folder) = folders.pop() {
        let cache_error = |source| ContentError::Cache {
            path: folder.clone(),
            source,
        };
        let mut entries = tokio::fs::read_dir(&folder).await.map_err(cache_error)?;
        while let Some(entry) = entries.next_entry().await.map_err(cache_error)? {
            let path = entry.path();
            if entry.file_type().await.map_err(cache_error)?.is_dir() {
                folders.push(path);
                continue;
            }
            let Some(relative) = path.strip_prefix(staging).ok().and_then(Path::to_str) else {
                continue;
            };
            place(files, &relative.replace('\\', "/"), &path).await?;
            count += 1;
        }
    }
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packs_may_only_download_from_the_formats_hosts() {
        assert!(download_allowed("https://cdn.modrinth.com/data/x/y.jar", None));
        assert!(download_allowed(
            "https://github.com/a/b/releases/download/c.jar",
            None
        ));
        assert!(
            !download_allowed("http://cdn.modrinth.com/data/x/y.jar", None),
            "https only"
        );
        assert!(!download_allowed("https://evil.example/x.jar", None));
        assert!(
            !download_allowed("http://169.254.169.254/latest/meta-data", None),
            "cloud metadata"
        );
        assert!(!download_allowed(
            "https://cdn.modrinth.com.evil.example/x.jar",
            None
        ));
        assert!(!download_allowed("file:///etc/passwd", None));
        assert!(!download_allowed("not a url", None));
        assert!(download_allowed(
            "http://127.0.0.1:9000/x.jar",
            Some("http://127.0.0.1:9000")
        ));
        assert!(!download_allowed(
            "http://127.0.0.1:9001/x.jar",
            Some("http://127.0.0.1:9000")
        ));
    }

    #[test]
    fn folder_names_stay_plain() {
        assert_eq!(import_folder_name("My Server (1).zip"), "My-Server--1-");
        assert_eq!(import_folder_name("../../etc.zip"), "etc");
        assert_eq!(import_folder_name(""), "server");
    }
}
