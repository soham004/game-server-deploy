//! Moving a server to another version of its pack: see what is available, get the new version (from
//! the pack's source, or uploaded), review what would change, and apply it in a job. Applying takes a
//! full backup first, changes the Minecraft or loader version if the pack asks for that, and then
//! changes only the files the pack owns (see [`super::changes`]).
//!
//! A pack can't move a server to another loader: that is a different server, so make a new one.

use std::{collections::BTreeMap, path::Path};

use bytes::Bytes;
use futures_util::Stream;
use gsd_core::{
    backups::{BackupKind, BackupRequest, BackupScope, BackupService},
    files::{FilesError, SafeRelPath, ServerFiles},
    ids::{InstanceId, JobId, UserId},
    instances::Instance,
    jobs::{JobContext, JobFailure, JobSpec, error_chain},
};
use gsd_minecraft::{MinecraftService, service::PreparedVersionChange, settings::MinecraftServer};
use serde::Serialize;
use sha1::{Digest, Sha1};
use tokio::io::AsyncReadExt;

use super::{
    ModpackError, browse,
    changes::{self, FileChanges, Local, NewFile, PackLock},
    classify_pack,
    desired::Desired,
    format::{Pack, PackInfo},
    modpack_record, read_pack, server_pack_path,
};
use crate::{
    error::ContentError,
    index::INDEX_FOLDER,
    installed::{self, DISABLED},
    model::{Channel, ContentVersion, SourceKind},
    service::ContentService,
    target::Target,
};

/// What a pack server is on, and what its source has.
#[derive(Debug, Clone, Serialize)]
pub struct PackStatus {
    pub source: String,
    pub name: String,
    pub project_id: Option<String>,
    /// Its version now, as the pack names itself.
    pub version: Option<String>,
    /// The newest release on its source, when it differs from the one installed.
    pub update: Option<AvailablePack>,
    /// Why the source couldn't be asked, if it couldn't.
    pub check_failed: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AvailablePack {
    pub version_id: String,
    pub name: String,
}

/// The new version, as it arrived from a client.
#[derive(Debug, Clone, Serialize)]
pub struct UploadedUpdate {
    pub upload_id: String,
    pub pack: PackInfo,
}

/// What updating would do.
#[derive(Debug, Clone, Serialize)]
pub struct UpdatePlan {
    pub name: String,
    pub from: Option<String>,
    pub to: Option<String>,
    /// The Minecraft and loader version the new version needs, when the server isn't on them.
    pub version_change: Option<VersionChange>,
    pub server_pack: bool,
    #[serde(flatten)]
    pub changes: FileChanges,
    /// What the pack lists and this can't install.
    pub left_out: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct VersionChange {
    pub mc_version: String,
    pub loader_version: Option<String>,
}

/// What the user confirmed.
#[derive(Debug, Clone)]
pub struct ApplyUpdate {
    pub upload_id: String,
    pub replace_changed: bool,
    pub by: Option<UserId>,
}

impl ContentService {
    /// A pack server's pack and whether a newer version exists. The lock says which version is
    /// really there: after restoring a backup, `[modpack]` can be ahead of the files.
    pub async fn modpack_status(&self, id: InstanceId) -> Result<PackStatus, ModpackError> {
        let instance = self.inner.instances.get(id).await.map_err(ContentError::from)?;
        let record = instance
            .definition
            .modpack
            .clone()
            .ok_or(ModpackError::NotAPackServer)?;
        let lock = read_lock(&ServerFiles::open(&instance).await.map_err(ContentError::from)?).await;
        let installed = lock
            .as_ref()
            .map_or(record.version_id.clone(), |lock| lock.version_id.clone());
        let mut status = PackStatus {
            source: record.source.clone(),
            name: record.name,
            project_id: record.project_id.clone(),
            version: lock.map_or(record.version, |lock| lock.version),
            update: None,
            check_failed: None,
        };
        let (Some(source), Some(project)) = (source_kind(&record.source), &record.project_id) else {
            return Ok(status);
        };
        let mc_version = MinecraftServer::from_table(&instance.definition.server)
            .map(|server| server.mc_version)
            .unwrap_or_default();
        match self.modpack_versions(source, project).await {
            Ok(versions) => {
                status.update = offered_update(&versions, &mc_version, installed.as_deref()).map(|newest| {
                    AvailablePack {
                        version_id: newest.id.clone(),
                        name: newest.name.clone(),
                    }
                });
            }
            Err(error) => status.check_failed = Some(error.to_string()),
        }
        Ok(status)
    }

