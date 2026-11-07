//! Quilt (a Fabric fork), via Quilt meta v3 and the Quilt installer from Quilt's Maven
//! (both verified 2026-11-07).
//!
//! Meta lists loader versions per game version **unsorted** (e.g. `0.20.0-beta.9` before
//! `0.24.0`), so they are ordered here. The server is installed by running the installer:
//! `install server <mc> <loader> --download-server --install-dir=.` (see `install::plan`).

use serde::Deserialize;

use super::{Artifact, CatalogError, Fetcher, LoaderVersion, maven, model::mark_recommended, version_order};

#[derive(Debug, Deserialize)]
struct GameEntry {
    version: String,
}

#[derive(Debug, Deserialize)]
struct LoaderEntry {
    loader: Loader,
}

#[derive(Debug, Deserialize)]
struct Loader {
    version: String,
}

pub(super) async fn supports(fetcher: &Fetcher, meta: &str, mc_version: &str) -> Result<bool, CatalogError> {
    let games: Vec<GameEntry> = fetcher.json(&format!("{meta}/v3/versions/game")).await?;
    Ok(games.iter().any(|game| game.version == mc_version))
}

/// Loader versions, newest first; betas are unstable; the newest stable one is recommended.
pub(super) async fn versions(
    fetcher: &Fetcher,
    meta: &str,
    mc_version: &str,
) -> Result<Vec<LoaderVersion>, CatalogError> {
    let entries: Vec<LoaderEntry> = fetcher
        .json(&format!("{meta}/v3/versions/loader/{mc_version}"))
        .await?;
    Ok(to_loader_versions(entries))
}

/// The newest released Quilt installer (it installs any loader version).
pub(super) async fn installer(fetcher: &Fetcher, maven_base: &str) -> Result<Artifact, CatalogError> {
    let root = format!("{maven_base}/org/quiltmc/quilt-installer");
    let metadata_url = format!("{root}/maven-metadata.xml");
    let xml = fetcher.text(&metadata_url).await?;
    let version = maven::release_version(&xml).ok_or(CatalogError::Malformed {
        url: metadata_url,
        detail: "no installer release".into(),
    })?;
    maven::artifact_with_sha1(fetcher, format!("{root}/{version}/quilt-installer-{version}.jar")).await
}

fn to_loader_versions(entries: Vec<LoaderEntry>) -> Vec<LoaderVersion> {
    let mut versions: Vec<LoaderVersion> = entries
        .into_iter()
        .map(|entry| LoaderVersion {
            stable: !version_order::is_pre_release(&entry.loader.version),
            id: entry.loader.version,
            released: None,
            recommended: false,
        })
        .collect();
    version_order::sort_newest_first(&mut versions, |v| &v.id);
    versions.dedup_by(|a, b| a.id == b.id);
    mark_recommended(versions, None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::fixture;

    #[test]
    fn orders_the_recorded_unsorted_list_and_recommends_the_newest_stable() {
        let entries: Vec<LoaderEntry> = serde_json::from_value(fixture("quilt_loaders_26.3.json")).unwrap();
        let count = entries.len();
        let versions = to_loader_versions(entries);
        assert_eq!(versions.len(), count, "no duplicates in the recorded list");
        assert!(
            versions
                .windows(2)
                .all(|w| version_order::compare(&w[0].id, &w[1].id).is_gt())
        );
        let recommended = versions.iter().find(|v| v.recommended).unwrap();
        assert!(recommended.stable);
        assert_eq!(recommended.id, "0.30.2", "the newest stable in the recorded list");
        assert!(versions.iter().any(|v| !v.stable), "betas are listed as unstable");
    }
}
