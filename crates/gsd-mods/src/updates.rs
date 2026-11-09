//! Keeping content current: which installed files have a newer version that fits the server
//! (one Modrinth request for all of them, by SHA-1; one Hangar or `CurseForge` request per project),
//! naming files added by hand (they become tracked; on Modrinth by SHA-1, on `CurseForge` by
//! fingerprint; Hangar has no lookup by file), and updating in a job that takes a full backup first
//! and installs any new required dependency.
//!
//! Turned-off files are neither checked nor updated: updating one would turn it back on.

use std::collections::{HashMap, HashSet};

use gsd_core::{
    backups::{BackupKind, BackupRequest, BackupScope, BackupService},
    files::{SafeRelPath, ServerFiles},
    ids::{InstanceId, JobId, UserId},
    jobs::{JobContext, JobFailure, JobSpec},
};
use serde::Serialize;

use crate::{
    error::{ContentError, SourceError},
    index::{INDEX_FOLDER, IndexEntry, Origin},
    install,
    installed::{self, InstalledItem},
    model::{Channel, ContentVersion, DependencyKind, Project, SideSupport, SourceKind},
    resolver::{self, InstallPlan, PlannedInstall, RequiredDependency},
    service::ContentService,
    source::{ForServer, Source},
    target::Target,
};

/// What the update check found.
#[derive(Debug, Clone, Serialize)]
pub struct UpdateCheck {
    pub updates: Vec<AvailableUpdate>,
    /// Files with no known source: "identify" may name them.
    pub unknown: Vec<String>,
}

/// A newer version of an installed file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AvailableUpdate {
    pub file: String,
    pub name: String,
    pub installed_version: Option<String>,
    pub source: SourceKind,
    pub project_id: String,
    pub version: ContentVersion,
}

/// What identifying unknown files did.
#[derive(Debug, Clone, Serialize)]
pub struct Identified {
    /// Names of the projects now known (and tracked in the index).
    pub identified: Vec<String>,
    /// Files Modrinth doesn't know (or the content source is not Modrinth).
    pub still_unknown: Vec<String>,
    /// Files that are another copy of a project already installed: left as they are, since one
    /// project has one index entry. Usually one of the copies should go.
    pub duplicates: Vec<String>,
}

/// What identifying found: by file name, the project and the version the file is.
type Found = HashMap<String, (Project, ContentVersion)>;

impl ContentService {
    /// Newer versions, for every turned-on file with a known source.
    pub async fn check_updates(&self, id: InstanceId) -> Result<UpdateCheck, ContentError> {
        let (instance, target) = self.server(id).await?;
        let items = self.installed(&instance, &target).await?;
        Ok(UpdateCheck {
            updates: self.available(&items, &target).await?,
            unknown: items
                .iter()
                .filter(|item| item.source.is_none())
                .map(|item| item.file.clone())
                .collect(),
        })
    }

    /// Looks up files with no source: on Modrinth by their SHA-1, then what is left on `CurseForge`
    /// by its fingerprint (when a key is set). The ones found get an index entry, so they can be
    /// updated from now on.
    pub async fn identify(&self, id: InstanceId) -> Result<Identified, ContentError> {
        let (instance, target) = self.server(id).await?;
        self.identify_on(&instance, &target).await
    }

