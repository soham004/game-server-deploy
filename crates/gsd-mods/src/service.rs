//! The content API for one server: what it has installed, searching Modrinth for what fits it,
//! planning and installing (in a job that holds the server's lock), turning files on and off,
//! and removing them.

use std::{path::PathBuf, sync::Arc};

use gsd_core::{
    events::{EventBus, EventKind},
    files::{SafeRelPath, ServerFiles},
    ids::{InstanceId, JobId},
    instances::{Instance, InstanceError, InstanceService},
    jobs::{JobFailure, JobRunner, JobSpec},
};
use gsd_minecraft::settings::MinecraftServer;
use serde::Serialize;

use crate::{
    error::{ContentError, SourceError},
    index::INDEX_FOLDER,
    install,
    installed::{self, DISABLED, InstalledItem, ScanCache},
    model::{ContentVersion, Project, SearchPage, SearchQuery},
    modrinth::Modrinth,
    resolver::{self, Catalog, InstallPlan},
    target::{ContentKind, Target},
};

/// What a server has installed.
#[derive(Debug, Clone, Serialize)]
pub struct ContentList {
    pub kind: ContentKind,
    pub folder: &'static str,
    pub target: Target,
    pub items: Vec<InstalledItem>,
}

#[derive(Debug, Clone)]
pub struct ContentService {
    pub(crate) inner: Arc<Inner>,
}

#[derive(Debug)]
pub(crate) struct Inner {
    pub(crate) modrinth: Modrinth,
    pub(crate) instances: InstanceService,
    pub(crate) jobs: JobRunner,
    pub(crate) events: EventBus,
    /// Downloaded files by hash, shared by every server (`<data>/cache/content`).
    pub(crate) cache_dir: PathBuf,
    pub(crate) scan: ScanCache,
}

