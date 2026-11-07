//! Mojang's version manifest and per-version metadata.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::{CatalogError, GameVersion};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VersionKind {
    Release,
    Snapshot,
    OldBeta,
    OldAlpha,
}

#[derive(Debug, Deserialize)]
pub(super) struct Manifest {
    versions: Vec<ManifestEntry>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ManifestEntry {
    id: String,
    #[serde(rename = "type")]
    kind: VersionKind,
    url: String,
    release_time: DateTime<Utc>,
}

impl Manifest {
    /// Newest first, by release time (version ids don't sort: "1.21.11" < "26.1" < "26.4-snapshot-7").
    pub(super) fn versions(&self) -> Vec<GameVersion> {
        let mut versions: Vec<GameVersion> = self
            .versions
            .iter()
            .map(|entry| GameVersion {
                id: entry.id.clone(),
                kind: entry.kind,
                released: entry.release_time,
            })
            .collect();
        versions.sort_by_key(|version| std::cmp::Reverse(version.released));
        versions
    }

    pub(super) fn find(&self, id: &str) -> Option<&str> {
        self.versions
            .iter()
            .find(|entry| entry.id == id)
            .map(|entry| entry.id.as_str())
    }

    pub(super) fn detail_url(&self, id: &str) -> Option<&str> {
        self.versions
            .iter()
            .find(|entry| entry.id == id)
            .map(|entry| entry.url.as_str())
    }
}

/// Per-version metadata (only the parts a server needs).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionDetail {
    pub id: String,
    #[serde(default)]
    java_version: Option<JavaVersion>,
    #[serde(default)]
    downloads: Downloads,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct JavaVersion {
    major_version: u32,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct Downloads {
    server: Option<ServerDownload>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ServerDownload {
    pub url: String,
    pub sha1: String,
    pub size: u64,
}

impl VersionDetail {
    /// The Java major this version needs. Old versions don't say; they all run on Java 8.
    pub fn java_major(&self) -> u32 {
        self.java_version.as_ref().map_or(8, |java| java.major_version)
    }

    pub fn server_download(&self) -> Result<&ServerDownload, CatalogError> {
        self.downloads
            .server
            .as_ref()
            .ok_or_else(|| CatalogError::NoServerDownload(self.id.clone()))
    }
}