    /// Starts downloading a version of the server's pack (its newest release when none is named), in
    /// a job on this server. What follows is [`Self::plan_pack_update`].
    pub async fn fetch_pack_update(
        &self,
        id: InstanceId,
        version_id: Option<String>,
        imports_dir: std::path::PathBuf,
    ) -> Result<browse::Fetching, ModpackError> {
        let instance = self.inner.instances.get(id).await.map_err(ContentError::from)?;
        let record = instance.definition.modpack.ok_or(ModpackError::NotAPackServer)?;
        let (Some(source), Some(project_id)) = (source_kind(&record.source), record.project_id) else {
            return Err(ModpackError::NoSourceToAsk);
        };
        let request = browse::FetchPack {
            source,
            project_id,
            version_id,
            mc_version: None,
        };
        self.fetch_modpack_for(request, imports_dir, Some(id)).await
    }

    /// Stores a new version a client brought. Only a modpack will do here.
    pub async fn upload_pack_update<S, E>(&self, body: S) -> Result<UploadedUpdate, ModpackError>
    where
        S: Stream<Item = Result<Bytes, E>> + Unpin + Send,
        E: std::fmt::Display,
    {
        let (upload_id, path) = self.store_upload(body).await?;
        let pack = classify_pack(path).await?;
        Ok(UploadedUpdate {
            upload_id,
            pack: pack.info,
        })
    }

    /// What updating to the waiting `upload_id` would do. Nothing on the server changes.
    pub async fn plan_pack_update(
        &self,
        id: InstanceId,
        upload_id: &str,
        replace_changed: bool,
        minecraft: &MinecraftService,
    ) -> Result<UpdatePlan, ModpackError> {
        let (instance, lock, pack, archive) = self.update_inputs(id, upload_id).await?;
        let version_change = self.needed_version_change(&instance, &pack, minecraft).await?;
        let files = ServerFiles::open(&instance).await.map_err(ContentError::from)?;
        let staging = archive.with_extension("plan");
        let desired = self.desired(&pack, &archive, &staging, &instance).await;
        let changes = match &desired {
            Ok(desired) => compare_with_server(&files, &lock, desired, replace_changed).await,
            Err(_) => FileChanges::default(),
        };
        let _ = tokio::fs::remove_dir_all(&staging).await;
        let desired = desired?;
        Ok(UpdatePlan {
            name: pack.info.name.clone(),
            from: lock.version,
            to: pack.info.version.clone(),
            version_change: version_change.is_some().then(|| VersionChange {
                mc_version: pack.info.mc_version.clone(),
                loader_version: pack.info.loader_version.clone(),
            }),
            server_pack: tokio::fs::try_exists(server_pack_path(&archive))
                .await
                .unwrap_or(false),
            changes,
            left_out: desired.notes,
        })
    }

    /// Applies the update in a job. The caller makes sure the server is stopped.
    pub async fn start_pack_update(
        &self,
        id: InstanceId,
        request: ApplyUpdate,
        minecraft: &MinecraftService,
        backups: &BackupService,
    ) -> Result<JobId, ModpackError> {
        let (instance, lock, pack, archive) = self.update_inputs(id, &request.upload_id).await?;
        let version_change = self.needed_version_change(&instance, &pack, minecraft).await?;
        let origin = browse::FetchNote::read(&archive).await.map(|note| note.origin);
        let spec = JobSpec {
            kind: "modpack.update",
            title: format!(
                "Updating {} to {}",
                instance.definition.name,
                describe(&pack.info)
            ),
            instance_id: Some(id),
        };
        let (service, minecraft, backups) = (self.clone(), minecraft.clone(), backups.clone());
        let job = self
            .inner
            .jobs
            .spawn(spec, move |job| async move {
                let backup = pack_backup(&instance, &pack.info, request.by, &backups, &job).await?;
                let applied = service
                    .apply_update(
                        &instance,
                        &pack,
                        &lock,
                        origin,
                        &archive,
                        version_change,
                        request.replace_changed,
                        &minecraft,
                        &job,
                    )
                    .await;
                service.changed(id);
                if applied.is_ok() {
                    service.forget_upload(&archive).await;
                }
                applied.map_err(|error| {
                    JobFailure::msg(format!(
                        "{}. The server's files may be partly changed: restore the backup {backup} to go back.",
                        error.0
                    ))
                })
            })
            .map_err(ContentError::from)?;
        Ok(job)
    }

