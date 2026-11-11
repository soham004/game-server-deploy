//! Files every installed server gets: `eula.txt` from the recorded consent, and a minimal
//! `server.properties` with the values the app manages (port, MOTD). The server fills in every
//! other property on first start.
//!
//! They go into a folder the server owns, and on a reinstall it has had its hands on it. So they are
//! written through the file manager ([`ServerFiles`]): a link the server left under one of these
//! names is replaced, never written through.

use std::fmt::Write as _;

use chrono::{DateTime, Utc};
use gsd_core::files::{EntryKind, FilesError, SafeRelPath, ServerFiles};

use super::InstallError;
use crate::properties::{MAX_FILE_BYTES, PropertiesDoc};

const EULA: &str = "eula.txt";
const PROPERTIES: &str = "server.properties";

pub async fn write_eula(files: &ServerFiles, accepted_at: DateTime<Utc>) -> Result<(), InstallError> {
    let text = format!(
        "# Accepted by the server owner on {} in {}.\n# https://aka.ms/MinecraftEULA\neula=true\n",
        accepted_at.to_rfc3339(),
        gsd_brand::NAME
    );
    files.replace_file(file(EULA)?, text.into_bytes()).await?;
    Ok(())
}

/// Points an existing configuration at `port` (see [`point_doc_at_port`]). Used for copied servers
/// (clones, imports); creates the file if there is none, or replaces a link of that name.
/// Everything else in it stays as it was.
pub async fn point_at_port(files: &ServerFiles, port: u16) -> Result<(), InstallError> {
    let path = file(PROPERTIES)?;
    let bytes = if files.kind(path.clone()).await? == Some(EntryKind::File) {
        files
            .read_optional(path.clone(), MAX_FILE_BYTES as u64)
            .await?
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    let mut doc = PropertiesDoc::from_bytes(&bytes);
    point_doc_at_port(&mut doc, port);
    files.replace_file(path, doc.render().into_bytes()).await?;
    Ok(())
}

/// Sets `server-port` and `query.port` to `port`: the sandbox only lets a server bind its own
/// port. A copied configuration also loses its control channel's secrets, which stay with the
/// server they came from; the next start makes new ones.
pub fn point_doc_at_port(doc: &mut PropertiesDoc, port: u16) {
    doc.set("server-port", &port.to_string());
    doc.set("query.port", &port.to_string());
    doc.remove("rcon.password");
    doc.remove("management-server-secret");
}

/// Writes the managed properties, unless a configuration already exists (reinstalls keep it).
/// Only a real file counts as one: a link (or anything else) under its name is replaced.
pub async fn seed_properties(files: &ServerFiles, port: u16, motd: &str) -> Result<(), InstallError> {
    let path = file(PROPERTIES)?;
    if files.kind(path.clone()).await? == Some(EntryKind::File) {
        return Ok(());
    }
    let text = format!(
        "server-port={port}\nquery.port={port}\nmotd={}\n",
        escape_property(motd)
    );
    files.replace_file(path, text.into_bytes()).await?;
    Ok(())
}

fn file(name: &str) -> Result<SafeRelPath, InstallError> {
    Ok(SafeRelPath::parse(name).map_err(FilesError::from)?)
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

    async fn folder(dir: &std::path::Path) -> ServerFiles {
        crate::test_support::server_files(dir).await
    }

    #[tokio::test]
    async fn points_an_existing_configuration_at_a_new_port() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("server.properties"),
            "#comment
motd=Hi
server-port=25565
query.port=25565
rcon.password=theirs
",
        )
        .unwrap();
        point_at_port(&folder(dir.path()).await, 25570).await.unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.path().join("server.properties")).unwrap(),
            "#comment
motd=Hi
server-port=25570
query.port=25570
"
        );
        let fresh = tempfile::tempdir().unwrap();
        point_at_port(&folder(fresh.path()).await, 25571).await.unwrap();
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
        let files = folder(dir.path()).await;
        write_eula(&files, Utc::now()).await.unwrap();
        assert!(
            std::fs::read_to_string(dir.path().join("eula.txt"))
                .unwrap()
                .contains("eula=true")
        );

        seed_properties(&files, 25570, "Friends' server").await.unwrap();
        let props = std::fs::read_to_string(dir.path().join("server.properties")).unwrap();
        assert!(props.contains("server-port=25570") && props.contains("motd=Friends' server"));

        seed_properties(&files, 1, "changed").await.unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.path().join("server.properties")).unwrap(),
            props,
            "not overwritten"
        );
    }

    /// On a reinstall the folder has been the server's: a link it left where these files go must
    /// be replaced by the file, not followed to what it points at.
    #[cfg(unix)]
    #[tokio::test]
    async fn links_the_server_left_are_replaced_not_written_through() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().unwrap();
        let server = dir.path().join("server");
        std::fs::create_dir(&server).unwrap();
        let outside = dir.path().join("outside.txt");
        std::fs::write(&outside, "untouched").unwrap();
        let missing = dir.path().join("made-by-the-app");
        symlink(&outside, server.join("eula.txt")).unwrap();
        symlink(&missing, server.join("server.properties")).unwrap();
        let files = folder(&server).await;

        write_eula(&files, Utc::now()).await.unwrap();
        seed_properties(&files, 25570, "x").await.unwrap();
        assert_eq!(std::fs::read_to_string(&outside).unwrap(), "untouched");
        assert!(!missing.exists());
        for name in ["eula.txt", "server.properties"] {
            let kind = std::fs::symlink_metadata(server.join(name)).unwrap();
            assert!(kind.is_file(), "{name}: the link was replaced by the file");
        }

        // A copied server can bring a link along too.
        std::fs::remove_file(server.join("server.properties")).unwrap();
        symlink(&outside, server.join("server.properties")).unwrap();
        point_at_port(&files, 25571).await.unwrap();
        assert_eq!(std::fs::read_to_string(&outside).unwrap(), "untouched");
        assert_eq!(
            std::fs::read_to_string(server.join("server.properties")).unwrap(),
            "server-port=25571\nquery.port=25571\n"
        );
    }
}
