//! Catalog types shared by every source: upstream endpoints, errors, and what the version
//! picker and the installer receive.

use chrono::{DateTime, Utc};
use gsd_core::net::ExpectedHash;
use serde::Serialize;

use super::mojang::VersionKind;
use crate::loader::{ContentKind, LoaderKind};

/// Upstream base URLs (overridable for tests).
#[derive(Debug, Clone)]
pub struct Endpoints {
    pub mojang_manifest: String,
    /// `PaperMC` Fill v3.
    pub paper: String,
    pub purpur: String,
    /// Fabric meta.
    pub fabric: String,
    pub quilt_meta: String,
    /// Quilt's Maven repository (the installer).
    pub quilt_maven: String,
    /// Forge's version lists (`maven-metadata.json`, promotions).
    pub forge_files: String,
    pub forge_maven: String,
    pub neoforge_maven: String,
}

impl Default for Endpoints {
    fn default() -> Self {
        Self {
            mojang_manifest: "https://piston-meta.mojang.com/mc/game/version_manifest_v2.json".into(),
            paper: "https://fill.papermc.io".into(),
            purpur: "https://api.purpurmc.org".into(),
            fabric: "https://meta.fabricmc.net".into(),
            quilt_meta: "https://meta.quiltmc.org".into(),
            quilt_maven: "https://maven.quiltmc.org/repository/release".into(),
            forge_files: "https://files.minecraftforge.net".into(),
            forge_maven: "https://maven.minecraftforge.net".into(),
            neoforge_maven: "https://maven.neoforged.net".into(),
        }
    }
}

impl Endpoints {
    /// Every source served by one host (tests: a single mock server). The Mojang manifest is
    /// expected at `/mojang/manifest.json`; the other sources use their real paths, which don't
    /// overlap.
    pub fn all_at(base: &str) -> Self {
        Self {
            mojang_manifest: format!("{base}/mojang/manifest.json"),
            paper: base.into(),
            purpur: base.into(),
            fabric: base.into(),
            quilt_meta: base.into(),
            quilt_maven: base.into(),
            forge_files: base.into(),
            forge_maven: base.into(),
            neoforge_maven: base.into(),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum CatalogError {
    #[error("could not reach {url} and nothing is cached")]
    Unreachable {
        url: String,
        #[source]
        source: reqwest::Error,
    },
    #[error("{url} answered HTTP {status}")]
    Status { url: String, status: u16 },
    #[error("unexpected response from {url}: {detail}")]
    Malformed { url: String, detail: String },
    #[error("Minecraft {0} does not exist")]
    UnknownVersion(String),
    #[error("Minecraft {0} has no dedicated server download")]
    NoServerDownload(String),
    #[error("{loader} is not available for Minecraft {mc_version}")]
    LoaderUnavailable { loader: LoaderKind, mc_version: String },
    #[error("{loader} version {version} does not exist for Minecraft {mc_version}")]
    UnknownLoaderVersion {
        loader: LoaderKind,
        mc_version: String,
        version: String,
    },
    #[error("{loader} build {version} for Minecraft {mc_version} failed upstream and can't be installed")]
    BrokenBuild {
        loader: LoaderKind,
        mc_version: String,
        version: String,
    },
}

/// A Minecraft version in the picker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GameVersion {
    pub id: String,
    pub kind: VersionKind,
    pub released: DateTime<Utc>,
}

/// Whether a loader can run a given Minecraft version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LoaderAvailability {
    pub loader: LoaderKind,
    pub name: &'static str,
    /// What the loader adds to the game (shown as a hint in the picker).
    pub content: ContentKind,
    pub available: bool,
}

impl LoaderAvailability {
    pub fn new(loader: LoaderKind, available: bool) -> Self {
        Self {
            loader,
            name: loader.display_name(),
            content: loader.content(),
            available,
        }
    }
}

/// One selectable loader version (Paper build, Fabric loader version, Forge version, …).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LoaderVersion {
    pub id: String,
    pub released: Option<DateTime<Utc>>,
    pub stable: bool,
    /// The one to pick by default (★ in the picker).
    pub recommended: bool,
}

/// A file to download and the checksum it must match.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Artifact {
    pub url: String,
    pub hash: ExpectedHash,
}

/// Marks the recommended entry: `preferred` if present, else the newest stable one, else the
/// newest. `versions` must be sorted newest first.
pub(super) fn mark_recommended(
    mut versions: Vec<LoaderVersion>,
    preferred: Option<&str>,
) -> Vec<LoaderVersion> {
    let index = preferred
        .and_then(|id| versions.iter().position(|v| v.id == id))
        .or_else(|| versions.iter().position(|v| v.stable))
        .or(if versions.is_empty() { None } else { Some(0) });
    if let Some(index) = index {
        versions[index].recommended = true;
    }
    versions
}

#[cfg(test)]
mod tests {
    use super::*;

    fn version(id: &str, stable: bool) -> LoaderVersion {
        LoaderVersion {
            id: id.into(),
            released: None,
            stable,
            recommended: false,
        }
    }

    fn recommended(versions: &[LoaderVersion]) -> Vec<&str> {
        versions
            .iter()
            .filter(|v| v.recommended)
            .map(|v| v.id.as_str())
            .collect()
    }

    #[test]
    fn recommends_preferred_then_newest_stable_then_newest() {
        let list = || vec![version("3-beta", false), version("2", true), version("1", true)];
        assert_eq!(recommended(&mark_recommended(list(), Some("1"))), ["1"]);
        assert_eq!(recommended(&mark_recommended(list(), Some("missing"))), ["2"]);
        assert_eq!(recommended(&mark_recommended(list(), None)), ["2"]);
        let unstable = vec![version("2-beta", false), version("1-beta", false)];
        assert_eq!(recommended(&mark_recommended(unstable, None)), ["2-beta"]);
        assert_eq!(mark_recommended(vec![], None), Vec::<LoaderVersion>::new());
    }
}
