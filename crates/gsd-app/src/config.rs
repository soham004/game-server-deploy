//! Runtime configuration.
//!
//! Precedence, lowest to highest: built-in defaults → `<data dir>/config.toml` → environment
//! variables (`<PREFIX>_*`, prefix from `gsd_brand`) → command-line flags.

use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    path::{Path, PathBuf},
    str::FromStr,
};

use chrono::TimeDelta;
use gsd_brand::env_var;
use gsd_core::ports::PortRange;
use ipnet::IpNet;
use serde::Deserialize;

/// Name of the optional config file inside the data directory.
pub const CONFIG_FILE_NAME: &str = "config.toml";

const DEFAULT_GAME_PORTS: (u16, u16) = (25565, 25584);
const DEFAULT_INTERNAL_PORTS: (u16, u16) = (35565, 35664);
const DEFAULT_SESSION_TTL_DAYS: u32 = 30;
const MAX_SESSION_TTL_DAYS: u32 = 365;
const DEFAULT_TRASH_RETENTION_DAYS: u32 = 7;

/// Command-line flags that override configuration.
#[derive(Debug, Default, Clone, clap::Args)]
pub struct ConfigOverrides {
    /// Directory for the database, instances, backups and caches.
    #[arg(long, global = true, value_name = "DIR")]
    pub data_dir: Option<PathBuf>,
    /// Address to serve the web UI on, e.g. 127.0.0.1:8080.
    #[arg(long, global = true, value_name = "ADDR")]
    pub bind: Option<SocketAddr>,
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("could not read {path}")]
    ReadFile {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("invalid config file {path}")]
    ParseFile {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },
    #[error("invalid value for environment variable {name}: {reason}")]
    InvalidEnv { name: String, reason: String },
    #[error("invalid configuration: {0}")]
    Invalid(String),
}

/// The effective configuration of a running app.
#[derive(Debug, Clone)]
pub struct Config {
    pub data_dir: PathBuf,
    pub bind: SocketAddr,
    /// Reverse proxies whose `X-Forwarded-*` headers are trusted (e.g. the nginx container).
    pub trusted_proxies: Vec<IpNet>,
    /// Public ports handed out to game servers (D-022).
    pub game_ports: PortRange,
    /// Loopback-only ports for RCON and the management protocol; never published.
    pub internal_ports: PortRange,
    pub session_ttl: TimeDelta,
    /// Deleted servers, files and backups are removed from the trash after this many days;
    /// 0 keeps them until removed by hand.
    pub trash_retention_days: u32,
    /// Where the `CurseForge` API is, when not the real one: the browser tests' stand-in, since they
    /// have no key for the real API. Environment only (`GSD_CURSEFORGE_API`).
    pub curseforge_api: Option<String>,
}

/// Settings that may appear in `config.toml` (all optional).
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct FileConfig {
    bind: Option<SocketAddr>,
    trusted_proxies: Option<Vec<IpNet>>,
    game_ports: Option<PortRange>,
    internal_ports: Option<PortRange>,
    session_ttl_days: Option<u32>,
    trash_retention_days: Option<u32>,
}

