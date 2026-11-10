//! A server's worlds: every folder with a `level.dat` (or the files a world that broke leaves behind:
//! `level.dat_old`, `session.lock`), the `level-name` folder whatever it holds, and what can be done
//! with them. A world that won't load is listed with why, so it can still be deleted from here.
//!
//! - **Make active:** sets `level-name`; a running server switches when it restarts.
//! - **Download:** the folder as a zip, while the server is stopped (a running one writes as it goes;
//!   a backup gives a consistent copy of a running server).
//! - **Upload:** a zip becomes a new world folder. It must hold a `level.dat`, at its top or in the one
//!   folder it contains.
//! - **Delete / reset** (stopped only): a job that backs up first (`pre_delete`), then moves the folders
//!   to the server's trash. Any world can be deleted, the active one too: it is often the one that
//!   won't load, and the next start makes a new one under its name. Reset is for the active world,
//!   and can also set the seed the new one gets.
//!
//! Older Paper and Purpur versions keep a world's nether and end in `<world>_nether` and `<world>_the_end`:
//! those go with their world, as its parts. (A real Paper 26.3 server keeps them inside the world folder, as
//! vanilla does.)

use std::collections::BTreeSet;

use bytes::Bytes;
use futures_util::Stream;
use gsd_core::{
    backups::{BackupKind, BackupRequest, BackupScope, BackupService},
    files::{EntryKind, ExtractLimits, FilesError, SafeRelPath, ServerFiles},
    ids::{InstanceId, JobId, UserId},
    instances::{Instance, InstanceError, InstanceService},
    jobs::{JobError, JobFailure, JobRunner, JobSpec, error_chain},
};
use serde::Serialize;

use super::{
    info::{self, WorldFacts},
    is_plain_folder_name, level_name_in,
};
use crate::{
    nbt::{self, NbtError},
    properties::PropertiesDoc,
};

const PROPERTIES: &str = "server.properties";
const LEVEL_DAT: &str = "level.dat";
/// Files only a world folder holds. The game keeps the last `level.dat` as `level.dat_old` and locks the
/// world with `session.lock` while it plays; a world that broke (a crash while saving, a first start
/// that never finished) can be left with these and no `level.dat`.
const WORLD_MARKERS: [&str; 3] = [LEVEL_DAT, "level.dat_old", "session.lock"];
const WORLD_GEN: &str = "data/minecraft/world_gen_settings.dat";
const PART_SUFFIXES: [&str; 2] = ["_nether", "_the_end"];
/// The largest world zip taken.
pub const MAX_UPLOAD_BYTES: u64 = 16 * 1024 * 1024 * 1024;
/// The longest seed taken; the game hashes text seeds, so any length works, but nobody types more.
const MAX_SEED_CHARS: usize = 64;
/// Folders a zip made on a Mac carries along, which aren't part of the world.
const MAC_FOLDER: &str = "__MACOSX";

#[derive(Debug, Clone, Serialize)]
pub struct WorldsView {
    /// Running (or starting, stopping): download, delete and reset wait for a stop.
    pub running: bool,
    /// `level-name`: the world the server plays.
    pub active: String,
    pub worlds: Vec<World>,
}

