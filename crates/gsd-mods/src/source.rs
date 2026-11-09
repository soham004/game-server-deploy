//! The content sources behind one small interface, so the service, the resolver and update checks
//! don't care which one they are talking to.
//!
//! An enum rather than a trait object: `async fn` in a trait isn't object-safe, and there are only
//! ever a few sources. What only some sources can do (Modrinth's lookup by hash) stays on their own
//! client and is called directly.

use crate::{
    curseforge::CurseForge,
    error::SourceError,
    hangar::Hangar,
    model::{ContentVersion, Project, SearchPage, SearchQuery, SourceKind},
    modrinth::Modrinth,
    resolver::Catalog,
    target::Target,
};

/// Where the sources' APIs are.
#[derive(Debug, Clone)]
pub struct SourceApis {
    pub modrinth: String,
    pub hangar: String,
    pub curseforge: String,
}

impl Default for SourceApis {
    fn default() -> Self {
        Self {
            modrinth: crate::modrinth::DEFAULT_API.to_owned(),
            hangar: crate::hangar::DEFAULT_API.to_owned(),
            curseforge: crate::curseforge::DEFAULT_API.to_owned(),
        }
    }
}

/// Every source this app can use.
#[derive(Debug, Clone)]
pub struct Sources {
    client: reqwest::Client,
    pub(crate) modrinth: Modrinth,
    hangar: Hangar,
    pub(crate) curseforge: CurseForge,
}

impl Sources {
    pub fn new(client: reqwest::Client, apis: SourceApis) -> Self {
        Self {
            modrinth: Modrinth::new(client.clone(), apis.modrinth),
            hangar: Hangar::new(client.clone(), apis.hangar),
            curseforge: CurseForge::new(client.clone(), apis.curseforge),
            client,
        }
    }

    /// The HTTP client (downloads use it too).
    pub(crate) fn client(&self) -> &reqwest::Client {
        &self.client
    }

    pub(crate) fn get(&self, kind: SourceKind) -> Source<'_> {
        match kind {
            SourceKind::Modrinth => Source::Modrinth(&self.modrinth),
            SourceKind::Hangar => Source::Hangar(&self.hangar),
            SourceKind::CurseForge => Source::CurseForge(&self.curseforge),
        }
    }

    /// The sources that have something for `target`.
    pub(crate) fn serving(&self, target: &Target) -> Vec<SourceKind> {
        SourceKind::ALL
            .into_iter()
            .filter(|kind| self.get(*kind).serves(target))
            .collect()
    }
}

/// One source, chosen at run time.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Source<'a> {
    Modrinth(&'a Modrinth),
    Hangar(&'a Hangar),
    CurseForge(&'a CurseForge),
}

impl Source<'_> {
    /// Whether this source can be used for `target` now (`CurseForge` needs its key).
    pub(crate) fn serves(self, target: &Target) -> bool {
        match self {
            Self::Modrinth(_) => true,
            Self::Hangar(_) => Hangar::serves(target),
            Self::CurseForge(curseforge) => curseforge.has_key(),
        }
    }

    pub(crate) async fn search(
        self,
        target: &Target,
        query: &SearchQuery,
    ) -> Result<SearchPage, SourceError> {
        match self {
            Self::Modrinth(modrinth) => modrinth.search(target, query).await,
            Self::Hangar(hangar) => hangar.search(target, query).await,
            Self::CurseForge(curseforge) => curseforge.search(target, query).await,
        }
    }

    pub(crate) async fn project(self, id: &str) -> Result<Project, SourceError> {
        match self {
            Self::Modrinth(modrinth) => modrinth.project(id).await,
            Self::Hangar(hangar) => hangar.project(id).await,
            Self::CurseForge(curseforge) => curseforge.project(id).await,
        }
    }

    /// Several projects; ones the source no longer has are left out.
    pub(crate) async fn projects(self, ids: &[String]) -> Result<Vec<Project>, SourceError> {
        match self {
            Self::Modrinth(modrinth) => modrinth.projects(ids).await,
            Self::CurseForge(curseforge) => curseforge.projects(ids).await,
            Self::Hangar(hangar) => {
                let mut projects = Vec::with_capacity(ids.len());
                for id in ids {
                    match hangar.project(id).await {
                        Ok(project) => projects.push(project),
                        Err(SourceError::NotFound) => {}
                        Err(error) => return Err(error),
                    }
                }
                Ok(projects)
            }
        }
    }

    /// A project's versions that fit `target`, newest first.
    pub(crate) async fn versions(
        self,
        project_id: &str,
        target: &Target,
    ) -> Result<Vec<ContentVersion>, SourceError> {
        match self {
            Self::Modrinth(modrinth) => modrinth.versions(project_id, target).await,
            Self::Hangar(hangar) => hangar.versions(project_id, target).await,
            Self::CurseForge(curseforge) => curseforge.versions(project_id, target).await,
        }
    }
}

/// One source, narrowed to what fits one server: what the resolver looks things up in. A
/// dependency is therefore always looked for on the source of what needs it (ids mean nothing on
/// another source).
pub(crate) struct ForServer<'a> {
    pub(crate) source: Source<'a>,
    pub(crate) target: &'a Target,
}

impl Catalog for ForServer<'_> {
    async fn project(&self, id: &str) -> Result<Project, SourceError> {
        self.source.project(id).await
    }

    async fn versions(&self, project_id: &str) -> Result<Vec<ContentVersion>, SourceError> {
        self.source.versions(project_id, self.target).await
    }
}