    /// What every step needs: a pack server, with a lock, and a pack on the same loader.
    async fn update_inputs(
        &self,
        id: InstanceId,
        upload_id: &str,
    ) -> Result<(Instance, PackLock, Pack, std::path::PathBuf), ModpackError> {
        let instance = self.inner.instances.get(id).await.map_err(ContentError::from)?;
        if instance.definition.modpack.is_none() {
            return Err(ModpackError::NotAPackServer);
        }
        let files = ServerFiles::open(&instance).await.map_err(ContentError::from)?;
        let lock = read_lock(&files).await.ok_or(ModpackError::NoLock)?;
        let archive = self.upload_path(upload_id)?;
        if !tokio::fs::try_exists(&archive).await.unwrap_or(false) {
            return Err(ModpackError::UnknownUpload);
        }
        let pack = read_pack(archive.clone())
            .await?
            .ok_or(ModpackError::UnknownUpload)?;
        let server = MinecraftServer::from_table(&instance.definition.server)
            .map_err(|error| std::io::Error::other(error.to_string()))?;
        if server.loader != pack.info.loader {
            return Err(ModpackError::ChangesLoader {
                from: server.loader,
                to: pack.info.loader,
            });
        }
        Ok((instance, lock, pack, archive))
    }

    /// The version change the pack needs, checked against the catalog, if any.
    async fn needed_version_change(
        &self,
        instance: &Instance,
        pack: &Pack,
        minecraft: &MinecraftService,
    ) -> Result<Option<PreparedVersionChange>, ModpackError> {
        Ok(minecraft
            .prepare_version_change(
                instance,
                &pack.info.mc_version,
                pack.info.loader_version.as_deref(),
            )
            .await?)
    }

