//! Hangar's JSON, and how it maps onto the source-independent model.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::Deserialize;

use super::PLATFORM;
use crate::model::{
    Channel, ContentFile, ContentVersion, Dependency, DependencyKind, Project, ProjectSummary, SearchPage,
    SideSupport, SourceKind,
};

const SITE: &str = "https://hangar.papermc.io";

#[derive(Debug, Deserialize)]
pub struct Page<T> {
    pub pagination: Pagination,
    pub result: Vec<T>,
}

#[derive(Debug, Deserialize)]
pub struct Pagination {
    pub count: u64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectResponse {
    id: u64,
    name: String,
    namespace: Namespace,
    stats: Stats,
    #[serde(default)]
    category: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    last_updated: Option<DateTime<Utc>>,
    #[serde(default)]
    avatar_url: Option<String>,
    /// Only on the project endpoint.
    #[serde(default)]
    main_page_content: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Namespace {
    owner: String,
    slug: String,
}

#[derive(Debug, Deserialize)]
struct Stats {
    downloads: u64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionResponse {
    id: u64,
    project_id: u64,
    name: String,
    #[serde(default)]
    created_at: Option<DateTime<Utc>>,
    channel: VersionChannel,
    #[serde(default)]
    plugin_dependencies: HashMap<String, Vec<PluginDependency>>,
    #[serde(default)]
    platform_dependencies: HashMap<String, Vec<String>>,
    #[serde(default)]
    downloads: HashMap<String, PlatformDownload>,
}

#[derive(Debug, Deserialize)]
struct VersionChannel {
    name: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PluginDependency {
    #[serde(default)]
    name: Option<String>,
    /// Absent for plugins hosted elsewhere (then only `externalUrl` says where).
    #[serde(default)]
    project_id: Option<u64>,
    required: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PlatformDownload {
    #[serde(default)]
    file_info: Option<FileInfo>,
    /// Absent when the file is hosted elsewhere.
    #[serde(default)]
    download_url: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FileInfo {
    name: String,
    size_bytes: u64,
    #[serde(default)]
    sha256_hash: Option<String>,
}

pub fn search_page(page: Page<ProjectResponse>, offset: u32) -> SearchPage {
    SearchPage {
        hits: page
            .result
            .into_iter()
            .map(ProjectResponse::into_summary)
            .collect(),
        offset,
        total: u32::try_from(page.pagination.count).unwrap_or(u32::MAX),
    }
}

impl ProjectResponse {
    fn into_summary(self) -> ProjectSummary {
        ProjectSummary {
            source: SourceKind::Hangar,
            page_url: format!("{SITE}/{}/{}", self.namespace.owner, self.namespace.slug),
            id: self.id.to_string(),
            slug: self.namespace.slug,
            title: self.name,
            description: self.description.unwrap_or_default(),
            author: self.namespace.owner,
            icon_url: self.avatar_url,
            downloads: self.stats.downloads,
            categories: self.category.into_iter().collect(),
            updated: self.last_updated,
            // Plugins run on the server only.
            server_side: SideSupport::Required,
            client_side: SideSupport::Unsupported,
        }
    }

    pub fn into_project(self) -> Project {
        Project {
            source: SourceKind::Hangar,
            id: self.id.to_string(),
            page_url: format!("{SITE}/{}/{}", self.namespace.owner, self.namespace.slug),
            slug: self.namespace.slug,
            title: self.name,
            description: self.description.unwrap_or_default(),
            body: self.main_page_content.unwrap_or_default(),
            icon_url: self.avatar_url,
            server_side: SideSupport::Required,
            client_side: SideSupport::Unsupported,
        }
    }
}

impl VersionResponse {
    /// The version, if Hangar hosts its Paper file (one hosted elsewhere can't be checked or
    /// installed from here).
    pub fn into_version(mut self) -> Option<ContentVersion> {
        let download = self.downloads.remove(PLATFORM)?;
        let (url, file) = (download.download_url?, download.file_info?);
        let dependencies = self
            .plugin_dependencies
            .remove(PLATFORM)
            .unwrap_or_default()
            .into_iter()
            .map(|dependency| Dependency {
                project_id: dependency.project_id.map(|id| id.to_string()),
                name: dependency.name,
                version_id: None,
                kind: if dependency.required {
                    DependencyKind::Required
                } else {
                    DependencyKind::Optional
                },
            })
            .collect();
        Some(ContentVersion {
            id: self.id.to_string(),
            project_id: self.project_id.to_string(),
            name: self.name.clone(),
            channel: channel(&self.channel.name),
            version_number: self.name,
            published: self.created_at,
            game_versions: self.platform_dependencies.remove(PLATFORM).unwrap_or_default(),
            loaders: vec!["paper".into()],
            dependencies,
            file: ContentFile {
                url,
                filename: file.name,
                size: file.size_bytes,
                sha1: None,
                sha256: file.sha256_hash,
                sha512: None,
                manual_url: None,
            },
        })
    }
}

/// Hangar channels are named by each project ("Release", "Snapshot", "Beta", "Dev", …). Only
/// "Release" counts as a release; "Alpha" as alpha; anything else as a beta.
fn channel(name: &str) -> Channel {
    if name.eq_ignore_ascii_case("release") {
        Channel::Release
    } else if name.to_ascii_lowercase().contains("alpha") {
        Channel::Alpha
    } else {
        Channel::Beta
    }
}
