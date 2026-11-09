//! What installing projects means: the projects plus every required dependency (and theirs),
//! each at a version that fits the server. Dependencies already installed are kept as they are;
//! only a project asked for replaces its installed version. Several projects asked for at once
//! share their dependencies (each is planned once), and one that another needs counts as asked
//! for. An `incompatible` relation with anything installed or planned stops the plan with an
//! explanation.
//!
//! One plan walks one source (a catalog), since project ids mean nothing on another. The same
//! plugin can still be on the server from elsewhere (another source, or copied in by hand), and
//! two copies conflict (the loader reports it and keeps at most one, not necessarily the newer), so
//! projects are also matched by name against files this source doesn't track: such a file counts
//! as the dependency, and is replaced when the project itself is asked for.

use std::collections::{HashMap, HashSet, VecDeque};

use serde::Serialize;

use crate::{
    error::SourceError,
    model::{Channel, ContentVersion, DependencyKind, Project, SourceKind},
};

/// A plan never grows beyond this many projects (a broken dependency graph can't run away; a
/// batch of mods asked for together, with their dependencies, stays well below it).
const MAX_PROJECTS: usize = 128;

/// Where the resolver gets projects and versions (one source, or a fake in tests).
pub trait Catalog {
    fn project(&self, id: &str) -> impl Future<Output = Result<Project, SourceError>> + Send;
    /// Versions that fit the server, newest first.
    fn versions(
        &self,
        project_id: &str,
    ) -> impl Future<Output = Result<Vec<ContentVersion>, SourceError>> + Send;
}

/// What is on the server, as one source sees it.
#[derive(Debug, Clone, Default)]
pub struct Installed {
    /// What this source tracks, by project id.
    projects: HashMap<String, InstalledVersion>,
    /// Files this source doesn't track, by each name they go by.
    untracked: HashMap<String, String>,
    /// Every name on the server, tracked or not.
    names: HashSet<String>,
    /// Names coming from a source planned earlier in the same install.
    planned_elsewhere: HashSet<String>,
}

impl Installed {
    pub fn new() -> Self {
        Self::default()
    }

    /// The ids of what this source tracks.
    pub fn project_ids(&self) -> impl Iterator<Item = &String> {
        self.projects.keys()
    }

    /// A file this source tracks as `project_id`, going by `names`.
    pub fn add_tracked(&mut self, project_id: String, version: InstalledVersion, names: &[&str]) {
        self.names.extend(names.iter().map(|name| comparable(name)));
        self.projects.insert(project_id, version);
    }

    /// A file from elsewhere (another source, or by hand), going by `names`.
    pub fn add_untracked(&mut self, filename: &str, names: &[&str]) {
        for name in names {
            let name = comparable(name);
            self.names.insert(name.clone());
            self.untracked.insert(name, filename.to_owned());
        }
    }

    /// Something an earlier source in this install brings.
    pub fn add_planned_elsewhere(&mut self, project: &Project) {
        self.planned_elsewhere.extend(names_of(project));
    }

    /// The file that already is `project`: tracked by this source, or going by its name.
    fn file_of(&self, project: &Project) -> Option<&str> {
        self.projects
            .get(&project.id)
            .map(|installed| installed.filename.as_str())
            .or_else(|| {
                names_of(project)
                    .iter()
                    .find_map(|name| self.untracked.get(name).map(String::as_str))
            })
    }

    fn is_planned_elsewhere(&self, project: &Project) -> bool {
        names_of(project)
            .iter()
            .any(|name| self.planned_elsewhere.contains(name))
    }

    fn has_name(&self, name: &str) -> bool {
        self.names.contains(&comparable(name))
    }
}

/// Tests name the server's projects directly.
impl<const N: usize> From<[(String, InstalledVersion); N]> for Installed {
    fn from(projects: [(String, InstalledVersion); N]) -> Self {
        Self {
            projects: HashMap::from(projects),
            ..Self::default()
        }
    }
}

/// What a project goes by, in comparable form.
fn names_of(project: &Project) -> [String; 2] {
    [comparable(&project.title), comparable(&project.slug)]
}

