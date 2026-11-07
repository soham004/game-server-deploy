//! Minecraft Forge (verified against the live endpoints 2026-11-07).
//!
//! - `files.minecraftforge.net/…/maven-metadata.json`: Minecraft version → Maven versions,
//!   oldest first (`"26.3": ["26.3-66.1.0", …]`; old ones carry a suffix:
//!   `"1.7.10-10.13.4.1614-1.7.10"`).
//! - `…/promotions_slim.json`: `"<mc>-recommended"` / `"<mc>-latest"` → Forge version (★).
//! - The installer: `maven.minecraftforge.net/…/forge/<maven>/forge-<maven>-installer.jar`,
//!   with a `.sha1` side file.
//!
//! Versions are shown without the Minecraft prefix (`66.1.0`), the way Prism shows them.

use std::collections::HashMap;

use serde::Deserialize;

use super::{Artifact, CatalogError, Fetcher, LoaderVersion, maven, model::mark_recommended, version_order};
use crate::loader::LoaderKind;

/// The oldest Minecraft version offered. Older Forge builds have no server installer.
const OLDEST_SUPPORTED_MC: &str = "1.7.10";

#[derive(Debug, Deserialize)]
struct Promotions {
    promos: HashMap<String, String>,
}

type MavenVersions = HashMap<String, Vec<String>>;

pub(super) async fn supports(fetcher: &Fetcher, files: &str, mc_version: &str) -> Result<bool, CatalogError> {
    let all: MavenVersions = fetcher.json(&metadata_url(files)).await?;
    Ok(is_supported(&all, mc_version))
}

/// Forge versions, newest first; the promoted "recommended" (else "latest") one is starred.
pub(super) async fn versions(
    fetcher: &Fetcher,
    files: &str,
    mc_version: &str,
) -> Result<Vec<LoaderVersion>, CatalogError> {
    let all: MavenVersions = fetcher.json(&metadata_url(files)).await?;
    let promotions: Promotions = fetcher
        .json(&format!("{files}/net/minecraftforge/forge/promotions_slim.json"))
        .await?;
    Ok(to_loader_versions(&all, &promotions, mc_version))
}

/// The installer for Forge `version` (as shown, without the Minecraft prefix).
pub(super) async fn installer(
    fetcher: &Fetcher,
    files: &str,
    maven_base: &str,
    mc_version: &str,
    version: &str,
) -> Result<Artifact, CatalogError> {
    let all: MavenVersions = fetcher.json(&metadata_url(files)).await?;
    let maven_version = format!("{mc_version}-{version}");
    if !all
        .get(mc_version)
        .is_some_and(|list| list.contains(&maven_version))
    {
        return Err(CatalogError::UnknownLoaderVersion {
            loader: LoaderKind::Forge,
            mc_version: mc_version.into(),
            version: version.into(),
        });
    }
    let url =
        format!("{maven_base}/net/minecraftforge/forge/{maven_version}/forge-{maven_version}-installer.jar");
    maven::artifact_with_sha1(fetcher, url).await
}

fn metadata_url(files: &str) -> String {
    format!("{files}/net/minecraftforge/forge/maven-metadata.json")
}

fn is_supported(all: &MavenVersions, mc_version: &str) -> bool {
    all.get(mc_version).is_some_and(|list| !list.is_empty())
        && version_order::compare(mc_version, OLDEST_SUPPORTED_MC).is_ge()
}

fn to_loader_versions(all: &MavenVersions, promotions: &Promotions, mc_version: &str) -> Vec<LoaderVersion> {
    if !is_supported(all, mc_version) {
        return Vec::new();
    }
    let prefix = format!("{mc_version}-");
    let mut versions: Vec<LoaderVersion> = all
        .get(mc_version)
        .into_iter()
        .flatten()
        .filter_map(|maven| maven.strip_prefix(&prefix))
        .map(|version| LoaderVersion {
            id: version.to_owned(),
            released: None,
            stable: true,
            recommended: false,
        })
        .collect();
    version_order::sort_newest_first(&mut versions, |v| &v.id);
    let promoted = promotions
        .promos
        .get(&format!("{mc_version}-recommended"))
        .or_else(|| promotions.promos.get(&format!("{mc_version}-latest")));
    mark_recommended(versions, promoted.map(String::as_str))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::fixture;

    fn recorded() -> (MavenVersions, Promotions) {
        (
            serde_json::from_value(fixture("forge_maven_metadata.json")).unwrap(),
            serde_json::from_value(fixture("forge_promotions.json")).unwrap(),
        )
    }

    #[test]
    fn strips_the_minecraft_prefix_and_stars_the_recommended_build() {
        let (all, promotions) = recorded();
        let versions = to_loader_versions(&all, &promotions, "26.3");
        assert_eq!(versions[0].id, "66.1.5", "newest first");
        let starred: Vec<_> = versions
            .iter()
            .filter(|v| v.recommended)
            .map(|v| v.id.as_str())
            .collect();
        assert_eq!(
            starred,
            ["66.1.0"],
            "promotions_slim says 26.3-recommended = 66.1.0"
        );
    }

    #[test]
    fn falls_back_to_the_latest_promotion() {
        let (all, promotions) = recorded();
        // 26.1 has only a "-latest" promotion (62.0.9).
        let versions = to_loader_versions(&all, &promotions, "26.1");
        assert_eq!(
            versions.iter().find(|v| v.recommended).map(|v| v.id.as_str()),
            Some("62.0.9")
        );
    }

    #[test]
    fn keeps_old_style_suffixes_and_hides_ancient_versions() {
        let (all, promotions) = recorded();
        let legacy = to_loader_versions(&all, &promotions, "1.7.10");
        assert!(legacy.iter().any(|v| v.id.ends_with("-1.7.10")), "{legacy:?}");
        assert!(is_supported(&all, "1.12.2"));
        assert!(!is_supported(&all, "1.5.2"), "too old for the server installer");
        assert!(!is_supported(&all, "9.9"));
        assert_eq!(
            to_loader_versions(&all, &promotions, "1.5.2"),
            Vec::<LoaderVersion>::new()
        );
    }
}