impl Config {
    /// All defaults, rooted at `data_dir`.
    pub fn with_data_dir(data_dir: PathBuf) -> Self {
        Self {
            data_dir,
            bind: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), gsd_brand::DEFAULT_PORT),
            trusted_proxies: Vec::new(),
            game_ports: port_range(DEFAULT_GAME_PORTS),
            internal_ports: port_range(DEFAULT_INTERNAL_PORTS),
            session_ttl: TimeDelta::days(i64::from(DEFAULT_SESSION_TTL_DAYS)),
            trash_retention_days: DEFAULT_TRASH_RETENTION_DAYS,
            curseforge_api: None,
        }
    }

    /// Builds the configuration from all sources. `env` looks up environment variables (injected
    /// so tests don't have to mutate the real process environment).
    pub fn load(
        overrides: &ConfigOverrides,
        env: impl Fn(&str) -> Option<String>,
    ) -> Result<Self, ConfigError> {
        let data_dir = overrides
            .data_dir
            .clone()
            .or_else(|| env(&env_var("DATA_DIR")).map(PathBuf::from))
            .unwrap_or_else(default_data_dir);

        let file = FileConfig::read(&data_dir.join(CONFIG_FILE_NAME))?;
        let mut config = Self::with_data_dir(data_dir);
        config.apply_file(file)?;
        config.apply_env(&env)?;
        config.curseforge_api = env(&env_var("CURSEFORGE_API"));
        config.apply_overrides(overrides);
        config.validate()?;
        Ok(config)
    }

    /// Location of the SQLite database.
    pub fn database_path(&self) -> PathBuf {
        self.data_dir.join(format!("{}.sqlite", gsd_brand::SLUG))
    }

    /// One folder per instance (see `gsd_core::instances::layout`).
    pub fn instances_dir(&self) -> PathBuf {
        self.data_dir.join("instances")
    }

    /// Backup archives, one folder per instance id (root only).
    pub fn backups_dir(&self) -> PathBuf {
        self.data_dir.join("backups")
    }

    /// Deleted instances, kept until purged.
    pub fn trash_dir(&self) -> PathBuf {
        self.data_dir.join("trash")
    }

    /// Managed Java runtimes (readable by every sandbox user).
    pub fn runtimes_dir(&self) -> PathBuf {
        self.data_dir.join("runtimes").join("java")
    }

    /// Mods and plugins downloaded for any server, by hash.
    pub fn content_cache_dir(&self) -> PathBuf {
        self.data_dir.join("cache").join("content")
    }

    /// Shared, content-addressed download cache (server jars).
    pub fn download_cache_dir(&self) -> PathBuf {
        self.data_dir.join("cache").join("downloads")
    }

    /// Server folders waiting to be imported (an admin copies them here).
    pub fn imports_dir(&self) -> PathBuf {
        self.data_dir.join("imports")
    }

    /// Scratch space for sandbox self-tests (on the same filesystem as the instances).
    pub fn sandbox_check_dir(&self) -> PathBuf {
        self.data_dir.join("sandbox-check")
    }

    /// Cached catalog responses (offline fallback).
    pub fn catalog_cache_dir(&self) -> PathBuf {
        self.data_dir.join("cache").join("meta")
    }

    fn apply_file(&mut self, file: FileConfig) -> Result<(), ConfigError> {
        if let Some(bind) = file.bind {
            self.bind = bind;
        }
        if let Some(proxies) = file.trusted_proxies {
            self.trusted_proxies = proxies;
        }
        if let Some(range) = file.game_ports {
            self.game_ports = range;
        }
        if let Some(range) = file.internal_ports {
            self.internal_ports = range;
        }
        if let Some(days) = file.session_ttl_days {
            self.session_ttl = session_ttl_from_days(days)?;
        }
        if let Some(days) = file.trash_retention_days {
            self.trash_retention_days = days;
        }
        Ok(())
    }

    fn apply_env(&mut self, env: &impl Fn(&str) -> Option<String>) -> Result<(), ConfigError> {
        if let Some(bind) = parse_env(env, "BIND")? {
            self.bind = bind;
        }
        if let Some(raw) = read_env(env, "TRUSTED_PROXIES") {
            self.trusted_proxies =
                parse_list(&raw).map_err(|reason| invalid_env("TRUSTED_PROXIES", reason))?;
        }
        if let Some(range) = parse_env(env, "GAME_PORTS")? {
            self.game_ports = range;
        }
        if let Some(range) = parse_env(env, "INTERNAL_PORTS")? {
            self.internal_ports = range;
        }
        if let Some(days) = parse_env::<u32>(env, "SESSION_TTL_DAYS")? {
            self.session_ttl = session_ttl_from_days(days)?;
        }
        if let Some(days) = parse_env::<u32>(env, "TRASH_RETENTION_DAYS")? {
            self.trash_retention_days = days;
        }
        Ok(())
    }

    fn apply_overrides(&mut self, overrides: &ConfigOverrides) {
        if let Some(bind) = overrides.bind {
            self.bind = bind;
        }
    }

    fn validate(&self) -> Result<(), ConfigError> {
        if self.game_ports.overlaps(self.internal_ports) {
            return Err(ConfigError::Invalid(format!(
                "game ports {} overlap internal ports {}",
                self.game_ports, self.internal_ports
            )));
        }
        let web_port = self.bind.port();
        if self.game_ports.contains(web_port) || self.internal_ports.contains(web_port) {
            return Err(ConfigError::Invalid(format!(
                "web port {web_port} lies inside a server port range"
            )));
        }
        Ok(())
    }
}

impl FileConfig {
    fn read(path: &Path) -> Result<Self, ConfigError> {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(source) => {
                return Err(ConfigError::ReadFile {
                    path: path.to_owned(),
                    source,
                });
            }
        };
        toml::from_str(&text).map_err(|source| ConfigError::ParseFile {
            path: path.to_owned(),
            source,
        })
    }
}

fn default_data_dir() -> PathBuf {
    directories::ProjectDirs::from("", "", gsd_brand::SLUG)
        .map_or_else(|| PathBuf::from("data"), |dirs| dirs.data_dir().to_path_buf())
}

