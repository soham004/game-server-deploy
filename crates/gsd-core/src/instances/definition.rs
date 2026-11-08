//! `instance.toml`: the portable definition of one server instance.
//!
//! The `[server]` table is owned by the game adapter (Minecraft version, loader, …) and is
//! opaque to the core. Everything else here is game-agnostic.

use std::path::Path;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::ids::InstanceId;

/// Current `instance.toml` schema version.
pub const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstanceDefinition {
    pub schema: u32,
    pub id: InstanceId,
    pub name: String,
    /// Which adapter owns `[server]`, e.g. `"minecraft"`.
    pub game: String,
    pub created_at: DateTime<Utc>,
    /// Set once the game files are installed; an instance without it can't be started.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub installed_at: Option<DateTime<Utc>>,
    /// Game-specific settings, interpreted by the adapter.
    #[serde(default)]
    pub server: toml::Table,
    pub launch: LaunchSpec,
    pub java: JavaSettings,
    #[serde(default)]
    pub behavior: Behavior,
    #[serde(default)]
    pub backups: BackupPolicy,
    pub ports: Ports,
    pub sandbox: SandboxIds,
}

/// How the JVM starts the server. Paths are relative to the server directory; `args` go after
/// the jar or args files (e.g. `nogui`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LaunchSpec {
    /// `java … -jar <jar> <args>`.
    Jar {
        jar: String,
        #[serde(default)]
        args: Vec<String>,
    },
    /// `java … @<jvm_args_file> @<args_file> <args>` (modern Forge / `NeoForge`).
    ArgsFiles {
        jvm_args_file: String,
        args_file: String,
        #[serde(default)]
        args: Vec<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JavaSettings {
    /// The Java major version this server needs (from the game's metadata).
    pub major: u32,
    /// Which installation to use.
    #[serde(default)]
    pub runtime: RuntimeChoice,
    pub min_memory_mb: u32,
    pub max_memory_mb: u32,
    #[serde(default)]
    pub preset: JvmPreset,
    #[serde(default)]
    pub extra_jvm_args: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RuntimeChoice {
    /// A runtime with exactly the required major (managed preferred).
    #[default]
    Auto,
    /// A specific detected runtime, by id.
    Runtime { id: String },
}

/// Garbage-collector tuning presets.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JvmPreset {
    /// Memory flags only.
    None,
    /// Aikar's widely used G1 flags, good up to ~12 GB.
    #[default]
    Aikar,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Behavior {
    pub auto_start: bool,
    pub auto_restart: bool,
    /// Give up auto-restarting after this many crashes within `crash_window_secs`.
    pub crash_limit: u32,
    pub crash_window_secs: u64,
    /// How long a graceful stop may take before the process is killed.
    pub stop_timeout_secs: u64,
}

impl Default for Behavior {
    fn default() -> Self {
        Self {
            auto_start: false,
            auto_restart: true,
            crash_limit: 3,
            crash_window_secs: 600,
            stop_timeout_secs: 60,
        }
    }
}

/// How many backups to keep. Pinned backups never count and are never deleted automatically.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct BackupPolicy {
    /// Manual and scheduled backups: the newest this many are kept.
    pub keep: u32,
    /// Safety backups taken before a restore or a version change: the newest this many are kept.
    pub keep_safety: u32,
}

impl Default for BackupPolicy {
    fn default() -> Self {
        Self {
            keep: 10,
            keep_safety: 3,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Ports {
    /// The public game port.
    pub game: u16,
}

/// The dedicated user the server runs as (Linux sandbox).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SandboxIds {
    pub uid: u32,
    pub gid: u32,
}

#[derive(Debug, thiserror::Error)]
pub enum DefinitionError {
    #[error("could not read {path}")]
    Read {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("invalid {path}")]
    Parse {
        path: String,
        #[source]
        source: toml::de::Error,
    },
    #[error("{path} uses schema {found}, but this version only understands up to {SCHEMA_VERSION}")]
    UnsupportedSchema { path: String, found: u32 },
    #[error("could not write {path}")]
    Write {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("could not serialise the instance definition")]
    Serialize(#[from] toml::ser::Error),
}

impl InstanceDefinition {
    pub fn load(path: &Path) -> Result<Self, DefinitionError> {
        let display = path.display().to_string();
        let text = std::fs::read_to_string(path).map_err(|source| DefinitionError::Read {
            path: display.clone(),
            source,
        })?;
        let definition: Self = toml::from_str(&text).map_err(|source| DefinitionError::Parse {
            path: display.clone(),
            source,
        })?;
        if definition.schema > SCHEMA_VERSION {
            return Err(DefinitionError::UnsupportedSchema {
                path: display,
                found: definition.schema,
            });
        }
        Ok(definition)
    }

    /// Writes atomically: a temporary file in the same directory, then a rename.
    pub fn save(&self, path: &Path) -> Result<(), DefinitionError> {
        let display = path.display().to_string();
        let write_error = |source| DefinitionError::Write {
            path: display.clone(),
            source,
        };
        let text = toml::to_string_pretty(self)?;
        let dir = path.parent().unwrap_or(Path::new("."));
        let mut temp = tempfile::NamedTempFile::new_in(dir).map_err(write_error)?;
        std::io::Write::write_all(&mut temp, text.as_bytes()).map_err(write_error)?;
        temp.as_file().sync_all().map_err(write_error)?;
        temp.persist(path).map_err(|error| write_error(error.error))?;
        Ok(())
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn sample() -> InstanceDefinition {
        let mut server = toml::Table::new();
        server.insert("mc_version".into(), "26.3".into());
        InstanceDefinition {
            schema: SCHEMA_VERSION,
            id: InstanceId::generate(),
            name: "Survival".into(),
            game: "minecraft".into(),
            created_at: Utc::now(),
            installed_at: Some(Utc::now()),
            server,
            launch: LaunchSpec::Jar {
                jar: "server.jar".into(),
                args: vec!["nogui".into()],
            },
            java: JavaSettings {
                major: 25,
                runtime: RuntimeChoice::Auto,
                min_memory_mb: 1024,
                max_memory_mb: 4096,
                preset: JvmPreset::Aikar,
                extra_jvm_args: vec![],
            },
            behavior: Behavior::default(),
            backups: BackupPolicy::default(),
            ports: Ports { game: 25565 },
            sandbox: SandboxIds {
                uid: 20000,
                gid: 20000,
            },
        }
    }

    #[test]
    fn round_trips_through_toml() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("instance.toml");
        let definition = sample();
        definition.save(&path).unwrap();

        let text = std::fs::read_to_string(&path).unwrap();
        assert!(
            text.contains("[launch]") && text.contains("kind = \"jar\""),
            "{text}"
        );
        assert_eq!(InstanceDefinition::load(&path).unwrap(), definition);
    }

    #[test]
    fn rejects_future_schemas_and_unknown_keys() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("instance.toml");
        let mut definition = sample();
        definition.schema = SCHEMA_VERSION + 1;
        definition.save(&path).unwrap();
        assert!(matches!(
            InstanceDefinition::load(&path),
            Err(DefinitionError::UnsupportedSchema { .. })
        ));

        let text = toml::to_string(&sample()).unwrap() + "\nsurprise = 1\n";
        std::fs::write(&path, text).unwrap();
        assert!(matches!(
            InstanceDefinition::load(&path),
            Err(DefinitionError::Parse { .. })
        ));
    }

    #[test]
    fn behaviour_defaults_are_safe() {
        let behavior = Behavior::default();
        assert!(behavior.auto_restart);
        assert!(!behavior.auto_start);
        assert!(behavior.stop_timeout_secs >= 30);
    }
}
