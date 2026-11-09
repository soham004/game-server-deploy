//! What content sources describe, independent of any one source: projects, their versions and
//! files, dependencies, and search pages. Sources map their own wire formats onto these.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Where a project comes from.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    #[default]
    Modrinth,
    /// The Paper project's plugin repository (plugins only).
    Hangar,
    /// Mods and Bukkit plugins, with the user's own API key.
    #[serde(rename = "curseforge")]
    CurseForge,
}

impl SourceKind {
    /// Every source, in the order the UI offers them.
    pub const ALL: [Self; 3] = [Self::Modrinth, Self::Hangar, Self::CurseForge];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Modrinth => "modrinth",
            Self::Hangar => "hangar",
            Self::CurseForge => "curseforge",
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
    /// The project's page on its source's website.
    pub page_url: String,
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
    /// Hangar publishes only this one.
    pub sha256: Option<String>,
    pub sha512: Option<String>,
    /// Set when the author doesn't allow other apps to download the file: where the user can get
    /// it by hand. `url` is empty then, and installing leaves the file for the user to upload.
    pub manual_url: Option<String>,
}

/// A checksum algorithm, named as packwiz names it in `hash-format`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HashFormat {
    Sha1,
    Sha256,
    Sha512,
}

impl HashFormat {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Sha1 => "sha1",
            Self::Sha256 => "sha256",
            Self::Sha512 => "sha512",
        }
    }
}

impl ContentFile {
    /// The strongest checksum the source gave, to check the download and to record in the index.
    pub fn best_hash(&self) -> Option<(HashFormat, &str)> {
        [
            (HashFormat::Sha512, &self.sha512),
            (HashFormat::Sha256, &self.sha256),
            (HashFormat::Sha1, &self.sha1),
        ]
        .into_iter()
        .find_map(|(format, hash)| hash.as_deref().map(|hash| (format, hash)))
    }
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
    /// `None` for one hosted outside this source, which can't be installed from here.
    pub project_id: Option<String>,
    /// What it is called, when the source says (so such a dependency can be named to the user).
    pub name: Option<String>,
    /// A specific version, when the dependent pins one.
    pub version_id: Option<String>,
    pub kind: DependencyKind,
}
