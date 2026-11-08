//! Keeping content current: which installed files have a newer version that fits the server
//! (one Modrinth request for all of them, by SHA-1), naming files added by hand (they become
//! tracked), and updating in a job that takes a full backup first and installs any new required
//! dependency.
//!
//! Turned-off files are neither checked nor updated: updating one would turn it back on.

use std::collections::HashSet;

use gsd_core::{
    backups::{BackupKind, BackupRequest, BackupScope, BackupService},
    files::{SafeRelPath, ServerFiles},
    ids::{InstanceId, JobId, UserId},
    jobs::{JobContext, JobFailure, JobSpec},
};
use serde::Serialize;

use crate::{
    error::ContentError,
    index::{INDEX_FOLDER, IndexEntry},
    install,
    installed::{self, InstalledItem},
    model::{ContentVersion, Project},
    resolver::{self, InstallPlan, PlannedInstall},
    service::{ContentService, ForServer},
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

impl ContentService {
    /// Newer versions, for every turned-on file that came from Modrinth.
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

    /// Looks up files with no source on Modrinth by their hash; the ones found get an index
    /// entry, so they can be updated from now on.
    pub async fn identify(&self, id: InstanceId) -> Result<Identified, ContentError> {
        let (instance, target) = self.server(id).await?;
        let (tracked, mut unknown): (Vec<InstalledItem>, Vec<InstalledItem>) = self
            .installed(&instance, &target)
            .await?
            .into_iter()
            .partition(|item| item.source.is_some());
        let mut taken: HashSet<String> = installed::by_project(&tracked).into_keys().collect();
        // Of two unknown copies of one project, the first by file name is the one tracked.
        unknown.sort_by(|a, b| a.file.cmp(&b.file));
        let sha1s: Vec<String> = unknown.iter().filter_map(|item| item.sha1.clone()).collect();
        let found = self.inner.modrinth.identify(&sha1s).await?;
        let project_ids: Vec<String> = found.values().map(|version| version.project_id.clone()).collect();
        let projects = self.inner.modrinth.projects(&project_ids).await?;

        let files = ServerFiles::open(&instance).await?;
        let index_folder = SafeRelPath::parse(target.kind.folder())?.join(INDEX_FOLDER)?;
        let mut result = Identified {
            identified: Vec::new(),
            still_unknown: Vec::new(),
            duplicates: Vec::new(),
        };
        for item in unknown {
            let known = item.sha1.as_ref().and_then(|sha1| found.get(sha1));
            let project = known.and_then(|version| projects.iter().find(|p| p.id == version.project_id));
            let (Some(version), Some(project)) = (known, project) else {
                result.still_unknown.push(item.file);
                continue;
            };
            if !taken.insert(project.id.clone()) {
                result.duplicates.push(item.file);
                continue;
            }
            let mut entry = IndexEntry::modrinth(&project.title, version, project.client_side);
            entry.filename.clone_from(&item.file);
            install::write_entry(&files, &index_folder, &project.slug, &entry).await?;
            result.identified.push(project.title.clone());
        }
        if !result.identified.is_empty() {
            self.changed(id);
        }
        Ok(result)
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
        let client = self.inner.modrinth.client();
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
        let ids: Vec<String> = updates.iter().map(|update| update.project_id.clone()).collect();
        let projects = self.inner.modrinth.projects(&ids).await?;
        let mut plan = InstallPlan {
            steps: Vec::new(),
            already_installed: Vec::new(),
        };
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
        let installed = installed::by_project(items);
        let catalog = ForServer {
            modrinth: &self.inner.modrinth,
            target,
        };
        let mut planned: HashSet<String> = plan.steps.iter().map(|step| step.project.id.clone()).collect();
        let needed: Vec<(String, Option<String>, String)> = plan
            .steps
            .iter()
            .flat_map(|step| {
                step.version
                    .dependencies
                    .iter()
                    .filter(|dependency| dependency.kind == crate::model::DependencyKind::Required)
                    .filter_map(|dependency| {
                        Some((
                            dependency.project_id.clone()?,
                            dependency.version_id.clone(),
                            step.project.title.clone(),
                        ))
                    })
            })
            .collect();
        for (project_id, version_id, required_by) in needed {
            if installed.contains_key(&project_id) || planned.contains(&project_id) {
                continue;
            }
            let extra = resolver::plan(&catalog, &installed, &project_id, version_id.as_deref()).await?;
            for mut step in extra
                .steps
                .into_iter()
                .filter(|step| planned.insert(step.project.id.clone()))
            {
                step.required_by.get_or_insert_with(|| required_by.clone());
                plan.steps.push(step);
            }
        }
        Ok(plan)
    }

    /// Newer versions for the turned-on files that came from Modrinth.
    async fn available(
        &self,
        items: &[InstalledItem],
        target: &Target,
    ) -> Result<Vec<AvailableUpdate>, ContentError> {
        let tracked: Vec<&InstalledItem> = items
            .iter()
            .filter(|item| item.enabled && item.source.is_some() && item.sha1.is_some())
            .collect();
        let sha1s: Vec<String> = tracked.iter().filter_map(|item| item.sha1.clone()).collect();
        let latest = self.inner.modrinth.latest(&sha1s, target).await?;
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
        let installed = self.inner.modrinth.identify(&outdated_sha1s).await?;
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
                project_id: newest.project_id.clone(),
                version: newest.clone(),
            })
            .collect())
    }
}

/// When Modrinth didn't return the project: enough to install and index the update.
fn fallback_project(update: &AvailableUpdate) -> Project {
    Project {
        source: crate::model::SourceKind::Modrinth,
        id: update.project_id.clone(),
        slug: update.project_id.to_lowercase(),
        title: update.name.clone(),
        description: String::new(),
        body: String::new(),
        icon_url: None,
        server_side: crate::model::SideSupport::Unknown,
        client_side: crate::model::SideSupport::Unknown,
        page_url: String::new(),
    }
}

/// "1 mod", "3 plugins" (from the folder name, which is plural).
fn plural(count: usize, folder: &str) -> String {
    let noun = folder.trim_end_matches('s');
    format!("{count} {noun}{}", if count == 1 { "" } else { "s" })
}
