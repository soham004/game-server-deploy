//! Files every installed server gets: `eula.txt` from the recorded consent, and a minimal
//! `server.properties` with the values the app manages (port, MOTD). The server fills in every
//! other property on first start.

use std::{fmt::Write as _, path::Path};

use chrono::{DateTime, Utc};

use super::InstallError;

pub async fn write_eula(server_dir: &Path, accepted_at: DateTime<Utc>) -> Result<(), InstallError> {
    let path = server_dir.join("eula.txt");
    let text = format!(
        "# Accepted by the server owner on {} in {}.\n# https://aka.ms/MinecraftEULA\neula=true\n",
        accepted_at.to_rfc3339(),
        gsd_brand::NAME
    );
    tokio::fs::write(&path, text)
        .await
        .map_err(|source| InstallError::Io { path, source })
}

/// Points an existing configuration at `port` (`server-port` and `query.port`): the sandbox
/// only lets a server bind its own port. Used for copied servers (clones, imports); creates the
/// file if there is none. Everything else in the file stays as it was.
pub fn point_at_port(server_dir: &Path, port: u16) -> Result<(), InstallError> {
    let path = server_dir.join("server.properties");
    let io = |source| InstallError::Io {
        path: path.clone(),
        source,
    };
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(error) => return Err(io(error)),
    };
    let mut doc = crate::properties::PropertiesDoc::from_bytes(&bytes);
    doc.set("server-port", &port.to_string());
    doc.set("query.port", &port.to_string());
    std::fs::write(&path, doc.render()).map_err(io)
}

/// Writes the managed properties, unless a configuration already exists (reinstalls keep it).
pub async fn seed_properties(server_dir: &Path, port: u16, motd: &str) -> Result<(), InstallError> {
    let path = server_dir.join("server.properties");
    if tokio::fs::try_exists(&path).await.unwrap_or(false) {
        return Ok(());
    }
    let text = format!(
        "server-port={port}\nquery.port={port}\nmotd={}\n",
        escape_property(motd)
    );
    tokio::fs::write(&path, text)
        .await
        .map_err(|source| InstallError::Io { path, source })
}

/// Java `.properties` value escaping (backslashes, line breaks, non-ASCII as `\uXXXX`).
fn escape_property(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_ascii() => out.push(c),
            c => {
                let mut units = [0_u16; 2];
                for unit in c.encode_utf16(&mut units) {
                    let _ = write!(out, "\\u{unit:04x}");
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_property_values() {
        assert_eq!(escape_property(r"A \ B"), r"A \\ B");
        assert_eq!(escape_property("two\nlines"), r"two\nlines");
        let backslash = '\\';
        assert_eq!(
            escape_property("Caf\u{e9} \u{2713}"),
            format!("Caf{backslash}u00e9 {backslash}u2713")
        );
        assert_eq!(escape_property("Friends' server #1"), "Friends' server #1");
    }

    #[test]
    fn points_an_existing_configuration_at_a_new_port() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("server.properties"),
            "#comment
motd=Hi
server-port=25565
query.port=25565
",
        )
        .unwrap();
        point_at_port(dir.path(), 25570).unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.path().join("server.properties")).unwrap(),
            "#comment
motd=Hi
server-port=25570
query.port=25570
"
        );
        let fresh = tempfile::tempdir().unwrap();
        point_at_port(fresh.path(), 25571).unwrap();
        assert_eq!(
            std::fs::read_to_string(fresh.path().join("server.properties")).unwrap(),
            "server-port=25571
query.port=25571
"
        );
    }

    #[tokio::test]
    async fn writes_eula_and_seeds_properties_once() {
        let dir = tempfile::tempdir().unwrap();
        write_eula(dir.path(), Utc::now()).await.unwrap();
        assert!(
            std::fs::read_to_string(dir.path().join("eula.txt"))
                .unwrap()
                .contains("eula=true")
        );

        seed_properties(dir.path(), 25570, "Friends' server")
            .await
            .unwrap();
        let props = std::fs::read_to_string(dir.path().join("server.properties")).unwrap();
        assert!(props.contains("server-port=25570") && props.contains("motd=Friends' server"));

        seed_properties(dir.path(), 1, "changed").await.unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.path().join("server.properties")).unwrap(),
            props,
            "not overwritten"
        );
    }
}
