//! The content API for one server: what it has installed, searching a source for what fits it,
//! planning and installing (in a job that holds the server's lock), turning files on and off,
//! and removing them.

use std::{path::PathBuf, sync::Arc};

use bytes::Bytes;
use futures_util::Stream;
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
    curseforge::InvalidKey,
    error::ContentError,
    index::INDEX_FOLDER,
    install,
    installed::{self, DISABLED, InstalledItem, ScanCache},
    model::{ContentVersion, Project, SearchPage, SearchQuery, SourceKind},
    resolver::{self, InstallPlan, Requested},
    source::{ForServer, Sources},
    target::{ContentKind, Target},
    updates::Identified,
};

/// The largest jar accepted (no real mod comes near it).
const MAX_UPLOAD_BYTES: u64 = 512 * 1024 * 1024;

/// What a server has installed.
#[derive(Debug, Clone, Serialize)]
pub struct ContentList {
    pub kind: ContentKind,
    pub folder: &'static str,
    pub target: Target,
    /// Where more can be found for this server.
    pub sources: Vec<SourceKind>,
    pub items: Vec<InstalledItem>,
}

#[derive(Debug, Clone)]
pub struct ContentService {
    pub(crate) inner: Arc<Inner>,
}

#[derive(Debug)]
pub(crate) struct Inner {
    pub(crate) sources: Sources,
    pub(crate) instances: InstanceService,
    pub(crate) jobs: JobRunner,
    pub(crate) events: EventBus,
    /// Downloaded files by hash, shared by every server (`<data>/cache/content`).
    pub(crate) cache_dir: PathBuf,
    pub(crate) scan: ScanCache,
}

impl ContentService {
    pub fn new(
        sources: Sources,
        instances: InstanceService,
        jobs: JobRunner,
        events: EventBus,
        cache_dir: PathBuf,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                sources,
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
            sources: self.inner.sources.serving(&target),
            target,
            items,
        })
    }

    /// Projects on `source` that fit the server.
    pub async fn search(
        &self,
        id: InstanceId,
        source: SourceKind,
        query: &SearchQuery,
    ) -> Result<SearchPage, ContentError> {
        let (_, target) = self.server(id).await?;
        Ok(self.inner.sources.get(source).search(&target, query).await?)
    }

    pub async fn project(&self, source: SourceKind, project_id: &str) -> Result<Project, ContentError> {
        Ok(self.inner.sources.get(source).project(project_id).await?)
    }

    /// A project's versions that fit the server, newest first.
    pub async fn versions(
        &self,
        id: InstanceId,
        source: SourceKind,
        project_id: &str,
    ) -> Result<Vec<ContentVersion>, ContentError> {
        let (_, target) = self.server(id).await?;
        Ok(self
            .inner
            .sources
            .get(source)
            .versions(project_id, &target)
            .await?)
    }

    /// What installing `requested` (one project or several, with every required dependency)
    /// would do, without doing it. Each source is planned on its own, since dependencies stay on
    /// the source of what needs them; what one brings counts for the sources after it.
    pub async fn plan(&self, id: InstanceId, requested: &[Requested]) -> Result<InstallPlan, ContentError> {
        let (instance, target) = self.server(id).await?;
        let items = self.installed(&instance, &target).await?;
        let mut plan = InstallPlan::default();
        for source in SourceKind::ALL {
            let wanted: Vec<Requested> = requested
                .iter()
                .filter(|request| request.source == source)
                .cloned()
                .collect();
            if wanted.is_empty() {
                continue;
            }
            let mut installed = installed::by_project(&items, source);
            for step in &plan.steps {
                installed.add_planned_elsewhere(&step.project);
            }
            let catalog = ForServer {
                source: self.inner.sources.get(source),
                target: &target,
            };
            plan.absorb(resolver::plan_many(&catalog, &installed, &wanted).await?);
        }
        Ok(plan)
    }

    /// Plans, then installs everything in one job. Returns the plan it carries out.
    pub async fn start_install(
        &self,
        id: InstanceId,
        requested: &[Requested],
    ) -> Result<(InstallPlan, JobId), ContentError> {
        let plan = self.plan(id, requested).await?;
        let (instance, target) = self.server(id).await?;
        let asked_for: Vec<&str> = plan
            .steps
            .iter()
            .filter(|step| step.required_by.is_none())
            .map(|step| step.project.title.as_str())
            .collect();
        let what = match asked_for.as_slice() {
            [one] => (*one).to_owned(),
            several => format!("{} {}", several.len(), target.kind.folder()),
        };
        let spec = JobSpec {
            kind: "content.install",
            title: format!("Installing {what} on {}", instance.definition.name),
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
                inner.sources.client(),
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

    /// Puts a jar the user downloaded (a file whose author only allows that, or any other) into
    /// the content folder, then looks up what it is, so it can be tracked from then on. An existing
    /// file of that name stays: replacing it could orphan its index entry.
    pub async fn upload<S, E>(&self, id: InstanceId, file: &str, body: S) -> Result<Identified, ContentError>
    where
        S: Stream<Item = Result<Bytes, E>> + Unpin + Send,
        E: std::fmt::Display,
    {
        let (instance, target) = self.server(id).await?;
        if self.inner.instances.is_busy(id) {
            return Err(InstanceError::Busy.into());
        }
        if !matches!(installed::jar_name(file), Some((_, true))) {
            return Err(ContentError::NotAJar(file.to_owned()));
        }
        let folder = SafeRelPath::parse(target.kind.folder())?;
        let files = ServerFiles::open(&instance).await?;
        match files.create_dir(folder.clone()).await {
            Ok(()) | Err(gsd_core::files::FilesError::AlreadyExists(_)) => {}
            Err(error) => return Err(error.into()),
        }
        files
            .upload(folder.join(file)?, body, false, MAX_UPLOAD_BYTES)
            .await?;
        self.changed(id);
        self.identify(id).await
    }

    /// Uses this `CurseForge` API key from now on (`None`: `CurseForge` is off).
    pub fn set_curseforge_key(&self, key: Option<&str>) -> Result<(), InvalidKey> {
        self.inner.sources.curseforge.set_key(key)
    }

    pub fn has_curseforge_key(&self) -> bool {
        self.inner.sources.curseforge.has_key()
    }

    /// Asks `CurseForge` whether it accepts `key`.
    pub async fn check_curseforge_key(&self, key: &str) -> Result<(), ContentError> {
        Ok(self.inner.sources.curseforge.check_key(key).await?)
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