impl ContentService {
    pub fn new(
        modrinth: Modrinth,
        instances: InstanceService,
        jobs: JobRunner,
        events: EventBus,
        cache_dir: PathBuf,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                modrinth,
                instances,
                jobs,
                events,
                cache_dir,
                scan: ScanCache::default(),
            }),
        }
    }

    pub async fn list(&self, id: InstanceId) -> Result<ContentList, ContentError> {
        let (instance, target) = self.server(id).await?;
        let items = self.installed(&instance, &target).await?;
        Ok(ContentList {
            kind: target.kind,
            folder: target.kind.folder(),
            target,
            items,
        })
    }

    /// Projects that fit the server.
    pub async fn search(&self, id: InstanceId, query: &SearchQuery) -> Result<SearchPage, ContentError> {
        let (_, target) = self.server(id).await?;
        Ok(self.inner.modrinth.search(&target, query).await?)
    }

    pub async fn project(&self, project_id: &str) -> Result<Project, ContentError> {
        Ok(self.inner.modrinth.project(project_id).await?)
    }

    /// A project's versions that fit the server, newest first.
    pub async fn versions(
        &self,
        id: InstanceId,
        project_id: &str,
    ) -> Result<Vec<ContentVersion>, ContentError> {
        let (_, target) = self.server(id).await?;
        Ok(self.inner.modrinth.versions(project_id, &target).await?)
    }

    /// What installing `project_id` would do (dependencies, replacements), without doing it.
    pub async fn plan(
        &self,
        id: InstanceId,
        project_id: &str,
        version_id: Option<&str>,
    ) -> Result<InstallPlan, ContentError> {
        let (instance, target) = self.server(id).await?;
        let installed = installed::by_project(&self.installed(&instance, &target).await?);
        let catalog = ForServer {
            modrinth: &self.inner.modrinth,
            target: &target,
        };
        Ok(resolver::plan(&catalog, &installed, project_id, version_id).await?)
    }

    /// Plans, then installs in a job. Returns the plan it carries out.
    pub async fn start_install(
        &self,
        id: InstanceId,
        project_id: &str,
        version_id: Option<&str>,
    ) -> Result<(InstallPlan, JobId), ContentError> {
        let plan = self.plan(id, project_id, version_id).await?;
        let (instance, target) = self.server(id).await?;
        let title = plan
            .steps
            .first()
            .map_or_else(String::new, |step| step.project.title.clone());
        let spec = JobSpec {
            kind: "content.install",
            title: format!("Installing {title} on {}", instance.definition.name),
            instance_id: Some(id),
        };
        let (service, steps) = (self.clone(), plan.clone());
        let job = self.inner.jobs.spawn(spec, move |job| async move {
            let files = ServerFiles::open(&instance).await?;
            let inner = &service.inner;
            let installed = install::run(
                &files,
                target.kind.folder(),
                &steps,
                inner.modrinth.client(),
                &inner.cache_dir,
                &job,
            )
            .await;
            inner
                .events
                .publish(Some(instance.id()), EventKind::ContentChanged);
            installed.map_err(|error| JobFailure::msg(gsd_core::jobs::error_chain(&error)))
        })?;
        Ok((plan, job))
    }

    /// Turns a file on (`x.jar`) or off (`x.jar.disabled`, which loaders ignore).
    pub async fn set_enabled(&self, id: InstanceId, file: &str, enabled: bool) -> Result<(), ContentError> {
        let (instance, target) = self.server(id).await?;
        let item = self.find(&instance, &target, file).await?;
        if item.enabled == enabled {
            return Ok(());
        }
        let folder = SafeRelPath::parse(target.kind.folder())?;
        let (on, off) = (
            folder.join(&item.file)?,
            folder.join(&format!("{}{DISABLED}", item.file))?,
        );
        let files = ServerFiles::open(&instance).await?;
        if enabled {
            files.rename(off, on).await?;
        } else {
            files.rename(on, off).await?;
        }
        self.changed(id);
        Ok(())
    }

    /// Moves a file and its index entry to the server's trash.
    pub async fn remove(&self, id: InstanceId, file: &str) -> Result<(), ContentError> {
        let (instance, target) = self.server(id).await?;
        let item = self.find(&instance, &target, file).await?;
        let folder = SafeRelPath::parse(target.kind.folder())?;
        let actual = if item.enabled {
            item.file.clone()
        } else {
            format!("{}{DISABLED}", item.file)
        };
        let mut paths = vec![folder.join(&actual)?];
        if let Some(index_file) = &item.index_file {
            paths.push(folder.join(INDEX_FOLDER)?.join(index_file)?);
        }
        ServerFiles::open(&instance).await?.delete(paths).await?;
        self.changed(id);
        Ok(())
    }

    /// The server and what it can run. Content changes while a job (an install, a backup) holds
    /// the server are refused by the callers' job lock; this only reads.
    pub(crate) async fn server(&self, id: InstanceId) -> Result<(Instance, Target), ContentError> {
        let instance = self.inner.instances.get(id).await?;
        if instance.definition.installed_at.is_none() {
            return Err(InstanceError::NotInstalled.into());
        }
        let server =
            MinecraftServer::from_table(&instance.definition.server).map_err(|_| ContentError::NoContent)?;
        let target = Target::for_server(&server).ok_or(ContentError::NoContent)?;
        Ok((instance, target))
    }

    pub(crate) async fn installed(
        &self,
        instance: &Instance,
        target: &Target,
    ) -> Result<Vec<InstalledItem>, ContentError> {
        let files = ServerFiles::open(instance).await?;
        Ok(installed::list(
            &files,
            target.kind.folder(),
            &self.inner.scan,
            &instance.id().to_string(),
        )
        .await?)
    }

    async fn find(
        &self,
        instance: &Instance,
        target: &Target,
        file: &str,
    ) -> Result<InstalledItem, ContentError> {
        if self.inner.instances.is_busy(instance.id()) {
            return Err(InstanceError::Busy.into());
        }
        self.installed(instance, target)
            .await?
            .into_iter()
            .find(|item| item.file == file)
            .ok_or_else(|| ContentError::NotInstalled(file.to_owned()))
    }

    pub(crate) fn changed(&self, id: InstanceId) {
        self.inner.events.publish(Some(id), EventKind::ContentChanged);
    }
}

/// Modrinth, narrowed to what fits one server (what the resolver looks things up in).
pub(crate) struct ForServer<'a> {
    pub(crate) modrinth: &'a Modrinth,
    pub(crate) target: &'a Target,
}

impl Catalog for ForServer<'_> {
    async fn project(&self, id: &str) -> Result<Project, SourceError> {
        self.modrinth.project(id).await
    }

    async fn versions(&self, project_id: &str) -> Result<Vec<ContentVersion>, SourceError> {
        self.modrinth.versions(project_id, self.target).await
    }
}
