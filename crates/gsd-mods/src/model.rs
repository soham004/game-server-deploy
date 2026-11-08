//! What content sources describe, independent of any one source: projects, their versions and
//! files, dependencies, and search pages. Sources map their own wire formats onto these.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Where a project comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    Modrinth,
}

impl SourceKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Modrinth => "modrinth",
        }
    }
}

/// Whether a project is needed, optional or useless on one side (client or server).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SideSupport {
    Required,
    Optional,
    Unsupported,
    Unknown,
}

/// A search result: enough for the browse list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProjectSummary {
    pub source: SourceKind,
    pub id: String,
    pub slug: String,
    pub title: String,
    pub description: String,
    pub author: String,
    pub icon_url: Option<String>,
    pub downloads: u64,
    pub categories: Vec<String>,
    pub updated: Option<DateTime<Utc>>,
    pub server_side: SideSupport,
    pub client_side: SideSupport,
}

/// One page of search results.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SearchPage {
    pub hits: Vec<ProjectSummary>,
    pub offset: u32,
    pub total: u32,
}

/// The order of search results.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchSort {
    #[default]
    Relevance,
    Downloads,
    Updated,
    Newest,
}

/// A search, already narrowed to what fits the server (see [`crate::Target`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchQuery {
    pub text: String,
    pub sort: SearchSort,
    pub offset: u32,
    pub limit: u32,
}

/// A project page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Project {
    pub source: SourceKind,
    pub id: String,
    pub slug: String,
    pub title: String,
    pub description: String,
    /// Long description (Markdown, from the source; render it sanitised).
    pub body: String,
    pub icon_url: Option<String>,
    pub server_side: SideSupport,
    pub client_side: SideSupport,
    pub page_url: String,
}

/// How mature a version is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Channel {
    Release,
    Beta,
    Alpha,
}

/// One version of a project.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ContentVersion {
    pub id: String,
    pub project_id: String,
    pub name: String,
    pub version_number: String,
    pub channel: Channel,
    pub published: Option<DateTime<Utc>>,
    pub game_versions: Vec<String>,
    pub loaders: Vec<String>,
    pub dependencies: Vec<Dependency>,
    /// The file to install (the source's primary file).
    pub file: ContentFile,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ContentFile {
    pub url: String,
    pub filename: String,
    pub size: u64,
    pub sha1: Option<String>,
    pub sha512: Option<String>,
}

/// What another project means to this version.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DependencyKind {
    /// Must be installed too.
    Required,
    /// Works better with it.
    Optional,
    /// Must not be installed alongside.
    Incompatible,
    /// Already inside the file.
    Embedded,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Dependency {
    pub project_id: Option<String>,
    /// A specific version, when the dependent pins one.
    pub version_id: Option<String>,
    pub kind: DependencyKind,
}