    /// [`Self::identify`] for a server already looked up, or one still being set up by a job.
    pub(crate) async fn identify_on(
        &self,
        instance: &gsd_core::instances::Instance,
        target: &Target,
    ) -> Result<Identified, ContentError> {
        let id = instance.id();
        let (tracked, mut unknown): (Vec<InstalledItem>, Vec<InstalledItem>) = self
            .installed(instance, target)
            .await?
            .into_iter()
            .partition(|item| item.source.is_some());
        let mut taken: HashSet<(SourceKind, String)> = tracked
            .iter()
            .filter_map(|item| item.source.as_ref())
            .map(|origin| (origin.source, origin.project_id.clone()))
            .collect();
        // Of two unknown copies of one project, the first by file name is the one tracked.
        unknown.sort_by(|a, b| a.file.cmp(&b.file));
        let files = ServerFiles::open(instance).await?;
        let mut found = self.found_on_modrinth(&unknown).await?;
        found.extend(
            self.found_on_curseforge(&files, target.kind.folder(), &unknown, &found)
                .await,
        );

        let index_folder = SafeRelPath::parse(target.kind.folder())?.join(INDEX_FOLDER)?;
        let mut result = Identified {
            identified: Vec::new(),
            still_unknown: Vec::new(),
            duplicates: Vec::new(),
        };
        for item in unknown {
            let Some((project, version)) = found.get(&item.file) else {
                result.still_unknown.push(item.file);
                continue;
            };
            if !taken.insert((project.source, project.id.clone())) {
                result.duplicates.push(item.file);
                continue;
            }
            let mut entry = IndexEntry::new(project, version);
            entry.filename.clone_from(&item.file);
            install::write_entry(&files, &index_folder, &project.slug, &entry).await?;
            result.identified.push(project.title.clone());
        }
        if !result.identified.is_empty() {
            self.changed(id);
        }
        Ok(result)
    }

    /// What Modrinth knows of `unknown`, by file name.
    async fn found_on_modrinth(&self, unknown: &[InstalledItem]) -> Result<Found, ContentError> {
        let modrinth = &self.inner.sources.modrinth;
        let sha1s: Vec<String> = unknown.iter().filter_map(|item| item.sha1.clone()).collect();
        let versions = modrinth.identify(&sha1s).await?;
        let ids: Vec<String> = versions
            .values()
            .map(|version| version.project_id.clone())
            .collect();
        let projects = modrinth.projects(&ids).await?;
        Ok(unknown
            .iter()
            .filter_map(|item| {
                let version = versions.get(item.sha1.as_ref()?)?;
                let project = projects.iter().find(|project| project.id == version.project_id)?;
                Some((item.file.clone(), (project.clone(), version.clone())))
            })
            .collect())
    }

    /// What `CurseForge` knows of the files in `unknown` not in `already`, by file name. Trouble here
    /// must not lose what Modrinth found, so it only leaves those files unknown (and a log line).
    async fn found_on_curseforge(
        &self,
        files: &ServerFiles,
        folder: &str,
        unknown: &[InstalledItem],
        already: &Found,
    ) -> Found {
        let curseforge = &self.inner.sources.curseforge;
        if !curseforge.has_key() {
            return Found::new();
        }
        let mut by_fingerprint = Vec::new();
        for item in unknown.iter().filter(|item| !already.contains_key(&item.file)) {
            if let Some(fingerprint) = installed::fingerprint_of(files, folder, item).await {
                by_fingerprint.push((fingerprint, item.file.clone()));
            }
        }
        let fingerprints: Vec<u32> = by_fingerprint
            .iter()
            .map(|(fingerprint, _)| *fingerprint)
            .collect();
        let found = async {
            let versions = curseforge.identify(&fingerprints).await?;
            let ids: Vec<String> = versions
                .values()
                .map(|version| version.project_id.clone())
                .collect();
            let projects = curseforge.projects(&ids).await?;
            Ok::<_, SourceError>((versions, projects))
        }
        .await;
        let (versions, projects) = match found {
            Ok(found) => found,
            Err(error) => {
                tracing::warn!(%error, "could not look files up on CurseForge");
                return Found::new();
            }
        };
        by_fingerprint
            .into_iter()
            .filter_map(|(fingerprint, file)| {
                let version = versions.get(&fingerprint)?;
                let project = projects.iter().find(|project| project.id == version.project_id)?;
                Some((file, (project.clone(), version.clone())))
            })
            .collect()
    }

