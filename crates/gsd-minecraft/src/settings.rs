//! The Minecraft-specific `[server]` table of `instance.toml`.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::loader::LoaderKind;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MinecraftServer {
    pub mc_version: String,
    pub loader: LoaderKind,
    /// Paper build number or Fabric loader version; `None` for vanilla.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loader_version: Option<String>,
    /// When the user accepted the Minecraft EULA for this server (required to start it).
    pub eula_accepted_at: DateTime<Utc>,
}

#[derive(Debug, thiserror::Error)]
pub enum SettingsError {
    #[error("the [server] table is not a valid Minecraft server configuration")]
    Invalid(#[source] toml::de::Error),
    #[error("could not encode the Minecraft server configuration")]
    Encode(#[source] toml::ser::Error),
}

impl MinecraftServer {
    pub fn from_table(table: &toml::Table) -> Result<Self, SettingsError> {
        table.clone().try_into().map_err(SettingsError::Invalid)
    }

    pub fn to_table(&self) -> Result<toml::Table, SettingsError> {
        toml::Table::try_from(self).map_err(SettingsError::Encode)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_a_table() {
        let server = MinecraftServer {
            mc_version: "26.3".into(),
            loader: LoaderKind::Fabric,
            loader_version: Some("0.20.1".into()),
            eula_accepted_at: Utc::now(),
        };
        let table = server.to_table().unwrap();
        assert_eq!(table["loader"].as_str(), Some("fabric"));
        assert_eq!(MinecraftServer::from_table(&table).unwrap(), server);
        assert!(MinecraftServer::from_table(&toml::Table::new()).is_err());
    }
}
