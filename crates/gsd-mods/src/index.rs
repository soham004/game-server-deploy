//! The packwiz index: one `<folder>/.index/<slug>.pw.toml` per installed file, saying where it
//! came from (the format Prism Launcher and packwiz use, so servers stay interoperable).
//!
//! ```toml
//! name = "Lithium"
//! filename = "lithium-fabric-0.26.3+mc26.3.jar"
//! side = "both"
//!
//! [download]
//! url = "https://cdn.modrinth.com/data/gvQqBUqZ/versions/fWv5bnZk/lithium-fabric-0.26.3%2Bmc26.3.jar"
//! hash-format = "sha512"
//! hash = "…"
//!
//! [update.modrinth]
//! mod-id = "gvQqBUqZ"
//! version = "fWv5bnZk"
//! ```

use serde::{Deserialize, Serialize};

use crate::model::{ContentVersion, SideSupport};

/// The folder (inside the content folder) that holds the index.
pub const INDEX_FOLDER: &str = ".index";
const SUFFIX: &str = ".pw.toml";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct IndexEntry {
    pub name: String,
    pub filename: String,
    #[serde(default = "both")]
    pub side: String,
    pub download: Download,
    #[serde(default, skip_serializing_if = "Update::is_empty")]
    pub update: Update,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct Download {
    #[serde(default)]
    pub url: String,
    pub hash_format: String,
    pub hash: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Update {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modrinth: Option<ModrinthUpdate>,
}

impl Update {
    fn is_empty(&self) -> bool {
        self.modrinth.is_none()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct ModrinthUpdate {
    pub mod_id: String,
    pub version: String,
}

fn both() -> String {
    "both".into()
}

impl IndexEntry {
    /// The entry for a file installed from Modrinth.
    pub fn modrinth(title: &str, version: &ContentVersion, client_side: SideSupport) -> Self {
        let (hash_format, hash) = match (&version.file.sha512, &version.file.sha1) {
            (Some(sha512), _) => ("sha512", sha512.clone()),
            (None, Some(sha1)) => ("sha1", sha1.clone()),
            (None, None) => ("sha512", String::new()),
        };
        Self {
            name: title.to_owned(),
            filename: version.file.filename.clone(),
            // A server only cares that it runs there; packwiz reads "server" as "not on clients".
            side: if client_side == SideSupport::Unsupported {
                "server"
            } else {
                "both"
            }
            .into(),
            download: Download {
                url: version.file.url.clone(),
                hash_format: hash_format.into(),
                hash,
            },
            update: Update {
                modrinth: Some(ModrinthUpdate {
                    mod_id: version.project_id.clone(),
                    version: version.id.clone(),
                }),
            },
        }
    }

    pub fn to_toml(&self) -> String {
        // Serialising plain strings into a table can't fail.
        toml::to_string(self).unwrap_or_default()
    }

    pub fn parse(text: &str) -> Option<Self> {
        toml::from_str(text).ok()
    }

    /// The Modrinth project, when the file came from Modrinth.
    pub fn modrinth_project(&self) -> Option<&str> {
        self.update.modrinth.as_ref().map(|update| update.mod_id.as_str())
    }
}

/// `<slug>.pw.toml`, with anything unusual in the slug replaced (it becomes a file name).
pub fn file_name(slug: &str) -> String {
    let safe: String = slug
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    format!(
        "{}{SUFFIX}",
        if safe.is_empty() { "unnamed".into() } else { safe }
    )
}

/// Whether a file in the index folder is an index entry.
pub fn is_entry(name: &str) -> bool {
    name.ends_with(SUFFIX)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use crate::model::{Channel, ContentFile};

    fn version() -> ContentVersion {
        ContentVersion {
            id: "fWv5bnZk".into(),
            project_id: "gvQqBUqZ".into(),
            name: "Lithium 0.26.3".into(),
            version_number: "mc26.3-0.26.3-fabric".into(),
            channel: Channel::Release,
            published: None,
            game_versions: vec!["26.3".into()],
            loaders: vec!["fabric".into()],
            dependencies: vec![],
            file: ContentFile {
                url: "https://cdn.modrinth.com/data/gvQqBUqZ/versions/fWv5bnZk/lithium.jar".into(),
                filename: "lithium-fabric-0.26.3+mc26.3.jar".into(),
                size: 919_892,
                sha1: Some("a".repeat(40)),
                sha512: Some("b".repeat(128)),
            },
        }
    }

    #[test]
    fn writes_the_packwiz_format_and_reads_it_back() {
        let entry = IndexEntry::modrinth("Lithium", &version(), SideSupport::Optional);
        let text = entry.to_toml();
        assert!(text.contains("name = \"Lithium\""), "{text}");
        assert!(text.contains("hash-format = \"sha512\""), "{text}");
        assert!(
            text.contains("[update.modrinth]") && text.contains("mod-id = \"gvQqBUqZ\""),
            "{text}"
        );
        assert_eq!(IndexEntry::parse(&text), Some(entry.clone()));
        assert_eq!(entry.modrinth_project(), Some("gvQqBUqZ"));
    }

    #[test]
    fn reads_entries_written_by_packwiz_and_prism() {
        let prism = r#"
filename = "sodium-fabric-0.6.0+mc1.21.1.jar"
name = "Sodium"
side = "client"

[download]
hash = "abc"
hash-format = "sha1"
mode = "url"
url = "https://cdn.modrinth.com/x.jar"

[update.modrinth]
mod-id = "AANobbMI"
version = "yaoBL9D9"
"#;
        let entry = IndexEntry::parse(prism).unwrap();
        assert_eq!(
            (entry.side.as_str(), entry.modrinth_project()),
            ("client", Some("AANobbMI"))
        );
        let curseforge_only = "name = \"x\"\nfilename = \"x.jar\"\n[download]\nhash-format = \"sha1\"\nhash = \"a\"\n[update.curseforge]\nproject-id = 1\nfile-id = 2\n";
        assert_eq!(
            IndexEntry::parse(curseforge_only).unwrap().modrinth_project(),
            None,
            "other sources are kept, not misread"
        );
        assert_eq!(IndexEntry::parse("not toml ["), None);
    }

    #[test]
    fn file_names_come_from_slugs_and_stay_plain() {
        assert_eq!(file_name("fabric-api"), "fabric-api.pw.toml");
        assert_eq!(file_name("../Evil Mod"), "---evil-mod.pw.toml");
        assert_eq!(file_name(""), "unnamed.pw.toml");
        assert!(is_entry("lithium.pw.toml") && !is_entry("lithium.jar"));
    }
}