    /// The job's work, after the backup. The error is ready to show.
    #[allow(clippy::too_many_arguments)]
    async fn apply_update(
        &self,
        instance: &Instance,
        pack: &Pack,
        lock: &PackLock,
        origin: Option<browse::PackOrigin>,
        archive: &Path,
        version_change: Option<PreparedVersionChange>,
        replace_changed: bool,
        minecraft: &MinecraftService,
        job: &JobContext,
    ) -> Result<(), ApplyFailed> {
        if let Some(change) = &version_change {
            minecraft
                .apply_version_change(instance, change, job)
                .await
                .map_err(|failure| ApplyFailed(failure.to_string()))?;
        }
        let failed = |error: &dyn std::error::Error| ApplyFailed(error_chain(error));
        let files = ServerFiles::open(instance)
            .await
            .map_err(|error| failed(&ContentError::from(error)))?;
        let staging = archive.with_extension("staging");
        let result = self
            .update_files(
                instance,
                &files,
                pack,
                lock,
                origin.as_ref(),
                archive,
                &staging,
                replace_changed,
                job,
            )
            .await;
        let _ = tokio::fs::remove_dir_all(&staging).await;
        result.map_err(|error| failed(&error))?;

        let record = modpack_record(&pack.info, origin);
        let saved = self
            .inner
            .instances
            .registry()
            .update(instance.id(), |definition| definition.modpack = Some(record))
            .await;
        if let Err(error) = saved {
            job.log(format!("Could not record which version this is: {error}"));
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    async fn update_files(
        &self,
        instance: &Instance,
        files: &ServerFiles,
        pack: &Pack,
        lock: &PackLock,
        origin: Option<&browse::PackOrigin>,
        archive: &Path,
        staging: &Path,
        replace_changed: bool,
        job: &JobContext,
    ) -> Result<(), ModpackError> {
        job.step(format!("Reading what {} brings", describe(&pack.info)));
        let desired = self.desired(pack, archive, staging, instance).await?;
        let changes = compare_with_server(files, lock, &desired, replace_changed).await;
        job.log(format!(
            "{} to add, {} to replace, {} to remove, {} kept as they are",
            changes.add.len(),
            changes.replace.len(),
            changes.remove.len(),
            changes.kept.len(),
        ));
        for kept in &changes.kept {
            job.log(format!("Kept {} ({:?})", kept.path, kept.reason));
        }
        let hashes = self.apply_changes(files, &desired, &changes, job).await?;
        remove_stale_index_entries(instance, files, &changes).await;
        write_lock(
            files,
            &PackLock {
                version_id: origin.map(|origin| origin.version_id.clone()),
                version: pack.info.version.clone(),
                files: hashes,
            },
        )
        .await?;
        self.track_installed(instance, job).await;
        Ok(())
    }

    /// Removes a waiting upload and what came with it.
    pub(super) async fn forget_upload(&self, archive: &Path) {
        let _ = tokio::fs::remove_file(archive).await;
        let _ = tokio::fs::remove_file(browse::FetchNote::path(archive)).await;
        let _ = tokio::fs::remove_file(server_pack_path(archive)).await;
    }

    /// Looks up what is there, so it can be updated on its own too. Only tracking: a lookup that
    /// fails leaves working files, not a failed install.
    pub(super) async fn track_installed(&self, instance: &Instance, job: &JobContext) {
        job.step("Looking up what was installed");
        let server = MinecraftServer::from_table(&instance.definition.server).ok();
        if let Some(target) = server.as_ref().and_then(Target::for_server) {
            match self.identify_on(instance, &target).await {
                Ok(found) => job.log(format!(
                    "Tracking {}; not found on any source: {}",
                    found.identified.len(),
                    found.still_unknown.len()
                )),
                Err(error) => job.log(format!("Could not look the files up: {error}")),
            }
        }
    }
}

/// A failure in the job, already worded for the user.
#[derive(Debug)]
struct ApplyFailed(String);

/// The version to offer, from a list that is newest first: the newest one for the server's Minecraft
/// that is newer than what is installed and at least as mature. A pack publishes for several Minecraft
/// versions at once, so the newest overall can be for an older one, which would mean downgrading the
/// world. And a pack's builds for a new Minecraft are often all betas, so a server on a beta must see
/// newer betas, while one on a release sees only releases.
fn offered_update<'a>(
    versions: &'a [ContentVersion],
    mc_version: &str,
    installed: Option<&str>,
) -> Option<&'a ContentVersion> {
    let least_mature = versions
        .iter()
        .find(|version| Some(version.id.as_str()) == installed)
        .map_or(Channel::Release, |version| version.channel);
    versions
        .iter()
        .take_while(|version| Some(version.id.as_str()) != installed)
        .filter(|version| version.channel <= least_mature)
        .find(|version| version.game_versions.iter().any(|game| game == mc_version))
}

fn source_kind(source: &str) -> Option<SourceKind> {
    match source {
        "modrinth" => Some(SourceKind::Modrinth),
        "curseforge" => Some(SourceKind::CurseForge),
        _ => None,
    }
}

fn describe(info: &PackInfo) -> String {
    info.version
        .as_ref()
        .filter(|version| !version.is_empty())
        .map_or_else(|| info.name.clone(), |version| format!("{} {version}", info.name))
}

async fn pack_backup(
    instance: &Instance,
    info: &PackInfo,
    by: Option<UserId>,
    backups: &BackupService,
    job: &JobContext,
) -> Result<String, JobFailure> {
    job.step("Backing up everything first");
    let request = BackupRequest {
        scope: BackupScope::Full,
        kind: BackupKind::PreUpdate,
        note: Some(format!("Before updating to {}", describe(info))),
        by,
    };
    let record = backups.create(instance, &request, job).await.map_err(|error| {
        JobFailure::msg(format!(
            "the safety backup failed, so nothing was changed: {}",
            error_chain(&error)
        ))
    })?;
    job.log(format!("Saved {}", record.file_name));
    Ok(record.file_name)
}

/// The lock, or `None` if it is missing or can't be read.
pub(super) async fn read_lock(files: &ServerFiles) -> Option<PackLock> {
    let path = SafeRelPath::parse(changes::LOCK_FILE).ok()?;
    let text = files.read_text(path).await.ok()?;
    serde_json::from_str(&text.content).ok()
}

pub(super) async fn write_lock(files: &ServerFiles, lock: &PackLock) -> Result<(), ContentError> {
    let path = SafeRelPath::parse(changes::LOCK_FILE)?;
    let text = serde_json::to_vec_pretty(lock)
        .map_err(std::io::Error::other)
        .map_err(|source| ContentError::Cache {
            path: changes::LOCK_FILE.into(),
            source,
        })?;
    let body = tokio_util::io::ReaderStream::new(std::io::Cursor::new(text));
    files.upload(path, body, true, u64::MAX).await?;
    Ok(())
}

/// How the lock and the new version compare with what is on the server now.
async fn compare_with_server(
    files: &ServerFiles,
    lock: &PackLock,
    desired: &Desired,
    replace_changed: bool,
) -> FileChanges {
    let new: Vec<NewFile<'_>> = desired.new_files();
    let mut local = BTreeMap::new();
    for path in changes::paths_involved(&lock.files, &new) {
        local.insert(path.to_owned(), state_of(files, path).await);
    }
    changes::compare(
        &lock.files,
        &new,
        |path| local.get(path).cloned().unwrap_or(Local::Missing),
        replace_changed,
    )
}

/// A file as it is on the server. One that can't be read counts as changed, so it is left alone.
async fn state_of(files: &ServerFiles, path: &str) -> Local {
    let Ok(parsed) = SafeRelPath::parse(path) else {
        return Local::Missing;
    };
    match files.open_file(parsed).await {
        Ok((mut file, _)) => {
            let mut hasher = Sha1::new();
            let mut buffer = vec![0; 64 * 1024];
            loop {
                match file.read(&mut buffer).await {
                    Ok(0) => break,
                    Ok(read) => hasher.update(&buffer[..read]),
                    Err(_) => return Local::File(String::new()),
                }
            }
            Local::File(hex::encode(hasher.finalize()))
        }
        Err(FilesError::NotFound(_)) => {
            let Ok(turned_off) = SafeRelPath::parse(&format!("{path}{DISABLED}")) else {
                return Local::Missing;
            };
            if files.open_file(turned_off).await.is_ok() {
                Local::TurnedOff
            } else {
                Local::Missing
            }
        }
        Err(_) => Local::File(String::new()),
    }
}

/// A file that was removed or replaced takes its index entry with it, so the Mods tab doesn't show
/// the old version's source. The lookup afterwards names what is there now.
async fn remove_stale_index_entries(instance: &Instance, files: &ServerFiles, changes: &FileChanges) {
    let server = MinecraftServer::from_table(&instance.definition.server).ok();
    let Some(target) = server.as_ref().and_then(Target::for_server) else {
        return;
    };
    let folder = target.kind.folder();
    let Ok(index_folder) = SafeRelPath::parse(&format!("{folder}/{INDEX_FOLDER}")) else {
        return;
    };
    let entries = installed::read_entries(files, &index_folder).await;
    let stale = changes
        .remove
        .iter()
        .chain(&changes.replace)
        .filter_map(|path| path.strip_prefix(folder)?.strip_prefix('/'))
        .filter_map(|name| entries.get(name))
        .filter_map(|(entry, _)| index_folder.join(entry).ok());
    for path in stale {
        let _ = files.delete(vec![path]).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ContentFile;

    fn version(id: &str, channel: Channel, minecraft: &str) -> ContentVersion {
        ContentVersion {
            id: id.into(),
            project_id: "pack".into(),
            name: id.into(),
            version_number: id.into(),
            channel,
            published: None,
            game_versions: vec![minecraft.into()],
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

    fn offered<'a>(versions: &'a [ContentVersion], installed: Option<&str>) -> Option<&'a str> {
        offered_update(versions, "26.3", installed).map(|version| version.id.as_str())
    }

    /// As a real pack lists them: each release for several Minecraft versions.
    #[test]
    fn the_offer_is_the_newest_release_for_this_minecraft_and_only_if_newer() {
        let versions = [
            version("2-for-26.2", Channel::Release, "26.2"),
            version("2-beta", Channel::Beta, "26.3"),
            version("2", Channel::Release, "26.3"),
            version("1-for-26.2", Channel::Release, "26.2"),
            version("1", Channel::Release, "26.3"),
        ];
        assert_eq!(offered(&versions, Some("1")), Some("2"));
        assert_eq!(offered(&versions, Some("2")), None, "up to date");
        assert_eq!(offered(&versions, Some("1-for-26.2")), Some("2"));
        assert_eq!(offered(&versions, None), Some("2"), "unknown: the newest fitting");
        assert_eq!(offered(&versions, Some("removed")), Some("2"));
        assert_eq!(
            offered(&versions[3..], Some("1")),
            None,
            "only older Minecraft is newer"
        );
    }

    /// The real Adrenaline: its builds for the newest Minecraft are betas, the others releases.
    #[test]
    fn a_server_on_a_beta_is_offered_newer_betas_and_one_on_a_release_is_not() {
        let versions = [
            version("3-beta", Channel::Beta, "26.3"),
            version("3-for-26.2", Channel::Release, "26.2"),
            version("2-beta", Channel::Beta, "26.3"),
            version("2-for-26.2", Channel::Release, "26.2"),
            version("1", Channel::Release, "26.3"),
        ];
        assert_eq!(offered(&versions, Some("2-beta")), Some("3-beta"));
        assert_eq!(offered(&versions, Some("3-beta")), None);
        assert_eq!(
            offered(&versions, Some("1")),
            None,
            "a release isn't moved to a beta"
        );
    }
}
