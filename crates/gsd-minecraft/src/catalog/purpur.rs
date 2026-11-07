//! Purpur (a Paper fork), via the Purpur API v2 (verified against the live API 2026-11-07).
//!
//! `/v2/purpur` lists Minecraft versions, `/v2/purpur/{mc}` the build numbers (`latest` +
//! `all`, oldest first), `/v2/purpur/{mc}/{build}` one build with its MD5 and result, and
//! `/v2/purpur/{mc}/{build}/download` the jar. MD5 is the only checksum Purpur publishes.

use gsd_core::net::ExpectedHash;
use serde::Deserialize;

use super::{Artifact, CatalogError, Fetcher, LoaderVersion, model::mark_recommended};
use crate::loader::LoaderKind;

#[derive(Debug, Deserialize)]
struct Project {
    versions: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct VersionBuilds {
    builds: Builds,
}

#[derive(Debug, Deserialize)]
struct Builds {
    latest: String,
    all: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct Build {
    result: String,
    md5: String,
}

pub(super) async fn supports(fetcher: &Fetcher, base: &str, mc_version: &str) -> Result<bool, CatalogError> {
    let project: Project = fetcher.json(&format!("{base}/v2/purpur")).await?;
    Ok(project.versions.iter().any(|v| v == mc_version))
}

/// Build numbers, newest first; Purpur's `latest` is recommended.
pub(super) async fn versions(
    fetcher: &Fetcher,
    base: &str,
    mc_version: &str,
) -> Result<Vec<LoaderVersion>, CatalogError> {
    let builds: VersionBuilds = fetcher.json(&format!("{base}/v2/purpur/{mc_version}")).await?;
    Ok(to_loader_versions(&builds.builds))
}

/// The jar of one build. Builds that failed upstream are refused.
pub(super) async fn artifact(
    fetcher: &Fetcher,
    base: &str,
    mc_version: &str,
    build: &str,
) -> Result<Artifact, CatalogError> {
    let url = format!("{base}/v2/purpur/{mc_version}/{build}");
    let detail: Build = fetcher.json(&url).await.map_err(|error| match error {
        CatalogError::Status { status: 404, .. } => CatalogError::UnknownLoaderVersion {
            loader: LoaderKind::Purpur,
            mc_version: mc_version.into(),
            version: build.into(),
        },
        other => other,
    })?;
    if !detail.result.eq_ignore_ascii_case("SUCCESS") {
        return Err(CatalogError::BrokenBuild {
            loader: LoaderKind::Purpur,
            mc_version: mc_version.into(),
            version: build.into(),
        });
    }
    Ok(Artifact {
        url: format!("{url}/download"),
        hash: ExpectedHash::Md5(detail.md5),
    })
}

fn to_loader_versions(builds: &Builds) -> Vec<LoaderVersion> {
    let versions = builds
        .all
        .iter()
        .rev()
        .map(|id| LoaderVersion {
            id: id.clone(),
            released: None,
            stable: true,
            recommended: false,
        })
        .collect();
    mark_recommended(versions, Some(&builds.latest))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::fixture;

    #[test]
    fn lists_recorded_builds_newest_first_with_latest_recommended() {
        let builds: VersionBuilds = serde_json::from_value(fixture("purpur_builds_26.3.json")).unwrap();
        let versions = to_loader_versions(&builds.builds);
        assert_eq!(versions[0].id, "2657");
        assert_eq!(versions.last().unwrap().id, "2634");
        let recommended: Vec<_> = versions.iter().filter(|v| v.recommended).collect();
        assert_eq!(recommended.len(), 1);
        assert_eq!(recommended[0].id, builds.builds.latest);
    }

    #[test]
    fn parses_a_recorded_build() {
        let build: Build = serde_json::from_value(fixture("purpur_build_26.3_2657.json")).unwrap();
        assert_eq!(build.result, "SUCCESS");
        assert_eq!(build.md5.len(), 32);
    }
}