fn port_range((first, last): (u16, u16)) -> PortRange {
    // WHY: the defaults above are valid by construction (first <= last, both non-zero).
    PortRange::new(first, last).unwrap_or_else(|_| unreachable!("default port range is valid"))
}

fn session_ttl_from_days(days: u32) -> Result<TimeDelta, ConfigError> {
    if !(1..=MAX_SESSION_TTL_DAYS).contains(&days) {
        return Err(ConfigError::Invalid(format!(
            "session_ttl_days must be 1–{MAX_SESSION_TTL_DAYS}, got {days}"
        )));
    }
    Ok(TimeDelta::days(i64::from(days)))
}

fn read_env(env: &impl Fn(&str) -> Option<String>, name: &str) -> Option<String> {
    env(&env_var(name))
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

fn parse_env<T>(env: &impl Fn(&str) -> Option<String>, name: &str) -> Result<Option<T>, ConfigError>
where
    T: FromStr,
    T::Err: std::fmt::Display,
{
    read_env(env, name)
        .map(|raw| {
            raw.parse()
                .map_err(|error: T::Err| invalid_env(name, error.to_string()))
        })
        .transpose()
}

fn parse_list<T>(raw: &str) -> Result<Vec<T>, String>
where
    T: FromStr,
    T::Err: std::fmt::Display,
{
    raw.split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(|item| item.parse().map_err(|e: T::Err| format!("{item:?}: {e}")))
        .collect()
}

fn invalid_env(name: &str, reason: impl Into<String>) -> ConfigError {
    ConfigError::InvalidEnv {
        name: env_var(name),
        reason: reason.into(),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn env_from(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(name, value)| (env_var(name), (*value).to_owned()))
            .collect();
        move |name| map.get(name).cloned()
    }

    #[test]
    fn defaults_bind_to_loopback_on_the_brand_port() {
        let dir = tempfile::tempdir().unwrap();
        let overrides = ConfigOverrides {
            data_dir: Some(dir.path().to_owned()),
            bind: None,
        };
        let config = Config::load(&overrides, env_from(&[])).unwrap();
        assert!(config.bind.ip().is_loopback());
        assert_eq!(config.bind.port(), gsd_brand::DEFAULT_PORT);
        assert_eq!(config.game_ports.count(), 20);
        assert_eq!(config.trash_retention_days, 7);
    }

    #[test]
    fn precedence_is_file_then_env_then_cli() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join(CONFIG_FILE_NAME),
            "bind = \"127.0.0.1:9001\"\ngame_ports = \"26000-26009\"\nsession_ttl_days = 7\ntrash_retention_days = 30\n",
        )
        .unwrap();
        let env = env_from(&[
            ("BIND", "127.0.0.1:9002"),
            ("TRUSTED_PROXIES", "172.16.0.0/12, 10.0.0.1/32"),
            ("TRASH_RETENTION_DAYS", "0"),
        ]);

        let from_env = Config::load(
            &ConfigOverrides {
                data_dir: Some(dir.path().to_owned()),
                bind: None,
            },
            &env,
        )
        .unwrap();
        assert_eq!(from_env.bind.port(), 9002);
        assert_eq!(from_env.game_ports.first(), 26000);
        assert_eq!(from_env.session_ttl, TimeDelta::days(7));
        assert_eq!(from_env.trusted_proxies.len(), 2);
        assert_eq!(
            from_env.trash_retention_days, 0,
            "the environment wins over the file"
        );

        let cli = ConfigOverrides {
            data_dir: Some(dir.path().to_owned()),
            bind: Some("127.0.0.1:9003".parse().unwrap()),
        };
        assert_eq!(Config::load(&cli, &env).unwrap().bind.port(), 9003);
    }

    #[test]
    fn rejects_overlapping_ranges_and_bad_env() {
        let dir = tempfile::tempdir().unwrap();
        let overrides = ConfigOverrides {
            data_dir: Some(dir.path().to_owned()),
            bind: None,
        };
        assert!(Config::load(&overrides, env_from(&[("INTERNAL_PORTS", "25570-25600")])).is_err());
        assert!(Config::load(&overrides, env_from(&[("GAME_PORTS", "nope")])).is_err());
    }

    #[test]
    fn unknown_file_keys_are_errors() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(CONFIG_FILE_NAME), "bnid = \"typo\"\n").unwrap();
        let overrides = ConfigOverrides {
            data_dir: Some(dir.path().to_owned()),
            bind: None,
        };
        assert!(matches!(
            Config::load(&overrides, env_from(&[])),
            Err(ConfigError::ParseFile { .. })
        ));
    }
}
