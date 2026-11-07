//! Paper, via the `PaperMC` Fill v3 API (the v2 API was shut down on 2026-07-01).
//!
//! Rules from `PaperMC`: send a User-Agent, prefer `STABLE` builds, and always use the download
//! URL from the response rather than building one.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use gsd_core::net::ExpectedHash;
use serde::Deserialize;

use super::{Artifact, CatalogError, Fetcher, LoaderVersion, model::mark_recommended};
use crate::loader::LoaderKind;

#[derive(Debug, Deserialize)]
struct Project {
    /// Version group → versions, newest first.
    versions: HashMap<String, Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
struct Build {
    id: u32,
    time: DateTime<Utc>,
    channel: String,
    downloads: HashMap<String, Download>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
struct Download {
    url: String,
    checksums: Checksums,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
struct Checksums {
    sha256: String,
}

impl Build {
    fn is_stable(&self) -> bool {
        self.channel.eq_ignore_ascii_case("STABLE")
    }
}

pub(super) async fn supports(fetcher: &Fetcher, base: &str, mc_version: &str) -> Result<bool, CatalogError> {
    let project: Project = fetcher.json(&format!("{base}/v3/projects/paper")).await?;
    Ok(project.versions.values().flatten().any(|v| v == mc_version))
}

/// Builds, newest first; the newest stable one is recommended.
pub(super) async fn versions(
    fetcher: &Fetcher,
    base: &str,
    mc_version: &str,
) -> Result<Vec<LoaderVersion>, CatalogError> {
    Ok(to_loader_versions(&builds(fetcher, base, mc_version).await?))
}

/// The server jar of one build, with its SHA-256.
pub(super) async fn artifact(
    fetcher: &Fetcher,
    base: &str,
    mc_version: &str,
    build_id: &str,
) -> Result<Artifact, CatalogError> {
    let builds = builds(fetcher, base, mc_version).await?;
    let download = builds
        .iter()
        .find(|build| build.id.to_string() == build_id)
        .and_then(|build| build.downloads.get("server:default"))
        .ok_or_else(|| CatalogError::UnknownLoaderVersion {
            loader: LoaderKind::Paper,
            mc_version: mc_version.into(),
            version: build_id.into(),
        })?;
    Ok(Artifact {
        url: download.url.clone(),
        hash: ExpectedHash::Sha256(download.checksums.sha256.clone()),
    })
}

async fn builds(fetcher: &Fetcher, base: &str, mc_version: &str) -> Result<Vec<Build>, CatalogError> {
    let url = format!("{base}/v3/projects/paper/versions/{mc_version}/builds");
    let mut builds: Vec<Build> = fetcher.json(&url).await?;
    builds.sort_by_key(|build| std::cmp::Reverse(build.id));
    Ok(builds)
}

fn to_loader_versions(builds: &[Build]) -> Vec<LoaderVersion> {
    let versions = builds
        .iter()
        .map(|build| LoaderVersion {
            id: build.id.to_string(),
            released: Some(build.time),
            stable: build.is_stable(),
            recommended: false,
        })
        .collect();
    mark_recommended(versions, None)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build(id: u32, channel: &str) -> Build {
        Build {
            id,
            time: Utc::now(),
            channel: channel.into(),
            downloads: HashMap::new(),
        }
    }

    #[test]
    fn recommends_the_newest_stable_build() {
        let builds = [build(12, "ALPHA"), build(11, "STABLE"), build(10, "STABLE")];
        let versions = to_loader_versions(&builds);
        assert_eq!(versions.iter().find(|v| v.recommended).unwrap().id, "11");
        assert!(!versions[0].stable);
    }

    #[test]
    fn parses_the_recorded_builds() {
        let builds: Vec<Build> =
            serde_json::from_value(crate::test_support::fixture("paper_builds_26.3.json")).unwrap();
        let download = &builds[0].downloads["server:default"];
        assert!(download.url.starts_with("https://fill-data.papermc.io/"));
        assert_eq!(download.checksums.sha256.len(), 64);
    }
}
