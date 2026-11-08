//! Modrinth's JSON, as it arrives (API v2, checked against the live API 2026-11-08; see
//! `tests/fixtures/record.mjs`), and how it maps onto the source-independent model.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::Deserialize;

use crate::model::{
    Channel, ContentFile, ContentVersion, Dependency, DependencyKind, Project, ProjectSummary, SearchPage,
    SideSupport, SourceKind,
};

#[derive(Debug, Deserialize)]
pub(super) struct SearchResponse {
    hits: Vec<SearchHit>,
    offset: u32,
    total_hits: u32,
}

#[derive(Debug, Deserialize)]
struct SearchHit {
    project_id: String,
    slug: String,
    title: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    author: String,
    icon_url: Option<String>,
    #[serde(default)]
    downloads: u64,
    #[serde(default)]
    categories: Vec<String>,
    date_modified: Option<DateTime<Utc>>,
    #[serde(default)]
    client_side: Side,
    #[serde(default)]
    server_side: Side,
}

#[derive(Debug, Deserialize)]
pub(super) struct ProjectResponse {
    id: String,
    slug: String,
    title: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    body: String,
    icon_url: Option<String>,
    #[serde(default)]
    client_side: Side,
    #[serde(default)]
    server_side: Side,
    project_type: String,
}

#[derive(Debug, Deserialize)]
pub(super) struct VersionResponse {
    id: String,
    project_id: String,
    #[serde(default)]
    name: String,
    version_number: String,
    version_type: String,
    date_published: Option<DateTime<Utc>>,
    #[serde(default)]
    game_versions: Vec<String>,
    #[serde(default)]
    loaders: Vec<String>,
    #[serde(default)]
    dependencies: Vec<DependencyResponse>,
    files: Vec<FileResponse>,
}

#[derive(Debug, Deserialize)]
struct DependencyResponse {
    project_id: Option<String>,
    version_id: Option<String>,
    dependency_type: String,
}

#[derive(Debug, Deserialize)]
struct FileResponse {
    url: String,
    filename: String,
    #[serde(default)]
    primary: bool,
    #[serde(default)]
    size: u64,
    #[serde(default)]
    hashes: HashMap<String, String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Side {
    Required,
    Optional,
    Unsupported,
    #[default]
    #[serde(other)]
    Unknown,
}

impl From<Side> for SideSupport {
    fn from(side: Side) -> Self {
        match side {
            Side::Required => Self::Required,
            Side::Optional => Self::Optional,
            Side::Unsupported => Self::Unsupported,
            Side::Unknown => Self::Unknown,
        }
    }
}

impl From<SearchResponse> for SearchPage {
    fn from(response: SearchResponse) -> Self {
        Self {
            hits: response.hits.into_iter().map(ProjectSummary::from).collect(),
            offset: response.offset,
            total: response.total_hits,
        }
    }
}

impl From<SearchHit> for ProjectSummary {
    fn from(hit: SearchHit) -> Self {
        Self {
            source: SourceKind::Modrinth,
            id: hit.project_id,
            slug: hit.slug,
            title: hit.title,
            description: hit.description,
            author: hit.author,
            icon_url: hit.icon_url,
            downloads: hit.downloads,
            categories: hit.categories,
            updated: hit.date_modified,
            server_side: hit.server_side.into(),
            client_side: hit.client_side.into(),
        }
    }
}

impl From<ProjectResponse> for Project {
    fn from(project: ProjectResponse) -> Self {
        // Plugin projects are listed under /plugin/, everything else under its type.
        let section = if project.project_type == "plugin" {
            "plugin"
        } else {
            "mod"
        };
        Self {
            source: SourceKind::Modrinth,
            page_url: format!("https://modrinth.com/{section}/{}", project.slug),
            id: project.id,
            slug: project.slug,
            title: project.title,
            description: project.description,
            body: project.body,
            icon_url: project.icon_url,
            server_side: project.server_side.into(),
            client_side: project.client_side.into(),
        }
    }
}

impl VersionResponse {
    /// The model's version, or `None` for a version without files (it can't be installed).
    pub(super) fn into_version(self) -> Option<ContentVersion> {
        let channel = match self.version_type.as_str() {
            "beta" => Channel::Beta,
            "alpha" => Channel::Alpha,
            _ => Channel::Release,
        };
        let mut files = self.files;
        let primary = files.iter().position(|file| file.primary).unwrap_or(0);
        if files.is_empty() {
            return None;
        }
        let file = files.swap_remove(primary);
        Some(ContentVersion {
            id: self.id,
            project_id: self.project_id,
            name: self.name,
            version_number: self.version_number,
            channel,
            published: self.date_published,
            game_versions: self.game_versions,
            loaders: self.loaders,
            dependencies: self
                .dependencies
                .into_iter()
                .filter_map(DependencyResponse::into_dependency)
                .collect(),
            file: ContentFile {
                url: file.url,
                filename: file.filename,
                size: file.size,
                sha1: file.hashes.get("sha1").cloned(),
                sha512: file.hashes.get("sha512").cloned(),
            },
        })
    }
}

impl DependencyResponse {
    fn into_dependency(self) -> Option<Dependency> {
        let kind = match self.dependency_type.as_str() {
            "required" => DependencyKind::Required,
            "optional" => DependencyKind::Optional,
            "incompatible" => DependencyKind::Incompatible,
            "embedded" => DependencyKind::Embedded,
            _ => return None,
        };
        Some(Dependency {
            project_id: self.project_id,
            version_id: self.version_id,
            kind,
        })
    }
}