    /// Updates `files` (all available updates when `None`) in a job: a full backup first, then the
    /// new versions, then any new required dependency.
    pub async fn start_update(
        &self,
        id: InstanceId,
        files: Option<&[String]>,
        backups: &BackupService,
        by: Option<UserId>,
    ) -> Result<(Vec<AvailableUpdate>, JobId), ContentError> {
        let (instance, target) = self.server(id).await?;
        let items = self.installed(&instance, &target).await?;
        let updates: Vec<AvailableUpdate> = self
            .available(&items, &target)
            .await?
            .into_iter()
            .filter(|update| files.is_none_or(|files| files.contains(&update.file)))
            .collect();
        if updates.is_empty() {
            return Err(ContentError::NothingToUpdate);
        }
        let spec = JobSpec {
            kind: "content.update",
            title: format!(
                "Updating {} on {}",
                plural(updates.len(), target.kind.folder()),
                instance.definition.name
            ),
            instance_id: Some(id),
        };
        let (service, backups, chosen) = (self.clone(), backups.clone(), updates.clone());
        let job = self.inner.jobs.spawn(spec, move |job| async move {
            let result = service
                .update(&instance, &target, &items, &chosen, &backups, by, &job)
                .await;
            service.changed(instance.id());
            result.map_err(|error| JobFailure::msg(gsd_core::jobs::error_chain(&error)))
        })?;
        Ok((updates, job))
    }

    /// The job's work.
    #[allow(clippy::too_many_arguments)]
    async fn update(
        &self,
        instance: &gsd_core::instances::Instance,
        target: &Target,
        items: &[InstalledItem],
        updates: &[AvailableUpdate],
        backups: &BackupService,
        by: Option<UserId>,
        job: &JobContext,
    ) -> Result<(), ContentError> {
        job.step("Backing up everything first");
        let request = BackupRequest {
            scope: BackupScope::Full,
            kind: BackupKind::PreUpdate,
            note: Some(format!(
                "Before updating {}",
                plural(updates.len(), target.kind.folder())
            )),
            by,
        };
        let backup = backups
            .create(instance, &request, job)
            .await
            .map_err(ContentError::Backup)?;
        job.log(format!("Saved {}", backup.file_name));

        let plan = self.update_plan(items, updates, target).await?;
        let files = ServerFiles::open(instance).await?;
        let client = self.inner.sources.client();
        install::run(
            &files,
            target.kind.folder(),
            &plan,
            client,
            &self.inner.cache_dir,
            job,
        )
        .await
    }

    /// The updated versions replacing their files, plus required dependencies they newly need.
    async fn update_plan(
        &self,
        items: &[InstalledItem],
        updates: &[AvailableUpdate],
        target: &Target,
    ) -> Result<InstallPlan, ContentError> {
        let mut plan = InstallPlan::default();
        for kind in SourceKind::ALL {
            let chosen: Vec<&AvailableUpdate> =
                updates.iter().filter(|update| update.source == kind).collect();
            if !chosen.is_empty() {
                let source = self.inner.sources.get(kind);
                plan.absorb(source_update_plan(source, kind, items, &chosen, target).await?);
            }
        }
        Ok(plan)
    }

    /// Newer versions for the turned-on files with a known source, by name.
    async fn available(
        &self,
        items: &[InstalledItem],
        target: &Target,
    ) -> Result<Vec<AvailableUpdate>, ContentError> {
        let mut updates = self.modrinth_updates(items, target).await?;
        for kind in [SourceKind::Hangar, SourceKind::CurseForge] {
            if self.inner.sources.get(kind).serves(target) {
                updates.extend(self.updates_by_project(kind, items, target).await?);
            }
        }
        updates.sort_by_key(|update| update.name.to_lowercase());
        Ok(updates)
    }

    /// Asks the source about each tracked project in turn. A project the source no longer has
    /// simply has no updates; it shouldn't hide the others'.
    async fn updates_by_project(
        &self,
        kind: SourceKind,
        items: &[InstalledItem],
        target: &Target,
    ) -> Result<Vec<AvailableUpdate>, ContentError> {
        let source = self.inner.sources.get(kind);
        let mut updates = Vec::new();
        for item in items.iter().filter(|item| item.enabled) {
            let Some(origin) = item.source.as_ref().filter(|origin| origin.source == kind) else {
                continue;
            };
            match source.versions(&origin.project_id, target).await {
                Ok(versions) => updates.extend(update_from(item, origin, &versions)),
                Err(SourceError::NotFound) => {
                    tracing::warn!(file = %item.file, source = kind.as_str(), "the project is gone from its source");
                }
                Err(error) => return Err(error.into()),
            }
        }
        Ok(updates)
    }