#[derive(Debug, Clone, Serialize)]
pub struct World {
    pub folder: String,
    pub active: bool,
    /// Its `_nether` and `_the_end` folders, where it has them.
    pub parts: Vec<String>,
    /// All its folders together.
    pub size_bytes: u64,
    #[serde(flatten)]
    pub facts: WorldFacts,
    /// Why its `level.dat` couldn't be read, or that it has none, as a sentence to show.
    pub problem: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum WorldError {
    #[error("there is no world called \"{0}\" here")]
    NotFound(String),
    #[error("\"{0}\" can't be a folder name")]
    InvalidName(String),
    #[error("there is already a folder called \"{0}\"")]
    Exists(String),
    #[error("stop the server first")]
    Running,
    #[error("wait until what the server is doing has finished")]
    Busy,
    #[error("only the world the server plays can be reset; delete the others")]
    NotActive,
    #[error("that zip has no level.dat, at its top or in the one folder it holds, so it isn't a world")]
    NoLevelDat,
    #[error("a seed is one line of at most {MAX_SEED_CHARS} characters")]
    BadSeed,
    #[error(transparent)]
    Files(#[from] FilesError),
    #[error(transparent)]
    Instance(#[from] InstanceError),
    #[error(transparent)]
    Job(#[from] JobError),
}

/// What a reset does about the seed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NewSeed {
    /// Whatever `level-seed` says now.
    Keep,
    /// A random one: `level-seed` is cleared.
    Random,
    Given(String),
}

impl NewSeed {
    /// From a request's `seed`: none keeps it, empty means random.
    pub fn from_request(seed: Option<String>) -> Result<Self, WorldError> {
        match seed.map(|seed| seed.trim().to_owned()) {
            None => Ok(Self::Keep),
            Some(seed) if seed.is_empty() => Ok(Self::Random),
            Some(seed) if seed.chars().count() > MAX_SEED_CHARS || seed.chars().any(char::is_control) => {
                Err(WorldError::BadSeed)
            }
            Some(seed) => Ok(Self::Given(seed)),
        }
    }
}

#[derive(Debug, Clone)]
pub struct WorldService {
    instances: InstanceService,
    backups: BackupService,
    jobs: JobRunner,
}

impl WorldService {
    pub fn new(instances: InstanceService, backups: BackupService, jobs: JobRunner) -> Self {
        Self {
            instances,
            backups,
            jobs,
        }
    }

    pub async fn list(&self, id: InstanceId) -> Result<WorldsView, WorldError> {
        let instance = self.instances.get(id).await?;
        let files = ServerFiles::open(&instance).await?;
        let active = level_name(&files).await?;
        let folders = world_folders(&files, &active).await?;
        let mut worlds = Vec::new();
        for folder in &folders {
            if is_part(folder, &folders) {
                continue;
            }
            let parts: Vec<String> = PART_SUFFIXES
                .iter()
                .map(|suffix| format!("{folder}{suffix}"))
                .filter(|part| folders.contains(part))
                .collect();
            let mut size_bytes = 0;
            for name in std::iter::once(folder).chain(&parts) {
                size_bytes += files.folder_size(path(name)?).await?;
            }
            let (facts, problem) = match read_facts(&files, folder).await {
                Ok(facts) => (facts, None),
                Err(error) => (WorldFacts::default(), Some(error)),
            };
            worlds.push(World {
                active: *folder == active,
                folder: folder.clone(),
                parts,
                size_bytes,
                facts,
                problem,
            });
        }
        worlds.sort_by(|a, b| b.active.cmp(&a.active).then_with(|| a.folder.cmp(&b.folder)));
        Ok(WorldsView {
            running: self.instances.state(id).is_active(),
            active,
            worlds,
        })
    }

    /// Makes `folder` the world the server plays. Returns whether a restart is needed for it to.
    pub async fn activate(&self, id: InstanceId, folder: &str) -> Result<bool, WorldError> {
        let view = self.list(id).await?;
        known(&view, folder)?;
        let instance = self.instances.get(id).await?;
        set_property(&ServerFiles::open(&instance).await?, "level-name", folder).await?;
        Ok(view.running)
    }

    /// A world folder (or one of its parts) as a zip, from a stopped server.
    pub async fn download(
        &self,
        id: InstanceId,
        folder: &str,
    ) -> Result<impl Stream<Item = std::io::Result<Bytes>> + use<>, WorldError> {
        let view = self.list(id).await?;
        let found = view
            .worlds
            .iter()
            .any(|world| world.folder == folder || world.parts.iter().any(|part| part == folder));
        if !found {
            return Err(WorldError::NotFound(folder.to_owned()));
        }
        if view.running {
            return Err(WorldError::Running);
        }
        let instance = self.instances.get(id).await?;
        Ok(ServerFiles::open(&instance).await?.zip_folder(path(folder)?))
    }

    /// Unpacks an uploaded zip as the new world `folder`, and makes it active if asked (`activate`).
    pub async fn upload<S, E>(
        &self,
        id: InstanceId,
        folder: &str,
        body: S,
        activate: bool,
    ) -> Result<World, WorldError>
    where
        S: Stream<Item = Result<Bytes, E>> + Unpin + Send,
        E: std::fmt::Display,
    {
        if !is_plain_folder_name(folder) || folder.starts_with('.') || folder.len() > 64 {
            return Err(WorldError::InvalidName(folder.to_owned()));
        }
        if self.instances.is_busy(id) {
            return Err(WorldError::Busy);
        }
        let instance = self.instances.get(id).await?;
        let files = ServerFiles::open(&instance).await?;
        let target = path(folder)?;
        if files
            .list(SafeRelPath::root())
            .await?
            .iter()
            .any(|entry| entry.name == folder)
        {
            return Err(WorldError::Exists(folder.to_owned()));
        }
        let zip = path(&format!(".gsd-world-upload-{folder}.zip"))?;
        files.upload(zip.clone(), body, true, MAX_UPLOAD_BYTES).await?;
        let unpacked = async {
            files
                .extract(zip.clone(), target.clone(), ExtractLimits::default())
                .await?;
            hoist_single_folder(&files, folder).await
        }
        .await;
        let _ = files.remove_file(zip).await;
        if let Err(error) = unpacked {
            // What was unpacked isn't a world: it goes to the trash, not left lying around.
            let _ = files.delete(vec![target]).await;
            return Err(error);
        }
        if activate {
            set_property(&files, "level-name", folder).await?;
        }
        self.list(id)
            .await?
            .worlds
            .into_iter()
            .find(|world| world.folder == folder)
            .ok_or_else(|| WorldError::NotFound(folder.to_owned()))
    }

    /// Backs up, then deletes a world (a job). Deleting the active one leaves `level-name` as it is, so the
    /// next start makes a new world under that name, unless another is made active first.
    pub async fn delete(
        &self,
        id: InstanceId,
        folder: &str,
        by: Option<UserId>,
    ) -> Result<JobId, WorldError> {
        let (instance, world) = self.removable(id, folder).await?;
        let request = BackupRequest {
            // A world backup holds the active world and its parts; backups can't yet hold just another
            // folder, so deleting any other world takes a full one.
            scope: if world.active {
                BackupScope::World
            } else {
                BackupScope::Full
            },
            kind: BackupKind::PreDelete,
            note: Some(format!("Before deleting the world {folder}")),
            by,
        };
        self.remove_job(
            instance,
            world,
            request,
            NewSeed::Keep,
            "world.delete",
            format!("Deleting the world {folder}"),
        )
    }

    /// Backs up, then removes the active world, so the next start makes a new one (a job).
    pub async fn reset(
        &self,
        id: InstanceId,
        folder: &str,
        seed: NewSeed,
        by: Option<UserId>,
    ) -> Result<JobId, WorldError> {
        let (instance, world) = self.removable(id, folder).await?;
        if !world.active {
            return Err(WorldError::NotActive);
        }
        let request = BackupRequest {
            scope: BackupScope::World,
            kind: BackupKind::PreDelete,
            note: Some(format!("Before resetting the world {}", world.folder)),
            by,
        };
        let title = format!("Resetting the world {}", world.folder);
        self.remove_job(instance, world, request, seed, "world.reset", title)
    }

    /// A world that can be removed now: there, and the server stopped and idle.
    async fn removable(&self, id: InstanceId, folder: &str) -> Result<(Instance, World), WorldError> {
        let view = self.list(id).await?;
        let world = known(&view, folder)?.clone();
        if view.running {
            return Err(WorldError::Running);
        }
        if self.instances.is_busy(id) {
            return Err(WorldError::Busy);
        }
        Ok((self.instances.get(id).await?, world))
    }

    fn remove_job(
        &self,
        instance: Instance,
        world: World,
        backup: BackupRequest,
        seed: NewSeed,
        kind: &'static str,
        title: String,
    ) -> Result<JobId, WorldError> {
        let spec = JobSpec {
            kind,
            title,
            instance_id: Some(instance.id()),
        };
        let (backups, instances) = (self.backups.clone(), self.instances.clone());
        Ok(self.jobs.spawn(spec, move |job| async move {
            // Checked again under the job's lock: the server may have been started meanwhile.
            if instances.state(instance.id()).is_active() {
                return Err(JobFailure::msg("the server was started; stop it first"));
            }
            job.step("Backing up first");
            let record = backups.create(&instance, &backup, &job).await.map_err(|error| {
                JobFailure::msg(format!(
                    "the backup failed, so nothing was deleted: {}",
                    error_chain(&error)
                ))
            })?;
            job.log(format!("Saved {}", record.file_name));
            job.step("Moving the world to the trash");
            let files = ServerFiles::open(&instance)
                .await
                .map_err(|e| JobFailure::msg(e.to_string()))?;
            let folders = std::iter::once(&world.folder)
                .chain(&world.parts)
                .map(|name| path(name))
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| JobFailure::msg(e.to_string()))?;
            files
                .delete(folders)
                .await
                .map_err(|e| JobFailure::msg(e.to_string()))?;
            match seed {
                NewSeed::Keep => {}
                NewSeed::Random => set_property(&files, "level-seed", "")
                    .await
                    .map_err(|e| JobFailure::msg(e.to_string()))?,
                NewSeed::Given(seed) => set_property(&files, "level-seed", &seed)
                    .await
                    .map_err(|e| JobFailure::msg(e.to_string()))?,
            }
            job.log(format!("{} is in the backup {}", world.folder, record.file_name));
            Ok(())
        })?)
    }
}

fn known<'a>(view: &'a WorldsView, folder: &str) -> Result<&'a World, WorldError> {
    view.worlds
        .iter()
        .find(|world| world.folder == folder)
        .ok_or_else(|| WorldError::NotFound(folder.to_owned()))
}

