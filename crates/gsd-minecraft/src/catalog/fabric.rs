//! Fabric meta API.
//!
//! The server is a single "launcher" jar built on demand by Fabric meta from (Minecraft version,
//! loader version, installer version); on first start it downloads the vanilla server itself.

use serde::Deserialize;

use super::LoaderVersion;

#[derive(Debug, Deserialize)]
pub(super) struct GameEntry {
    pub version: String,
}

#[derive(Debug, Deserialize)]
pub(super) struct LoaderEntry {
    pub loader: FabricLoaderVersion,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct FabricLoaderVersion {
    pub version: String,
    pub stable: bool,
}

#[derive(Debug, Deserialize)]
pub(super) struct InstallerEntry {
    version: String,
    stable: bool,
}

/// Newest stable installer, else the newest one.
pub(super) fn pick_installer(installers: &[InstallerEntry]) -> Option<String> {
    installers
        .iter()
        .find(|i| i.stable)
        .or_else(|| installers.first())
        .map(|i| i.version.clone())
}

/// The newest stable loader, else the newest one.
pub fn pick_loader(loaders: &[FabricLoaderVersion]) -> Option<&FabricLoaderVersion> {
    loaders
        .iter()
        .find(|loader| loader.stable)
        .or_else(|| loaders.first())
}

pub(super) fn to_loader_versions(loaders: &[FabricLoaderVersion]) -> Vec<LoaderVersion> {
    let recommended = pick_loader(loaders).map(|loader| loader.version.clone());
    loaders
        .iter()
        .map(|loader| LoaderVersion {
            id: loader.version.clone(),
            released: None,
            stable: loader.stable,
            recommended: Some(&loader.version) == recommended.as_ref(),
        })
        .collect()
}

/// URL of the server launcher jar.
pub fn server_jar_url(base: &str, mc_version: &str, loader: &str, installer: &str) -> String {
    format!("{base}/v2/versions/loader/{mc_version}/{loader}/{installer}/server/jar")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_stable_versions() {
        let loaders = vec![
            FabricLoaderVersion {
                version: "0.21.0-beta".into(),
                stable: false,
            },
            FabricLoaderVersion {
                version: "0.20.1".into(),
                stable: true,
            },
        ];
        assert_eq!(pick_loader(&loaders).unwrap().version, "0.20.1");
        let versions = to_loader_versions(&loaders);
        assert!(!versions[0].recommended && versions[1].recommended);
    }

    #[test]
    fn builds_the_launcher_url() {
        assert_eq!(
            server_jar_url("https://meta.fabricmc.net", "26.3", "0.20.1", "1.1.2"),
            "https://meta.fabricmc.net/v2/versions/loader/26.3/0.20.1/1.1.2/server/jar"
        );
    }
}
