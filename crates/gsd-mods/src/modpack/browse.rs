//! Finding modpacks on a source, and fetching one. A fetched pack ends up exactly where an uploaded
//! one does, so the same confirm-and-create step follows.
//!
//! Fetching is a job: a pack can run to gigabytes, longer than a reverse proxy keeps a request open.
//! The client gets the upload's id straight away, follows the job, then asks what it turned out to be
//! ([`ContentService::fetched_modpack`]).
//!
//! A `CurseForge` pack with a server version brings both files. The server version has what a server
//! should run: its author has left out what only clients need, which nothing here can tell from
//! `CurseForge`'s data. But real ones often hold only `mods/` and `config/`, with no loader to start
//! them, so the pack's own file still says which loader to install (see `fill`).

use std::path::{Path, PathBuf};

use gsd_core::{
    ids::JobId,
    jobs::{JobContext, JobFailure, JobSpec, error_chain},
};
use serde::{Deserialize, Serialize};

use super::{ModpackError, Uploaded, classify, new_upload_id, read_pack, server_pack_path};
use crate::{
    SourceError,
    error::ContentError,
    install,
    model::{Channel, ContentVersion, SearchPage, SearchQuery, SourceKind},
    service::ContentService,
};

/// Which pack to fetch: the given version, or else its newest release.
#[derive(Debug, Clone)]
pub struct FetchPack {
    pub source: SourceKind,
    pub project_id: String,
    pub version_id: Option<String>,
}

/// A fetch under way.
#[derive(Debug, Clone, Serialize)]
pub struct Fetching {
    pub upload_id: String,
    pub job_id: JobId,
}

/// Where a fetched pack came from, kept next to it until it is installed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct PackOrigin {
    pub(super) source: SourceKind,
    pub(super) project_id: String,
    pub(super) version_id: String,
}

/// What a finished fetch left behind (`<upload id>.json`). Without one, the fetch hasn't finished,
/// or it failed.
#[derive(Debug, Serialize, Deserialize)]
pub(super) struct FetchNote {
    pub(super) origin: PackOrigin,
    /// The imports folder it went to, when it was a plain server folder.
    folder: Option<String>,
    /// The author's server version came along.
    server_pack: bool,
}

impl FetchNote {
    pub(super) fn path(archive: &Path) -> PathBuf {
        archive.with_extension("json")
    }

    pub(super) async fn read(archive: &Path) -> Option<Self> {
        let text = tokio::fs::read(Self::path(archive)).await.ok()?;
        serde_json::from_slice(&text).ok()
    }
}

/// What to download for a pack: its file, and the author's server version if there is one to get.
struct Chosen {
    version: ContentVersion,
    server_pack: Option<ContentVersion>,
}

impl ContentService {
    /// Where packs can be searched now (`CurseForge` needs its key).
    pub fn pack_sources(&self) -> Vec<SourceKind> {
        let mut sources = vec![SourceKind::Modrinth];
        if self.has_curseforge_key() {
            sources.push(SourceKind::CurseForge);
        }
        sources
    }

    pub async fn search_modpacks(
        &self,
        source: SourceKind,
        query: &SearchQuery,
        mc_version: Option<&str>,
    ) -> Result<SearchPage, ModpackError> {
        let sources = &self.inner.sources;
        let page = match source {
            SourceKind::Modrinth => sources.modrinth.search_packs(query, mc_version).await,
            SourceKind::CurseForge => sources.curseforge.search_packs(query, mc_version).await,
            SourceKind::Hangar => return Err(ModpackError::NoPacksThere),
        };
        Ok(page.map_err(ContentError::from)?)
    }

    /// A pack's versions, newest first.
    pub async fn modpack_versions(
        &self,
        source: SourceKind,
        project_id: &str,
    ) -> Result<Vec<ContentVersion>, ModpackError> {
        let sources = &self.inner.sources;
        let versions = match source {
            SourceKind::Modrinth => sources.modrinth.pack_versions(project_id).await,
            SourceKind::CurseForge => sources.curseforge.pack_versions(project_id).await,
            SourceKind::Hangar => return Err(ModpackError::NoPacksThere),
        };
        Ok(versions.map_err(ContentError::from)?)
    }

    /// Starts downloading a pack. What can be told without downloading (no such version, only
    /// downloadable by hand) is an error here.
    pub async fn fetch_modpack(
        &self,
        request: FetchPack,
        imports_dir: PathBuf,
    ) -> Result<Fetching, ModpackError> {
        let chosen = self.choose(&request).await?;
        let upload_id = new_upload_id()?;
        let origin = PackOrigin {
            source: request.source,
            project_id: request.project_id,
            version_id: chosen.version.id.clone(),
        };
        let spec = JobSpec {
            kind: "modpack.fetch",
            title: format!("Downloading {}", chosen.version.name),
            instance_id: None,
        };
        let (service, id) = (self.clone(), upload_id.clone());
        let job_id = self
            .inner
            .jobs
            .spawn(spec, move |job| async move {
                service
                    .download_pack(&id, &chosen, origin, &imports_dir, &job)
                    .await
                    .map_err(|error| JobFailure::msg(error_chain(&error)))
            })
            .map_err(|error| ModpackError::Io(std::io::Error::other(error.to_string())))?;
        Ok(Fetching { upload_id, job_id })
    }