fn path(name: &str) -> Result<SafeRelPath, WorldError> {
    SafeRelPath::parse(name).map_err(|_| WorldError::InvalidName(name.to_owned()))
}

/// Folders at the top of the server folder that are worlds: those holding one of [`WORLD_MARKERS`], and
/// the `active` one whatever it holds (the server plays it, so it is a world even when it is broken).
async fn world_folders(files: &ServerFiles, active: &str) -> Result<BTreeSet<String>, WorldError> {
    let mut worlds = BTreeSet::new();
    for entry in files.list(SafeRelPath::root()).await? {
        if entry.kind != EntryKind::Directory || entry.name.starts_with('.') {
            continue;
        }
        if entry.name == active
            || files
                .list(entry.path.clone())
                .await?
                .iter()
                .any(|inner| inner.kind == EntryKind::File && WORLD_MARKERS.contains(&inner.name.as_str()))
        {
            worlds.insert(entry.name);
        }
    }
    Ok(worlds)
}

/// `world_nether` is a part of `world` when both are worlds.
fn is_part(folder: &str, folders: &BTreeSet<String>) -> bool {
    PART_SUFFIXES.iter().any(|suffix| {
        folder
            .strip_suffix(suffix)
            .is_some_and(|base| !base.is_empty() && folders.contains(base))
    })
}

