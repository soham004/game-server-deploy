//! What installing a project means: the project plus every required dependency (and theirs),
//! each at a version that fits the server. Dependencies already installed are kept as they are;
//! only the project asked for replaces its installed version. An `incompatible` relation with
//! anything installed or planned stops the plan with an explanation.

use std::collections::{HashMap, HashSet, VecDeque};

use serde::Serialize;

use crate::{
    error::SourceError,
    model::{Channel, ContentVersion, DependencyKind, Project},
};

/// A plan never grows beyond this many projects (a broken dependency graph can't run away).
const MAX_PROJECTS: usize = 64;

/// Where the resolver gets projects and versions (Modrinth, or a fake in tests).
pub trait Catalog {
    fn project(&self, id: &str) -> impl Future<Output = Result<Project, SourceError>> + Send;
    /// Versions that fit the server, newest first.
    fn versions(
        &self,
        project_id: &str,
    ) -> impl Future<Output = Result<Vec<ContentVersion>, SourceError>> + Send;
}

/// What is installed, by project id: its file name and version.
pub type Installed = HashMap<String, InstalledVersion>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledVersion {
    pub filename: String,
    pub version_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct InstallPlan {
    pub steps: Vec<PlannedInstall>,
    /// Required dependencies already installed (kept as they are).
    pub already_installed: Vec<String>,
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
    #[error("too many dependencies (more than {MAX_PROJECTS} projects)")]
    TooMany,
    #[error(transparent)]
    Source(#[from] SourceError),
}

/// Plans installing `project_id` (at `version_id`, or the best fitting version).
pub async fn plan(
    catalog: &impl Catalog,
    installed: &Installed,
    project_id: &str,
    version_id: Option<&str>,
) -> Result<InstallPlan, ResolveError> {
    let mut queue = VecDeque::from([Wanted {
        project_id: project_id.to_owned(),
        version_id: version_id.map(str::to_owned),
        required_by: None,
    }]);
    let mut seen = HashSet::new();
    let mut plan = InstallPlan {
        steps: Vec::new(),
        already_installed: Vec::new(),
    };
    let mut forbidden: Vec<(String, String)> = Vec::new(); // (project id, title of who forbids it)
    while let Some(wanted) = queue.pop_front() {
        if !seen.insert(wanted.project_id.clone()) {
            continue;
        }
        if seen.len() > MAX_PROJECTS {
            return Err(ResolveError::TooMany);
        }
        let project = catalog.project(&wanted.project_id).await?;
        if wanted.required_by.is_some() && installed.contains_key(&project.id) {
            plan.already_installed.push(project.title);
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
            replaces: installed
                .get(&project.id)
                .map(|existing| existing.filename.clone()),
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
        if let Some(existing) = installed.get(id) {
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
