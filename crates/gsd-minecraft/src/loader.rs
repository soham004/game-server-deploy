//! Server software ("loaders") an instance can run.

use std::{fmt, str::FromStr};

use serde::{Deserialize, Serialize};

/// Server software. Forge, `NeoForge`, Quilt and Purpur arrive in milestone M2.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LoaderKind {
    /// Mojang's official server.
    Vanilla,
    /// High-performance fork with plugin support (Bukkit/Spigot/Paper plugins).
    Paper,
    /// Lightweight mod loader.
    Fabric,
}

impl LoaderKind {
    pub const ALL: [Self; 3] = [Self::Vanilla, Self::Paper, Self::Fabric];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Vanilla => "vanilla",
            Self::Paper => "paper",
            Self::Fabric => "fabric",
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            Self::Vanilla => "Vanilla",
            Self::Paper => "Paper",
            Self::Fabric => "Fabric",
        }
    }

    /// Whether this loader has its own version list (vanilla is just the Minecraft version).
    pub fn has_versions(self) -> bool {
        !matches!(self, Self::Vanilla)
    }
}

impl fmt::Display for LoaderKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
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
    fn parses_names() {
        assert_eq!("Paper".parse::<LoaderKind>().unwrap(), LoaderKind::Paper);
        assert!("forge".parse::<LoaderKind>().is_err());
        assert!(!LoaderKind::Vanilla.has_versions());
    }
}
