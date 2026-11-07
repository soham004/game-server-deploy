//! Server software ("loaders") an instance can run.

use std::{fmt, str::FromStr};

use serde::{Deserialize, Serialize};

/// Server software, in the order the version picker shows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LoaderKind {
    /// Mojang's official server.
    Vanilla,
    /// High-performance fork with plugin support (Bukkit/Spigot/Paper plugins).
    Paper,
    /// Paper fork with extra gameplay options; runs Paper plugins.
    Purpur,
    /// Lightweight mod loader.
    Fabric,
    /// Fabric fork; runs most Fabric mods.
    Quilt,
    /// The original mod loader.
    Forge,
    /// Forge fork, the main loader for modern modpacks.
    NeoForge,
}

/// What a loader lets the server add.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ContentKind {
    None,
    Plugins,
    Mods,
}

impl LoaderKind {
    pub const ALL: [Self; 7] = [
        Self::Vanilla,
        Self::Paper,
        Self::Purpur,
        Self::Fabric,
        Self::Quilt,
        Self::Forge,
        Self::NeoForge,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Vanilla => "vanilla",
            Self::Paper => "paper",
            Self::Purpur => "purpur",
            Self::Fabric => "fabric",
            Self::Quilt => "quilt",
            Self::Forge => "forge",
            Self::NeoForge => "neoforge",
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            Self::Vanilla => "Vanilla",
            Self::Paper => "Paper",
            Self::Purpur => "Purpur",
            Self::Fabric => "Fabric",
            Self::Quilt => "Quilt",
            Self::Forge => "Forge",
            Self::NeoForge => "NeoForge",
        }
    }

    pub fn content(self) -> ContentKind {
        match self {
            Self::Vanilla => ContentKind::None,
            Self::Paper | Self::Purpur => ContentKind::Plugins,
            Self::Fabric | Self::Quilt | Self::Forge | Self::NeoForge => ContentKind::Mods,
        }
    }

    /// Whether this loader has its own version list (vanilla is just the Minecraft version).
    pub fn has_versions(self) -> bool {
        !matches!(self, Self::Vanilla)
    }

    /// Whether the server is set up by running the loader's installer (rather than by
    /// downloading a ready-to-run jar).
    pub fn uses_installer(self) -> bool {
        matches!(self, Self::Quilt | Self::Forge | Self::NeoForge)
    }
}

impl fmt::Display for LoaderKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.display_name())
    }
}

#[derive(Debug, thiserror::Error)]
#[error("unknown server software {0:?}")]
pub struct UnknownLoader(String);

impl FromStr for LoaderKind {
    type Err = UnknownLoader;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|kind| kind.as_str().eq_ignore_ascii_case(raw))
            .ok_or_else(|| UnknownLoader(raw.into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_round_trip_through_text_and_serde() {
        for kind in LoaderKind::ALL {
            assert_eq!(kind.as_str().parse::<LoaderKind>().unwrap(), kind);
            assert_eq!(kind.as_str().to_uppercase().parse::<LoaderKind>().unwrap(), kind);
            let json = serde_json::to_string(&kind).unwrap();
            assert_eq!(json, format!("\"{}\"", kind.as_str()));
            assert_eq!(serde_json::from_str::<LoaderKind>(&json).unwrap(), kind);
        }
        assert!("spigot".parse::<LoaderKind>().is_err());
    }

    #[test]
    fn describes_each_loader() {
        assert!(!LoaderKind::Vanilla.has_versions());
        assert!(LoaderKind::Purpur.has_versions());
        assert_eq!(LoaderKind::NeoForge.to_string(), "NeoForge");
        let installers: Vec<_> = LoaderKind::ALL
            .into_iter()
            .filter(|l| l.uses_installer())
            .collect();
        assert_eq!(
            installers,
            [LoaderKind::Quilt, LoaderKind::Forge, LoaderKind::NeoForge]
        );
        assert_eq!(LoaderKind::Purpur.content(), ContentKind::Plugins);
        assert_eq!(LoaderKind::Quilt.content(), ContentKind::Mods);
    }
}