/// `Fabric API`, `fabric-api` and `fabric_api` are one name.
fn comparable(name: &str) -> String {
    name.chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledVersion {
    pub filename: String,
    pub version_id: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct InstallPlan {
    pub steps: Vec<PlannedInstall>,
    /// Required dependencies already installed (kept as they are).
    pub already_installed: Vec<String>,
    /// Required dependencies hosted outside the source: they can't be installed from here, so the
    /// user has to know.
    pub not_available: Vec<UnavailableDependency>,
}

impl InstallPlan {
    /// Adds what another source's plan does.
    pub fn absorb(&mut self, other: Self) {
        self.steps.extend(other.steps);
        for name in other.already_installed {
            if !self.already_installed.contains(&name) {
                self.already_installed.push(name);
            }
        }
        self.not_available.extend(other.not_available);
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UnavailableDependency {
    pub name: String,
    pub required_by: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PlannedInstall {
    pub project: Project,
    pub version: ContentVersion,
    /// The title of the project that needs it; `None` for the one asked for.
    pub required_by: Option<String>,
    /// The installed file this one replaces.
    pub replaces: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum ResolveError {
    #[error("{0} has no version for this server's Minecraft version and loader")]
    NoCompatibleVersion(String),
    #[error("{0} doesn't have the chosen version for this server")]
    UnknownVersion(String),
    #[error("{0} can't be installed together with {1}")]
    Incompatible(String, String),
    #[error("{0} would be installed twice, from two sources; keep one of them on the list")]
    FromTwoSources(String),
    #[error("too many dependencies (more than {MAX_PROJECTS} projects)")]
    TooMany,
    #[error(transparent)]
    Source(#[from] SourceError),
}

/// A project asked for, at a chosen version or the best fitting one.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Requested {
    /// Where the project is (Modrinth unless said otherwise).
    #[serde(default)]
    pub source: SourceKind,
    pub project_id: String,
    #[serde(default)]
    pub version_id: Option<String>,
}

/// Plans installing `project_id` (at `version_id`, or the best fitting version).
pub async fn plan(
    catalog: &impl Catalog,
    installed: &Installed,
    project_id: &str,
    version_id: Option<&str>,
) -> Result<InstallPlan, ResolveError> {
    let requested = Requested {
        source: SourceKind::default(),
        project_id: project_id.to_owned(),
        version_id: version_id.map(str::to_owned),
    };
    plan_many(catalog, installed, std::slice::from_ref(&requested)).await
}

/// Plans installing every project in `requested` together (see the module docs). The `source` of
/// each is the caller's business: `catalog` is the one source they are all on.
pub async fn plan_many(
    catalog: &impl Catalog,
    installed: &Installed,
    requested: &[Requested],
) -> Result<InstallPlan, ResolveError> {
    // Everything asked for comes first, so a project that one of them needs is planned as asked
    // for (at its chosen version) rather than as a dependency.
    let queue = requested
        .iter()
        .map(|request| Wanted {
            project_id: request.project_id.clone(),
            version_id: request.version_id.clone(),
            required_by: None,
        })
        .collect();
    walk(catalog, installed, queue).await
}

/// Plans the dependencies an update newly brings: what is installed already is kept, and everything
/// else comes as a dependency of who needs it.
pub async fn plan_required(
    catalog: &impl Catalog,
    installed: &Installed,
    wanted: Vec<RequiredDependency>,
) -> Result<InstallPlan, ResolveError> {
    let queue = wanted
        .into_iter()
        .map(|dependency| Wanted {
            project_id: dependency.project_id,
            version_id: dependency.version_id,
            required_by: Some(dependency.required_by),
        })
        .collect();
    walk(catalog, installed, queue).await
}

/// A dependency to resolve, and the title of what needs it.
#[derive(Debug, Clone)]
pub struct RequiredDependency {
    pub project_id: String,
    pub version_id: Option<String>,
    pub required_by: String,
}

/// Breadth-first over required dependencies.
async fn walk(
    catalog: &impl Catalog,
    installed: &Installed,
    mut queue: VecDeque<Wanted>,
) -> Result<InstallPlan, ResolveError> {
    let mut seen = HashSet::new();
    let mut plan = InstallPlan::default();
    let mut forbidden: Vec<(String, String)> = Vec::new(); // (project id, title of who forbids it)
    while let Some(wanted) = queue.pop_front() {
        if !seen.insert(wanted.project_id.clone()) {
            continue;
        }
        if seen.len() > MAX_PROJECTS {
            return Err(ResolveError::TooMany);
        }
        let project = catalog.project(&wanted.project_id).await?;
        // Asked for by slug and needed by id (or the other way round): still one project.
        if !seen.insert(project.id.clone()) && plan.steps.iter().any(|step| step.project.id == project.id) {
            continue;
        }
        if installed.is_planned_elsewhere(&project) {
            if wanted.required_by.is_none() {
                return Err(ResolveError::FromTwoSources(project.title));
            }
            continue;
        }
        let existing = installed.file_of(&project).map(str::to_owned);
        if wanted.required_by.is_some() && existing.is_some() {
            if !plan.already_installed.contains(&project.title) {
                plan.already_installed.push(project.title);
            }
            continue;
        }
        let version = choose(
            catalog,
            &project,
            wanted.version_id.as_deref(),
            wanted.required_by.is_none(),
        )
        .await?;
        for dependency in &version.dependencies {
            let Some(id) = &dependency.project_id else {
                if let (DependencyKind::Required, Some(name)) = (dependency.kind, &dependency.name)
                    && !installed.has_name(name)
                {
                    plan.not_available.push(UnavailableDependency {
                        name: name.clone(),
                        required_by: project.title.clone(),
                    });
                }
                continue;
            };
            match dependency.kind {
                DependencyKind::Required => queue.push_back(Wanted {
                    project_id: id.clone(),
                    version_id: dependency.version_id.clone(),
                    required_by: Some(project.title.clone()),
                }),
                DependencyKind::Incompatible => forbidden.push((id.clone(), project.title.clone())),
                DependencyKind::Optional | DependencyKind::Embedded => {}
            }
        }
        plan.steps.push(PlannedInstall {
            replaces: existing,
            required_by: wanted.required_by,
            project,
            version,
        });
    }
    check_incompatibilities(&plan, installed, &forbidden)?;
    Ok(plan)
}

struct Wanted {
    project_id: String,
    version_id: Option<String>,
    required_by: Option<String>,
}

/// The pinned version if given (and it fits), else the newest release, else the newest of any.
/// A pin that doesn't fit is an error for what was asked for; for a dependency, the best fit is
/// taken instead (authors often pin an exact build of a library).
async fn choose(
    catalog: &impl Catalog,
    project: &Project,
    version_id: Option<&str>,
    asked_for: bool,
) -> Result<ContentVersion, ResolveError> {
    let versions = catalog.versions(&project.id).await?;
    if let Some(wanted) = version_id {
        if let Some(version) = versions.iter().find(|version| version.id == wanted) {
            return Ok(version.clone());
        }
        if asked_for {
            return Err(ResolveError::UnknownVersion(project.title.clone()));
        }
    }
    versions
        .iter()
        .find(|version| version.channel == Channel::Release)
        .or_else(|| versions.first())
        .cloned()
        .ok_or_else(|| ResolveError::NoCompatibleVersion(project.title.clone()))
}

fn check_incompatibilities(
    plan: &InstallPlan,
    installed: &Installed,
    forbidden: &[(String, String)],
) -> Result<(), ResolveError> {
    for (id, forbidder) in forbidden {
        if let Some(step) = plan.steps.iter().find(|step| &step.project.id == id) {
            return Err(ResolveError::Incompatible(
                forbidder.clone(),
                step.project.title.clone(),
            ));
        }
        if let Some(existing) = installed.projects.get(id) {
            return Err(ResolveError::Incompatible(
                forbidder.clone(),
                existing.filename.clone(),
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