/// What a world's files say about it, or why they can't, as a sentence to show.
async fn read_facts(files: &ServerFiles, folder: &str) -> Result<WorldFacts, String> {
    let read = |name: String| async move {
        let file = path(&name).map_err(|e| e.to_string())?;
        match files.read_bytes(file).await {
            Ok((bytes, _)) => nbt::read_file(&bytes)
                .map(|(_, root)| Some(root))
                .map_err(|e: NbtError| e.to_string()),
            Err(FilesError::NotFound(_)) => Ok(None),
            Err(error) => Err(error.to_string()),
        }
    };
    let level = read(format!("{folder}/{LEVEL_DAT}"))
        .await
        .map_err(|error| format!("its level.dat can't be read: {error}"))?
        .ok_or_else(|| "it has no level.dat".to_owned())?;
    // Its own file since 1.21.11; one that can't be read only costs the seed.
    let world_gen = read(format!("{folder}/{WORLD_GEN}")).await.ok().flatten();
    Ok(info::facts(&level, world_gen.as_ref()))
}

/// A zip that held one folder with the world inside: that folder's contents become the world's.
async fn hoist_single_folder(files: &ServerFiles, folder: &str) -> Result<(), WorldError> {
    let top = files.list(path(folder)?).await?;
    if top.iter().any(|entry| entry.name == LEVEL_DAT) {
        return Ok(());
    }
    let inner: Vec<_> = top.iter().filter(|entry| entry.name != MAC_FOLDER).collect();
    let [only] = inner.as_slice() else {
        return Err(WorldError::NoLevelDat);
    };
    if only.kind != EntryKind::Directory
        || !files
            .list(only.path.clone())
            .await?
            .iter()
            .any(|entry| entry.name == LEVEL_DAT)
    {
        return Err(WorldError::NoLevelDat);
    }
    let moved = path(&format!(".gsd-world-{folder}"))?;
    files.rename(only.path.clone(), moved.clone()).await?;
    files.delete(vec![path(folder)?]).await?;
    files.rename(moved, path(folder)?).await?;
    Ok(())
}

