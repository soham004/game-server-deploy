//! What content fits a server: mods for modded servers, plugins for Paper and Purpur, which
//! loader tags count, and the folder the files go in.

use gsd_minecraft::{LoaderKind, settings::MinecraftServer};
use serde::Serialize;

/// Mods or plugins.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ContentKind {
    Mods,
    Plugins,
}

impl ContentKind {
    /// The server folder the files go in.
    pub fn folder(self) -> &'static str {
        match self {
            Self::Mods => "mods",
            Self::Plugins => "plugins",
        }
    }
}

/// What a server can run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Target {
    pub kind: ContentKind,
    pub mc_version: String,
    /// Loader tags a file may carry to run here, most specific first (Quilt runs Fabric mods;
    /// Purpur runs Paper, Spigot and Bukkit plugins).
    pub loaders: Vec<&'static str>,
}

impl Target {
    /// What `server` can run; `None` for vanilla, which runs neither mods nor plugins.
    pub fn for_server(server: &MinecraftServer) -> Option<Self> {
        let (kind, loaders): (ContentKind, &[&'static str]) = match server.loader {
            LoaderKind::Vanilla => return None,
            LoaderKind::Fabric => (ContentKind::Mods, &["fabric"]),
            LoaderKind::Quilt => (ContentKind::Mods, &["quilt", "fabric"]),
            LoaderKind::Forge => (ContentKind::Mods, &["forge"]),
            LoaderKind::NeoForge => (ContentKind::Mods, &["neoforge"]),
            LoaderKind::Paper => (ContentKind::Plugins, &["paper", "spigot", "bukkit"]),
            LoaderKind::Purpur => (ContentKind::Plugins, &["purpur", "paper", "spigot", "bukkit"]),
        };
        Some(Self {
            kind,
            mc_version: server.mc_version.clone(),
            loaders: loaders.to_vec(),
        })
    }

    /// Whether a version made for `loaders` and `game_versions` runs here.
    pub fn accepts(&self, loaders: &[String], game_versions: &[String]) -> bool {
        game_versions.contains(&self.mc_version)
            && loaders
                .iter()
                .any(|loader| self.loaders.contains(&loader.as_str()))
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use chrono::Utc;

    use super::*;

    fn server(loader: LoaderKind) -> MinecraftServer {
        MinecraftServer {
            mc_version: "26.3".into(),
            loader,
            loader_version: None,
            eula_accepted_at: Utc::now(),
        }
    }

    #[test]
    fn each_loader_maps_to_its_kind_folder_and_tags() {
        assert_eq!(Target::for_server(&server(LoaderKind::Vanilla)), None);
        let quilt = Target::for_server(&server(LoaderKind::Quilt)).unwrap();
        assert_eq!((quilt.kind, quilt.kind.folder()), (ContentKind::Mods, "mods"));
        assert_eq!(quilt.loaders, ["quilt", "fabric"], "Quilt runs Fabric mods");
        let purpur = Target::for_server(&server(LoaderKind::Purpur)).unwrap();
        assert_eq!((purpur.kind.folder(), purpur.loaders.len()), ("plugins", 4));
    }

    #[test]
    fn accepts_only_matching_loader_and_minecraft_version() {
        let fabric = Target::for_server(&server(LoaderKind::Fabric)).unwrap();
        let strings = |items: &[&str]| items.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        assert!(fabric.accepts(&strings(&["fabric", "quilt"]), &strings(&["26.2", "26.3"])));
        assert!(!fabric.accepts(&strings(&["neoforge"]), &strings(&["26.3"])));
        assert!(!fabric.accepts(&strings(&["fabric"]), &strings(&["26.2"])));
    }
}