    /// Newer versions of what came from Modrinth: one request for all of them, by SHA-1.
    async fn modrinth_updates(
        &self,
        items: &[InstalledItem],
        target: &Target,
    ) -> Result<Vec<AvailableUpdate>, ContentError> {
        let modrinth = &self.inner.sources.modrinth;
        let tracked: Vec<&InstalledItem> = items
            .iter()
            .filter(|item| {
                item.enabled
                    && item.sha1.is_some()
                    && item
                        .source
                        .as_ref()
                        .is_some_and(|origin| origin.source == SourceKind::Modrinth)
            })
            .collect();
        let sha1s: Vec<String> = tracked.iter().filter_map(|item| item.sha1.clone()).collect();
        let latest = modrinth.latest(&sha1s, target).await?;
        let outdated: Vec<(&InstalledItem, &ContentVersion)> = tracked
            .into_iter()
            .filter_map(|item| {
                let origin = item.source.as_ref()?;
                let newest = latest.get(item.sha1.as_ref()?)?;
                (newest.project_id == origin.project_id && newest.id != origin.version_id)
                    .then_some((item, newest))
            })
            .collect();
        // The installed version as Modrinth numbers it, so both sides of "old → new" read alike
        // (a jar's own version is often shorter, "0.26.2" against "mc26.3-0.26.3-fabric").
        let outdated_sha1s: Vec<String> = outdated
            .iter()
            .filter_map(|(item, _)| item.sha1.clone())
            .collect();
        let installed = modrinth.identify(&outdated_sha1s).await?;
        Ok(outdated
            .into_iter()
            .map(|(item, newest)| AvailableUpdate {
                file: item.file.clone(),
                name: item.name.clone(),
                installed_version: item
                    .sha1
                    .as_ref()
                    .and_then(|sha1| installed.get(sha1))
                    .map(|version| version.version_number.clone())
                    .or_else(|| item.version.clone()),
                source: SourceKind::Modrinth,
                project_id: newest.project_id.clone(),
                version: newest.clone(),
            })
            .collect())
    }
}

/// The update for `item`, given its project's versions that fit (newest first), if there is one.
fn update_from(
    item: &InstalledItem,
    origin: &Origin,
    versions: &[ContentVersion],
) -> Option<AvailableUpdate> {
    let newer = newer_than(versions, &origin.version_id)?;
    Some(AvailableUpdate {
        file: item.file.clone(),
        name: item.name.clone(),
        installed_version: versions
            .iter()
            .find(|version| version.id == origin.version_id)
            .map(|version| version.version_number.clone())
            .or_else(|| item.version.clone()),
        source: origin.source,
        project_id: origin.project_id.clone(),
        version: newer.clone(),
    })
}

/// What to offer instead of `installed`: the newest release published after it. A snapshot is only
/// offered to someone already on a snapshot, so a chosen release never turns into one. An installed
/// version that isn't listed (too old for the page, or for another Minecraft version) counts as
/// older than everything.
fn newer_than<'a>(versions: &'a [ContentVersion], installed: &str) -> Option<&'a ContentVersion> {
    let position = versions.iter().position(|version| version.id == installed);
    let newer = &versions[..position.unwrap_or(versions.len())];
    let release = newer.iter().find(|version| version.channel == Channel::Release);
    let on_snapshot = position.is_some_and(|at| versions[at].channel != Channel::Release);
    if on_snapshot || position.is_none() {
        release.or_else(|| newer.first())
    } else {
        release
    }
}