async fn level_name(files: &ServerFiles) -> Result<String, WorldError> {
    let file = path(PROPERTIES)?;
    let text = match files.read_text(file).await {
        Ok(file) => file.content,
        Err(FilesError::NotFound(_)) => String::new(),
        Err(error) => return Err(error.into()),
    };
    Ok(level_name_in(&PropertiesDoc::parse(&text)))
}

async fn set_property(files: &ServerFiles, key: &str, value: &str) -> Result<(), WorldError> {
    let file = path(PROPERTIES)?;
    let (mut doc, revision) = match files.read_text(file.clone()).await {
        Ok(text) => (PropertiesDoc::parse(&text.content), Some(text.revision)),
        Err(FilesError::NotFound(_)) => (PropertiesDoc::parse(""), None),
        Err(error) => return Err(error.into()),
    };
    doc.set(key, value);
    files.write_text(file, doc.render(), revision).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn paper_dimension_folders_belong_to_their_world() {
        let folders: BTreeSet<String> = [
            "world",
            "world_nether",
            "world_the_end",
            "creative_nether",
            "_nether",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect();
        assert!(is_part("world_nether", &folders));
        assert!(is_part("world_the_end", &folders));
        assert!(!is_part("world", &folders));
        assert!(
            !is_part("creative_nether", &folders),
            "there is no world called creative"
        );
        assert!(!is_part("_nether", &folders));
    }

    #[test]
    fn seeds_from_requests() {
        assert_eq!(NewSeed::from_request(None).unwrap(), NewSeed::Keep);
        assert_eq!(NewSeed::from_request(Some("  ".into())).unwrap(), NewSeed::Random);
        assert_eq!(
            NewSeed::from_request(Some(" -42 ".into())).unwrap(),
            NewSeed::Given("-42".into())
        );
        assert_eq!(
            NewSeed::from_request(Some("glacier".into())).unwrap(),
            NewSeed::Given("glacier".into())
        );
        assert!(NewSeed::from_request(Some("a\nb".into())).is_err());
        assert!(NewSeed::from_request(Some("x".repeat(65))).is_err());
    }
}
