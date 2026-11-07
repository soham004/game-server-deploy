//! Fabric meta API.
//!
//! The server is a single "launcher" jar built on demand by Fabric meta from (Minecraft version,
//! loader version, installer version); on first start it downloads the vanilla server itself.

use gsd_core::net::ExpectedHash;
use serde::Deserialize;

use super::{Artifact, CatalogError, Fetcher, LoaderVersion, model::mark_recommended};
use crate::loader::LoaderKind;

#[derive(Debug, Deserialize)]
struct GameEntry {
    version: String,
}

#[derive(Debug, Deserialize)]
struct LoaderEntry {
    loader: VersionEntry,
}

#[derive(Debug, Deserialize)]
struct VersionEntry {
    version: String,
    stable: bool,
}

pub(super) async fn supports(fetcher: &Fetcher, base: &str, mc_version: &str) -> Result<bool, CatalogError> {
    let games: Vec<GameEntry> = fetcher.json(&format!("{base}/v2/versions/game")).await?;
    Ok(games.iter().any(|game| game.version == mc_version))
}

/// Loader versions, newest first (meta's order); the newest stable one is recommended.
pub(super) async fn versions(
    fetcher: &Fetcher,
    base: &str,
    mc_version: &str,
) -> Result<Vec<LoaderVersion>, CatalogError> {
    let entries: Vec<LoaderEntry> = fetcher
        .json(&format!("{base}/v2/versions/loader/{mc_version}"))
        .await?;
    let versions = entries
        .into_iter()
        .map(|entry| LoaderVersion {
            id: entry.loader.version,
            released: None,
            stable: entry.loader.stable,
            recommended: false,
        })
        .collect();
    Ok(mark_recommended(versions, None))
}

/// The server launcher jar for `loader_version` with the newest stable installer.
pub(super) async fn artifact(
    fetcher: &Fetcher,
    base: &str,
    mc_version: &str,
    loader_version: &str,
) -> Result<Artifact, CatalogError> {
    let known = versions(fetcher, base, mc_version).await?;
    if !known.iter().any(|v| v.id == loader_version) {
        return Err(CatalogError::UnknownLoaderVersion {
            loader: LoaderKind::Fabric,
            mc_version: mc_version.into(),
            version: loader_version.into(),
        });
    }
    let url = format!("{base}/v2/versions/installer");
    let installers: Vec<VersionEntry> = fetcher.json(&url).await?;
    let installer = pick_installer(&installers).ok_or(CatalogError::Malformed {
        url,
        detail: "no installer versions".into(),
    })?;
    // WHY: Fabric meta builds this jar on demand and publishes no checksum; it is small and
    // served over HTTPS. On first start it downloads (and verifies) the vanilla server itself.
    Ok(Artifact {
        url: server_jar_url(base, mc_version, loader_version, installer),
        hash: ExpectedHash::None,
    })
}

/// Newest stable installer, else the newest one.
fn pick_installer(installers: &[VersionEntry]) -> Option<&str> {
    installers
        .iter()
        .find(|i| i.stable)
        .or_else(|| installers.first())
        .map(|i| i.version.as_str())
}

fn server_jar_url(base: &str, mc_version: &str, loader: &str, installer: &str) -> String {
    format!("{base}/v2/versions/loader/{mc_version}/{loader}/{installer}/server/jar")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::fixture;

    #[test]
    fn picks_the_newest_stable_installer() {
        let installers: Vec<VersionEntry> =
            serde_json::from_value(fixture("fabric_installers.json")).unwrap();
        assert_eq!(pick_installer(&installers), Some("1.1.2"));
        assert_eq!(pick_installer(&[]), None);
    }

    #[test]
    fn builds_the_launcher_url() {
        assert_eq!(
            server_jar_url("https://meta.fabricmc.net", "26.3", "0.20.1", "1.1.2"),
            "https://meta.fabricmc.net/v2/versions/loader/26.3/0.20.1/1.1.2/server/jar"
        );
    }
}
