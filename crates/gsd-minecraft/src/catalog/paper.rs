//! `PaperMC` Fill v3 API (the v2 API was shut down on 2026-07-01).
//!
//! Rules from `PaperMC`: send a User-Agent, prefer `STABLE` builds, and always use the download
//! URL from the response rather than building one.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::Deserialize;

use super::LoaderVersion;

#[derive(Debug, Deserialize)]
pub(super) struct Project {
    /// Version group → versions, newest first.
    versions: HashMap<String, Vec<String>>,
}

impl Project {
    pub(super) fn all_versions(self) -> Vec<String> {
        self.versions.into_values().flatten().collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct PaperBuild {
    pub id: u32,
    pub time: DateTime<Utc>,
    pub channel: String,
    pub downloads: HashMap<String, PaperDownload>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct PaperDownload {
    pub name: String,
    pub url: String,
    pub checksums: PaperChecksums,
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct PaperChecksums {
    pub sha256: String,
}

impl PaperBuild {
    pub fn is_stable(&self) -> bool {
        self.channel.eq_ignore_ascii_case("STABLE")
    }

    /// The server jar (`server:default`).
    pub fn server_download(&self) -> Option<&PaperDownload> {
        self.downloads.get("server:default")
    }
}

/// Newest build first.
pub(super) fn sorted(mut builds: Vec<PaperBuild>) -> Vec<PaperBuild> {
    builds.sort_by_key(|build| std::cmp::Reverse(build.id));
    builds
}

/// The newest stable build, or the newest build if none is stable.
pub fn pick_build(builds: &[PaperBuild]) -> Option<&PaperBuild> {
    builds
        .iter()
        .find(|build| build.is_stable())
        .or_else(|| builds.first())
}

pub(super) fn to_loader_versions(builds: &[PaperBuild]) -> Vec<LoaderVersion> {
    let recommended = pick_build(builds).map(|build| build.id);
    builds
        .iter()
        .map(|build| LoaderVersion {
            id: build.id.to_string(),
            released: Some(build.time),
            stable: build.is_stable(),
            recommended: Some(build.id) == recommended,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build(id: u32, channel: &str) -> PaperBuild {
        PaperBuild {
            id,
            time: Utc::now(),
            channel: channel.into(),
            downloads: HashMap::new(),
        }
    }

    #[test]
    fn prefers_newest_stable_then_newest() {
        let builds = sorted(vec![build(10, "STABLE"), build(12, "ALPHA"), build(11, "STABLE")]);
        assert_eq!(pick_build(&builds).unwrap().id, 11);
        let experimental_only = sorted(vec![build(1, "ALPHA"), build(2, "BETA")]);
        assert_eq!(pick_build(&experimental_only).unwrap().id, 2);
        assert!(pick_build(&[]).is_none());
    }

    #[test]
    fn parses_the_fixture() {
        let builds: Vec<PaperBuild> =
            serde_json::from_value(crate::catalog::tests::fixture("paper_builds_26.3.json")).unwrap();
        let download = builds[0].server_download().unwrap();
        assert!(download.url.starts_with("https://fill-data.papermc.io/"));
        assert_eq!(download.checksums.sha256.len(), 64);
    }
}
