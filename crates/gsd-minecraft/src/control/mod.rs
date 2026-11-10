//! The channel the app controls a running server through, besides its console.
//!
//! Each server has one, on an internal port of its own that is never published:
//! - **The management protocol (MCSMP)** on Minecraft 1.21.9 and later: structured, and it can be told
//!   to listen on loopback only.
//! - **RCON** on older versions, and only if the user turns it on (`enable-rcon`). It has no setting for
//!   its address: it listens wherever the game does, seen on a real server as every interface, so this app
//!   won't open that on its own. Those servers work through their console, as all of them always could.
//!   Once it is on, it gets a port the sandbox allows and a long random password if it has none. The app
//!   itself only connects over loopback, so the password doesn't cross a network.
//!
//! The settings are written before every start, through the file manager, because the file is the
//! server's own and it may have turned it into a link. The app reads the secret back from it when it
//! connects, so there is one copy of it, and it never goes to a browser.

pub mod mcsmp;
pub mod rcon;

use std::time::Duration;

use gsd_core::{
    files::{FilesError, SafeRelPath, ServerFiles},
    instances::Instance,
    random,
};
use serde::Serialize;

use crate::{properties::PropertiesDoc, settings::MinecraftServer};
use mcsmp::{Mcsmp, McsmpError};
use rcon::{Rcon, RconError};

const PROPERTIES_FILE: &str = "server.properties";
/// How long a connection attempt may take. It is on the same machine, so it either works quickly or won't.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// Which protocol a server is controlled through.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Channel {
    Management,
    Rcon,
}

