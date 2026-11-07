//! `NeoForge` (verified against the live Maven API 2026-11-07).
//!
//! One list of every `NeoForge` version (oldest first) comes from
//! `maven.neoforged.net/api/maven/versions/releases/net/neoforged/neoforge`. The Minecraft
//! version is encoded in the `NeoForge` version:
//! - 1.20.2 – 1.21.x: drop Minecraft's leading `1.`: `21.1.77` → `1.21.1`, `21.0.5` → `1.21`.
//! - 26.x onwards: `<year>.<drop>.<hotfix>.<build>`: `26.3.0.101` → `26.3`, `26.1.2.71` → `26.1.2`.
//!
//! Versions with a suffix (`-beta`, `+snapshot-3`) are unstable; ones that map to no release
//! (April Fools builds, snapshots) are skipped. Minecraft 1.20.1 used a different artifact
//! (`net.neoforged:forge`) and is not offered.

use serde::Deserialize;

use super::{Artifact, CatalogError, Fetcher, LoaderVersion, maven, model::mark_recommended, version_order};
use crate::loader::LoaderKind;

#[derive(Debug, Deserialize)]
struct VersionList {
    versions: Vec<String>,
}

pub(super) async fn supports(
    fetcher: &Fetcher,
    maven_base: &str,
    mc_version: &str,
) -> Result<bool, CatalogError> {
    let list = version_list(fetcher, maven_base).await?;
    Ok(list
        .iter()
        .any(|v| minecraft_version(v).as_deref() == Some(mc_version)))
}

/// `NeoForge` versions for `mc_version`, newest first; the newest stable one is recommended.
pub(super) async fn versions(
    fetcher: &Fetcher,
    maven_base: &str,
    mc_version: &str,
) -> Result<Vec<LoaderVersion>, CatalogError> {
    Ok(to_loader_versions(
        &version_list(fetcher, maven_base).await?,
        mc_version,
    ))
}

pub(super) async fn installer(
    fetcher: &Fetcher,
    maven_base: &str,
    mc_version: &str,
    version: &str,
) -> Result<Artifact, CatalogError> {
    let list = version_list(fetcher, maven_base).await?;
    if !list.iter().any(|v| v == version) || minecraft_version(version).as_deref() != Some(mc_version) {
        return Err(CatalogError::UnknownLoaderVersion {
            loader: LoaderKind::NeoForge,
            mc_version: mc_version.into(),
            version: version.into(),
        });
    }
    let url =
        format!("{maven_base}/releases/net/neoforged/neoforge/{version}/neoforge-{version}-installer.jar");
    maven::artifact_with_sha1(fetcher, url).await
}

async fn version_list(fetcher: &Fetcher, maven_base: &str) -> Result<Vec<String>, CatalogError> {
    let url = format!("{maven_base}/api/maven/versions/releases/net/neoforged/neoforge");
    let list: VersionList = fetcher.json(&url).await?;
    Ok(list.versions)
}

/// The Minecraft release a `NeoForge` version is built for, if it is one.
pub(crate) fn minecraft_version(neoforge: &str) -> Option<String> {
    let (core, _) = neoforge.split_once(['-', '+']).unwrap_or((neoforge, ""));
    let parts: Vec<u32> = core.split('.').map(str::parse).collect::<Result<_, _>>().ok()?;
    match parts.as_slice() {
        // 26.x: year.drop.hotfix.build
        [year, drop, hotfix, _build] if *year >= 26 => Some(if *hotfix == 0 {
            format!("{year}.{drop}")
        } else {
            format!("{year}.{drop}.{hotfix}")
        }),
        // 1.20.2 – 1.21.x: minor.patch.build
        [minor @ 20..=25, patch, _build] => Some(if *patch == 0 {
            format!("1.{minor}")
        } else {
            format!("1.{minor}.{patch}")
        }),
        _ => None,
    }
}

fn to_loader_versions(all: &[String], mc_version: &str) -> Vec<LoaderVersion> {
    let mut versions: Vec<LoaderVersion> = all
        .iter()
        .filter(|v| minecraft_version(v).as_deref() == Some(mc_version))
        .map(|v| LoaderVersion {
            id: v.clone(),
            released: None,
            stable: !version_order::is_pre_release(v),
            recommended: false,
        })
        .collect();
    version_order::sort_newest_first(&mut versions, |v| &v.id);
    mark_recommended(versions, None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::fixture;

    #[test]
    fn maps_versions_to_minecraft_releases() {
        let cases = [
            ("26.3.0.101", Some("26.3")),
            ("26.1.2.126", Some("26.1.2")),
            ("26.2.0.89", Some("26.2")),
            ("26.3.0.64-beta", Some("26.3")),
            ("21.1.77", Some("1.21.1")),
            ("21.0.167", Some("1.21")),
            ("20.4.251", Some("1.20.4")),
            ("20.2.3-beta", Some("1.20.2")),
            ("0.25w14craftmine.3-beta", None),
            ("garbage", None),
            ("1.2", None),
        ];
        for (neoforge, minecraft) in cases {
            assert_eq!(minecraft_version(neoforge).as_deref(), minecraft, "{neoforge}");
        }
    }

    #[test]
    fn lists_recorded_versions_for_one_release() {
        let list: VersionList = serde_json::from_value(fixture("neoforge_versions.json")).unwrap();
        let versions = to_loader_versions(&list.versions, "26.3");
        assert_eq!(versions[0].id, "26.3.0.101");
        assert!(versions.iter().all(|v| v.id.starts_with("26.3.0.")));
        assert!(versions.iter().any(|v| !v.stable), "betas are listed as unstable");
        let recommended = versions.iter().find(|v| v.recommended).unwrap();
        assert_eq!(recommended.id, "26.3.0.101");

        assert!(to_loader_versions(&list.versions, "1.21.1").len() > 100);
        assert!(
            to_loader_versions(&list.versions, "1.20.1").is_empty(),
            "legacy artifact"
        );
    }
}
