//! `CurseForge`'s JSON (`docs.curseforge.com`), and how it maps onto the source-independent model.

use chrono::{DateTime, Utc};
use serde::Deserialize;

use crate::model::{
    Channel, ContentFile, ContentVersion, Dependency, DependencyKind, Project, ProjectSummary, SearchPage,
    SideSupport, SourceKind,
};

/// The loaders `CurseForge` names among a file's game versions. A Bukkit plugin names none.
const MOD_LOADERS: [&str; 4] = ["forge", "fabric", "quilt", "neoforge"];

/// Every `CurseForge` answer wraps what was asked for.
#[derive(Debug, Deserialize)]
pub struct Data<T> {
    pub data: T,
}

#[derive(Debug, Deserialize)]
pub struct SearchResponse {
    data: Vec<ModResponse>,
    pagination: Pagination,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Pagination {
    total_count: u64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModResponse {
    id: u64,
    name: String,
    slug: String,
    #[serde(default)]
    summary: String,
    /// A whole number, though the API's schema has called it a double.
    #[serde(default)]
    download_count: f64,
    #[serde(default)]
    links: Links,
    #[serde(default)]
    authors: Vec<Author>,
    #[serde(default)]
    logo: Option<Logo>,
    #[serde(default)]
    categories: Vec<Category>,
    #[serde(default)]
    date_modified: Option<DateTime<Utc>>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Links {
    #[serde(default)]
    website_url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Author {
    name: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Logo {
    #[serde(default)]
    thumbnail_url: Option<String>,
    #[serde(default)]
    url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Category {
    name: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileResponse {
    id: u64,
    mod_id: u64,
    display_name: String,
    file_name: String,
    release_type: u8,
    #[serde(default)]
    file_date: Option<DateTime<Utc>>,
    #[serde(default)]
    file_length: u64,
    /// Absent when the author doesn't allow other apps to download the file.
    #[serde(default)]
    download_url: Option<String>,
    #[serde(default)]
    hashes: Vec<FileHash>,
    /// Minecraft versions, loaders and sides, all in one list.
    #[serde(default)]
    game_versions: Vec<String>,
    #[serde(default)]
    dependencies: Vec<FileDependency>,
    #[serde(default = "available")]
    is_available: bool,
    #[serde(default)]
    is_server_pack: Option<bool>,
    /// On a modpack's file: the file holding its server version, if the author made one.
    #[serde(default)]
    server_pack_file_id: Option<u64>,
    #[serde(default)]
    pub file_fingerprint: u64,
}

fn available() -> bool {
    true
}

#[derive(Debug, Deserialize)]
struct FileHash {
    value: String,
    algo: u8,
}

/// `algo` of a SHA-1 (2 is MD5, which isn't used).
const ALGO_SHA1: u8 = 1;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FileDependency {
    mod_id: u64,
    relation_type: u8,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FingerprintMatches {
    #[serde(default)]
    pub exact_matches: Vec<FingerprintMatch>,
}

#[derive(Debug, Deserialize)]
pub struct FingerprintMatch {
    pub file: FileResponse,
}

impl From<SearchResponse> for SearchPageParts {
    fn from(response: SearchResponse) -> Self {
        Self {
            hits: response.data.into_iter().map(ModResponse::into_summary).collect(),
            total: u32::try_from(response.pagination.total_count).unwrap_or(u32::MAX),
        }
    }
}

/// A search page, short of the offset the caller asked for.
pub struct SearchPageParts {
    hits: Vec<ProjectSummary>,
    total: u32,
}

impl SearchPageParts {
    pub fn at(self, offset: u32) -> SearchPage {
        SearchPage {
            hits: self.hits,
            offset,
            total: self.total,
        }
    }
}

impl ModResponse {
    fn page_url(&self) -> String {
        self.links
            .website_url
            .clone()
            .unwrap_or_else(|| format!("https://www.curseforge.com/minecraft/search?search={}", self.slug))
    }

    fn icon_url(&self) -> Option<String> {
        let logo = self.logo.as_ref()?;
        logo.thumbnail_url.clone().or_else(|| logo.url.clone())
    }

    fn into_summary(self) -> ProjectSummary {
        // A count of downloads never comes near the range where this loses anything.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let downloads = self.download_count as u64;
        ProjectSummary {
            source: SourceKind::CurseForge,
            page_url: self.page_url(),
            icon_url: self.icon_url(),
            id: self.id.to_string(),
            slug: self.slug,
            title: self.name,
            description: self.summary,
            author: self
                .authors
                .into_iter()
                .next()
                .map_or_else(String::new, |author| author.name),
            downloads,
            categories: self
                .categories
                .into_iter()
                .map(|category| category.name)
                .collect(),
            updated: self.date_modified,
            // CurseForge doesn't say which side a project is for.
            server_side: SideSupport::Unknown,
            client_side: SideSupport::Unknown,
        }
    }

    pub fn into_project(self) -> Project {
        Project {
            source: SourceKind::CurseForge,
            page_url: self.page_url(),
            icon_url: self.icon_url(),
            id: self.id.to_string(),
            slug: self.slug,
            title: self.name,
            description: self.summary,
            body: String::new(),
            server_side: SideSupport::Unknown,
            client_side: SideSupport::Unknown,
        }
    }
}

impl FileResponse {
    /// Whether this is a file to offer at all.
    pub fn is_offered(&self) -> bool {
        self.is_available && self.is_server_pack != Some(true)
    }

    pub fn is_blocked(&self) -> bool {
        self.download_url.is_none()
    }

    /// The file holding this pack's server version. `CurseForge` can say 0 for "none".
    pub fn server_pack(&self) -> Option<u64> {
        self.server_pack_file_id.filter(|id| *id != 0)
    }

    /// The model's version. `project_page` is where a blocked file can be downloaded by hand.
    pub fn into_version(self, project_page: Option<&str>) -> ContentVersion {
        let channel = match self.release_type {
            2 => Channel::Beta,
            3 => Channel::Alpha,
            _ => Channel::Release,
        };
        let sha1 = self
            .hashes
            .iter()
            .find(|hash| hash.algo == ALGO_SHA1)
            .map(|hash| hash.value.to_lowercase());
        let manual_url = self
            .download_url
            .is_none()
            .then(|| project_page.map(|page| format!("{page}/files/{}", self.id)))
            .flatten();
        let mut loaders: Vec<String> = self
            .game_versions
            .iter()
            .map(|tag| tag.to_lowercase())
            .filter(|tag| MOD_LOADERS.contains(&tag.as_str()))
            .collect();
        if loaders.is_empty() {
            loaders.push("bukkit".into());
        }
        ContentVersion {
            id: self.id.to_string(),
            project_id: self.mod_id.to_string(),
            name: self.display_name.clone(),
            version_number: self.display_name,
            channel,
            published: self.file_date,
            game_versions: self
                .game_versions
                .into_iter()
                .filter(|tag| tag.starts_with(|c: char| c.is_ascii_digit()))
                .collect(),
            loaders,
            dependencies: self
                .dependencies
                .into_iter()
                .filter_map(FileDependency::into_dependency)
                .collect(),
            file: ContentFile {
                url: self.download_url.unwrap_or_default(),
                filename: self.file_name,
                size: self.file_length,
                sha1,
                sha256: None,
                sha512: None,
                manual_url,
            },
        }
    }
}

impl FileDependency {
    fn into_dependency(self) -> Option<Dependency> {
        let kind = match self.relation_type {
            1 | 6 => DependencyKind::Embedded,
            2 | 4 => DependencyKind::Optional,
            3 => DependencyKind::Required,
            5 => DependencyKind::Incompatible,
            _ => return None,
        };
        Some(Dependency {
            project_id: Some(self.mod_id.to_string()),
            name: None,
            version_id: None,
            kind,
        })
    }
}