/// `updates` from one source replacing their files, plus required dependencies they newly need
/// (from the same source, since that is where their ids mean anything).
async fn source_update_plan(
    source: Source<'_>,
    kind: SourceKind,
    items: &[InstalledItem],
    updates: &[&AvailableUpdate],
    target: &Target,
) -> Result<InstallPlan, ContentError> {
    let ids: Vec<String> = updates.iter().map(|update| update.project_id.clone()).collect();
    let projects = source.projects(&ids).await?;
    let mut plan = InstallPlan::default();
    for update in updates {
        let project = projects
            .iter()
            .find(|project| project.id == update.project_id)
            .cloned()
            .unwrap_or_else(|| fallback_project(update));
        plan.steps.push(PlannedInstall {
            project,
            version: update.version.clone(),
            required_by: None,
            replaces: Some(update.file.clone()),
        });
    }
    let needed: Vec<RequiredDependency> = plan
        .steps
        .iter()
        .flat_map(|step| {
            step.version
                .dependencies
                .iter()
                .filter(|dependency| dependency.kind == DependencyKind::Required)
                .filter_map(|dependency| {
                    Some(RequiredDependency {
                        project_id: dependency.project_id.clone()?,
                        version_id: dependency.version_id.clone(),
                        required_by: step.project.title.clone(),
                    })
                })
        })
        .collect();
    let installed = installed::by_project(items, kind);
    let catalog = ForServer { source, target };
    plan.absorb(resolver::plan_required(&catalog, &installed, needed).await?);
    Ok(plan)
}

/// When the source didn't return the project: enough to install and index the update.
fn fallback_project(update: &AvailableUpdate) -> Project {
    Project {
        source: update.source,
        id: update.project_id.clone(),
        slug: update.project_id.to_lowercase(),
        title: update.name.clone(),
        description: String::new(),
        body: String::new(),
        icon_url: None,
        server_side: SideSupport::Unknown,
        client_side: SideSupport::Unknown,
        page_url: String::new(),
    }
}

/// "1 mod", "3 plugins" (from the folder name, which is plural).
fn plural(count: usize, folder: &str) -> String {
    let noun = folder.trim_end_matches('s');
    format!("{count} {noun}{}", if count == 1 { "" } else { "s" })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use crate::model::ContentFile;

    fn version(id: &str, channel: Channel) -> ContentVersion {
        ContentVersion {
            id: id.into(),
            project_id: "4".into(),
            name: id.into(),
            version_number: id.into(),
            channel,
            published: None,
            game_versions: vec!["26.3".into()],
            loaders: vec!["paper".into()],
            dependencies: vec![],
            file: ContentFile {
                url: String::new(),
                filename: format!("{id}.jar"),
                size: 1,
                sha1: None,
                sha256: None,
                sha512: None,
                manual_url: None,
            },
        }
    }

    fn offered<'a>(versions: &'a [ContentVersion], installed: &str) -> Option<&'a str> {
        newer_than(versions, installed).map(|version| version.id.as_str())
    }

    #[test]
    fn the_newest_release_after_the_installed_one_is_offered() {
        let versions = [
            version("snapshot-3", Channel::Beta),
            version("release-2", Channel::Release),
            version("release-1", Channel::Release),
        ];
        assert_eq!(offered(&versions, "release-1"), Some("release-2"));
        assert_eq!(offered(&versions, "release-2"), None, "no snapshot for a release");
        assert_eq!(offered(&versions, "snapshot-3"), None, "the newest");
    }

    #[test]
    fn someone_on_a_snapshot_gets_newer_snapshots_or_a_release() {
        let versions = [
            version("snapshot-4", Channel::Beta),
            version("snapshot-3", Channel::Beta),
            version("release-2", Channel::Release),
        ];
        assert_eq!(offered(&versions, "snapshot-3"), Some("snapshot-4"));
        let released = [
            version("release-5", Channel::Release),
            version("snapshot-4", Channel::Beta),
        ];
        assert_eq!(offered(&released, "snapshot-4"), Some("release-5"));
    }

    #[test]
    fn a_version_no_longer_listed_counts_as_older() {
        let versions = [
            version("snapshot-3", Channel::Beta),
            version("release-2", Channel::Release),
        ];
        assert_eq!(offered(&versions, "gone"), Some("release-2"));
        let only_snapshots = [version("snapshot-3", Channel::Beta)];
        assert_eq!(offered(&only_snapshots, "gone"), Some("snapshot-3"));
        assert_eq!(offered(&[], "gone"), None, "nothing fits this server");
    }
}