    /// What a finished fetch turned out to be. `UnknownUpload` until it has finished, and if it failed.
    pub async fn fetched_modpack(&self, upload_id: &str) -> Result<Uploaded, ModpackError> {
        let archive = self.upload_path(upload_id)?;
        let note = FetchNote::read(&archive)
            .await
            .ok_or(ModpackError::UnknownUpload)?;
        if let Some(folder) = note.folder {
            return Ok(Uploaded::ServerFolder { folder });
        }
        let pack = read_pack(archive).await?.ok_or(ModpackError::UnknownUpload)?;
        Ok(Uploaded::Pack {
            upload_id: upload_id.to_owned(),
            pack: pack.info,
            server_pack: note.server_pack,
        })
    }

    async fn choose(&self, request: &FetchPack) -> Result<Chosen, ModpackError> {
        let versions = self.modpack_versions(request.source, &request.project_id).await?;
        let version = match &request.version_id {
            Some(id) => versions.iter().find(|version| version.id == *id),
            None => newest_release(&versions),
        }
        .ok_or_else(|| ContentError::from(SourceError::NotFound))?
        .clone();

        // Needed even with a server version: it says which loader to install.
        if version.file.url.is_empty() {
            return Err(ModpackError::ByHand {
                name: version.name,
                url: version.file.manual_url.unwrap_or_default(),
            });
        }
        let server_pack = if request.source == SourceKind::CurseForge {
            self.inner
                .sources
                .curseforge
                .server_pack(&request.project_id, &version.id)
                .await
                .map_err(ContentError::from)?
                // One only downloadable by hand is no use: the pack's own files will do.
                .filter(|server_pack| !server_pack.file.url.is_empty())
        } else {
            None
        };
        Ok(Chosen { version, server_pack })
    }

    /// The job: download what was chosen (checked against the source's hashes), see what it is.
    async fn download_pack(
        &self,
        upload_id: &str,
        chosen: &Chosen,
        origin: PackOrigin,
        imports_dir: &Path,
        job: &JobContext,
    ) -> Result<(), ModpackError> {
        let archive = self.upload_path(upload_id)?;
        if let Some(uploads) = archive.parent() {
            tokio::fs::create_dir_all(uploads).await?;
        }
        self.download_to(&chosen.version, &archive, job).await?;

        job.step("Reading it");
        let file_name = &chosen.version.file.filename;
        let uploaded = classify(upload_id, &archive, file_name, imports_dir).await?;
        let folder = match uploaded {
            Uploaded::Pack { .. } => None,
            Uploaded::ServerFolder { folder } => Some(folder),
        };
        let server_pack = chosen.server_pack.as_ref().filter(|_| folder.is_none());
        if let Some(server_pack) = server_pack {
            self.download_to(server_pack, &server_pack_path(&archive), job)
                .await?;
        }
        let note = FetchNote {
            origin,
            folder,
            server_pack: server_pack.is_some(),
        };
        let note = serde_json::to_vec(&note).map_err(std::io::Error::other)?;
        tokio::fs::write(FetchNote::path(&archive), note).await?;
        Ok(())
    }

    /// Moved out of the cache, not copied: a pack is never needed from there again, and can be huge.
    async fn download_to(
        &self,
        version: &ContentVersion,
        destination: &Path,
        job: &JobContext,
    ) -> Result<(), ModpackError> {
        job.step(format!("Downloading {}", version.file.filename));
        let cached = install::download(
            self.inner.sources.client(),
            &version.file,
            &self.inner.cache_dir,
            job,
        )
        .await?;
        tokio::fs::rename(&cached, destination).await?;
        Ok(())
    }
}

/// The newest release, or the newest of anything when a pack has no releases.
fn newest_release(versions: &[ContentVersion]) -> Option<&ContentVersion> {
    versions
        .iter()
        .find(|version| version.channel == Channel::Release)
        .or_else(|| versions.first())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ContentFile;

    fn version(id: &str, channel: Channel) -> ContentVersion {
        ContentVersion {
            id: id.into(),
            project_id: "pack".into(),
            name: id.into(),
            version_number: id.into(),
            channel,
            published: None,
            game_versions: vec![],
            loaders: vec![],
            dependencies: vec![],
            file: ContentFile {
                url: String::new(),
                filename: String::new(),
                size: 0,
                sha1: None,
                sha256: None,
                sha512: None,
                manual_url: None,
            },
        }
    }

    #[test]
    fn the_newest_release_is_preferred_over_a_newer_beta() {
        let versions = [
            version("beta", Channel::Beta),
            version("release", Channel::Release),
        ];
        assert_eq!(newest_release(&versions).map(|v| v.id.as_str()), Some("release"));

        let only_betas = [version("new", Channel::Beta), version("old", Channel::Alpha)];
        assert_eq!(newest_release(&only_betas).map(|v| v.id.as_str()), Some("new"));
        assert!(newest_release(&[]).is_none());
    }
}