#[derive(Debug, thiserror::Error)]
pub enum ControlError {
    #[error("this server has no control channel yet: it gets one when it starts")]
    NoPort,
    #[error("its server.properties has no secret for the control channel")]
    NoSecret,
    #[error("that takes the management protocol, and this server has RCON")]
    WrongChannel,
    #[error("could not read the server's settings")]
    Files(#[from] FilesError),
    #[error(transparent)]
    Mcsmp(#[from] McsmpError),
    #[error(transparent)]
    Rcon(#[from] RconError),
}

/// The management protocol came with Minecraft 1.21.9 and snapshot 25w35a. Anything this can't read
/// counts as older: RCON works on every version.
pub fn channel_for(mc_version: &str) -> Channel {
    if has_management(mc_version) {
        Channel::Management
    } else {
        Channel::Rcon
    }
}

fn has_management(version: &str) -> bool {
    // Snapshots: 25w35a. Their year and week are enough.
    if let Some((year, rest)) = version.split_once('w') {
        let week = rest.trim_end_matches(|c: char| c.is_ascii_alphabetic());
        if let (Ok(year), Ok(week)) = (year.parse::<u32>(), week.parse::<u32>()) {
            return (year, week) >= (25, 35);
        }
    }
    // Pre-releases and release candidates count as what they lead to: 1.21.9-rc1.
    let release = version.split('-').next().unwrap_or(version);
    let mut parts = release.split('.').map(|part| part.parse::<u32>().ok());
    match (parts.next().flatten(), parts.next().flatten(), parts.next()) {
        (Some(1), Some(minor), patch) => {
            let patch = patch.flatten().unwrap_or(0);
            (minor, patch) >= (21, 9)
        }
        // The new version numbers: 26.1, 26.3.
        (Some(major), Some(_), _) => major >= 26,
        _ => false,
    }
}

/// A new secret: 40 letters and digits, as the management protocol wants.
fn new_secret() -> Result<String, String> {
    random::random_bytes::<20>()
        .map(hex::encode)
        .map_err(|error| error.to_string())
}

fn is_valid_secret(secret: &str) -> bool {
    secret.len() == 40 && secret.chars().all(|c| c.is_ascii_alphanumeric())
}

/// Puts the channel's settings in `doc`. Returns whether anything changed.
fn apply_settings(
    doc: &mut PropertiesDoc,
    channel: Channel,
    port: u16,
    secret: &dyn Fn() -> Result<String, String>,
) -> Result<bool, String> {
    let before = doc.render();
    let port_text = port.to_string();
    match channel {
        Channel::Management => {
            doc.set("management-server-enabled", "true");
            doc.set("management-server-host", "127.0.0.1");
            doc.set("management-server-port", &port_text);
            // No TLS: nothing leaves this machine, and a keystore is a lot to ask for that.
            doc.set("management-server-tls-enabled", "false");
            if !doc.get("management-server-secret").is_some_and(is_valid_secret) {
                doc.set("management-server-secret", &secret()?);
            }
            // After moving up from a version that used RCON on this port.
            if doc.get("rcon.port") == Some(port_text.as_str()) {
                doc.set("enable-rcon", "false");
            }
        }
        Channel::Rcon if doc.get("enable-rcon") != Some("true") => {}
        Channel::Rcon => {
            doc.set("rcon.port", &port_text);
            if doc.get("rcon.password").is_none_or(str::is_empty) {
                doc.set("rcon.password", &secret()?);
            }
            // Operators needn't see every command this app runs.
            doc.set("broadcast-rcon-to-ops", "false");
        }
    }
    Ok(doc.render() != before)
}

/// Sets the channel up, before a start. Only writes when something needs to change.
pub async fn prepare_start(instance: &Instance) -> Result<(), String> {
    let Some(port) = instance.definition.ports.control else {
        return Ok(());
    };
    let server =
        MinecraftServer::from_table(&instance.definition.server).map_err(|error| error.to_string())?;
    let files = ServerFiles::open(instance)
        .await
        .map_err(|error| error.to_string())?;
    let path = SafeRelPath::parse(PROPERTIES_FILE).map_err(|error| error.to_string())?;
    let (mut doc, revision) = match files.read_text(path.clone()).await {
        Ok(file) => (PropertiesDoc::parse(&file.content), Some(file.revision)),
        Err(FilesError::NotFound(_)) => (PropertiesDoc::parse(""), None),
        Err(error) => return Err(error.to_string()),
    };
    if apply_settings(&mut doc, channel_for(&server.mc_version), port, &new_secret)? {
        files
            .write_text(path, doc.render(), revision)
            .await
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

/// The server's settings, as they are in its file.
async fn read_settings(instance: &Instance) -> Result<PropertiesDoc, ControlError> {
    let files = ServerFiles::open(instance).await?;
    let path = SafeRelPath::parse(PROPERTIES_FILE).map_err(FilesError::from)?;
    Ok(PropertiesDoc::parse(&files.read_text(path).await?.content))
}

fn server_channel(instance: &Instance) -> Channel {
    MinecraftServer::from_table(&instance.definition.server)
        .map_or(Channel::Rcon, |server| channel_for(&server.mc_version))
}

/// Connects to the running server's management protocol.
pub async fn management(instance: &Instance) -> Result<Mcsmp, ControlError> {
    if server_channel(instance) != Channel::Management {
        return Err(ControlError::WrongChannel);
    }
    let port = instance.definition.ports.control.ok_or(ControlError::NoPort)?;
    let doc = read_settings(instance).await?;
    let secret = doc
        .get("management-server-secret")
        .ok_or(ControlError::NoSecret)?;
    Ok(Mcsmp::connect(port, secret, CONNECT_TIMEOUT).await?)
}

/// Connects to the running server's RCON.
pub async fn rcon(instance: &Instance) -> Result<Rcon, ControlError> {
    let port = instance.definition.ports.control.ok_or(ControlError::NoPort)?;
    let doc = read_settings(instance).await?;
    let password = doc.get("rcon.password").ok_or(ControlError::NoSecret)?;
    Ok(Rcon::connect(port, password, CONNECT_TIMEOUT).await?)
}

/// How the channel of a server is doing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ControlStatus {
    pub channel: Channel,
    pub connected: bool,
    /// Why not, if the server runs and it still couldn't connect.
    pub problem: Option<String>,
    /// The management protocol's version, and what this server offers.
    pub api_version: Option<String>,
    pub methods: Vec<String>,
}

/// Tries the channel of a server that is running.
pub async fn status(instance: &Instance) -> ControlStatus {
    let channel = server_channel(instance);
    let mut status = ControlStatus {
        channel,
        connected: false,
        problem: None,
        api_version: None,
        methods: Vec::new(),
    };
    // RCON is the user's to turn on; off isn't a problem, and there is nothing to try.
    if channel == Channel::Rcon
        && let Ok(doc) = read_settings(instance).await
        && doc.get("enable-rcon") != Some("true")
    {
        return status;
    }
    let attempt = match channel {
        Channel::Management => {
            async {
                let mut client = management(instance).await?;
                Ok::<_, ControlError>(Some(client.discover().await?))
            }
            .await
        }
        Channel::Rcon => {
            // A command, since logging in alone doesn't show that commands get their replies.
            async {
                let reply = rcon(instance).await?.command("list").await?;
                if reply.is_empty() {
                    return Err(RconError::Invalid("no reply to a command").into());
                }
                Ok::<_, ControlError>(None)
            }
            .await
        }
    };
    match attempt {
        Ok(discovered) => {
            status.connected = true;
            if let Some(discovered) = discovered {
                status.api_version = discovered.version;
                status.methods = discovered.methods;
            }
        }
        Err(error) => status.problem = Some(error.to_string()),
    }
    status
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn the_channel_follows_the_minecraft_version() {
        for (version, channel) in [
            ("26.3", Channel::Management),
            ("26.1.2", Channel::Management),
            ("1.21.9", Channel::Management),
            ("1.21.11", Channel::Management),
            ("1.21.9-rc1", Channel::Management),
            ("25w35a", Channel::Management),
            ("26w01a", Channel::Management),
            ("1.21.8", Channel::Rcon),
            ("1.21", Channel::Rcon),
            ("1.20.1", Channel::Rcon),
            ("1.7.10", Channel::Rcon),
            ("25w34a", Channel::Rcon),
            ("24w10a", Channel::Rcon),
            ("something odd", Channel::Rcon),
            ("", Channel::Rcon),
        ] {
            assert_eq!(channel_for(version), channel, "{version}");
        }
    }

    // A `Result` because it stands in for `new_secret`, which can fail.
    #[allow(clippy::unnecessary_wraps)]
    fn secret() -> Result<String, String> {
        Ok("a".repeat(40))
    }

    #[test]
    fn the_management_protocol_is_set_up_for_loopback_with_no_tls() {
        let mut doc = PropertiesDoc::parse("motd=Hi\n");
        assert!(apply_settings(&mut doc, Channel::Management, 35565, &secret).unwrap());
        assert_eq!(doc.get("management-server-enabled"), Some("true"));
        assert_eq!(doc.get("management-server-host"), Some("127.0.0.1"));
        assert_eq!(doc.get("management-server-port"), Some("35565"));
        assert_eq!(doc.get("management-server-tls-enabled"), Some("false"));
        assert_eq!(doc.get("management-server-secret"), Some("a".repeat(40).as_str()));
        assert_eq!(doc.get("motd"), Some("Hi"), "the rest stays");
        assert_eq!(doc.get("enable-rcon"), None, "RCON is left alone");

        assert!(
            !apply_settings(&mut doc, Channel::Management, 35565, &|| Err("not again".into())).unwrap(),
            "a second start has nothing to change, and keeps its secret"
        );
    }

    #[test]
    fn a_secret_the_protocol_would_refuse_is_replaced() {
        let mut doc = PropertiesDoc::parse("management-server-secret=short\n");
        apply_settings(&mut doc, Channel::Management, 35565, &secret).unwrap();
        assert_eq!(doc.get("management-server-secret"), Some("a".repeat(40).as_str()));
    }

    /// It would listen on every network interface, so that is the user's choice to make.
    #[test]
    fn rcon_is_never_turned_on_by_this() {
        for text in ["motd=Hi\n", "enable-rcon=false\n"] {
            let mut doc = PropertiesDoc::parse(text);
            assert!(
                !apply_settings(&mut doc, Channel::Rcon, 35566, &secret).unwrap(),
                "{text}"
            );
            assert_eq!(doc.render(), text);
        }
    }

    #[test]
    fn rcon_that_was_turned_on_gets_its_port_and_a_password_and_keeps_one_it_has() {
        let mut doc = PropertiesDoc::parse("enable-rcon=true\nrcon.password=\n");
        assert!(apply_settings(&mut doc, Channel::Rcon, 35566, &secret).unwrap());
        assert_eq!(doc.get("rcon.port"), Some("35566"));
        assert_eq!(doc.get("rcon.password"), Some("a".repeat(40).as_str()));
        assert_eq!(doc.get("broadcast-rcon-to-ops"), Some("false"));

        let mut doc = PropertiesDoc::parse("enable-rcon=true\nrcon.password=mine\n");
        apply_settings(&mut doc, Channel::Rcon, 35566, &secret).unwrap();
        assert_eq!(doc.get("rcon.password"), Some("mine"));
    }

    #[test]
    fn moving_up_a_version_turns_the_old_rcon_off() {
        let mut doc = PropertiesDoc::parse("enable-rcon=true\nrcon.port=35566\n");
        apply_settings(&mut doc, Channel::Management, 35566, &secret).unwrap();
        assert_eq!(doc.get("enable-rcon"), Some("false"));

        let mut theirs = PropertiesDoc::parse("enable-rcon=true\nrcon.port=25575\n");
        apply_settings(&mut theirs, Channel::Management, 35566, &secret).unwrap();
        assert_eq!(
            theirs.get("enable-rcon"),
            Some("true"),
            "not one the user set on another port"
        );
    }

    #[test]
    fn secrets_are_long_enough_and_differ() {
        let (one, two) = (new_secret().unwrap(), new_secret().unwrap());
        assert!(is_valid_secret(&one), "{one}");
        assert_ne!(one, two);
    }
}
