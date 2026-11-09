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
//!
//! Hangar has no packwiz updater, so plugins from Hangar get a table of the same shape that this
//! app reads and other tools skip (`[update.hangar]` with `project-id` and `version-id`).

use serde::{Deserialize, Serialize};

use crate::model::{ContentVersion, Project, SideSupport, SourceKind};

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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hangar: Option<HangarUpdate>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub curseforge: Option<CurseForgeUpdate>,
}

impl Update {
    fn is_empty(&self) -> bool {
        self.modrinth.is_none() && self.hangar.is_none() && self.curseforge.is_none()
    }
}

/// packwiz's table for `CurseForge` (numbers, as packwiz and Prism write them), plus the page address
/// (other tools skip it), which the ids don't give.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct CurseForgeUpdate {
    pub project_id: u64,
    pub file_id: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page_url: Option<String>,
}

/// This app's own updater table for Hangar (see the module docs).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct HangarUpdate {
    pub project_id: String,
    pub version_id: String,
    /// Hangar's page addresses need the owner, which the ids don't say.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page_url: Option<String>,
}

/// Where an installed file came from, per its index entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Origin {
    pub source: SourceKind,
    pub project_id: String,
    pub version_id: String,
    /// The project's page on its source's website, when known.
    pub page_url: Option<String>,
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
    /// The entry for a file of `version` of `project`.
    pub fn new(project: &Project, version: &ContentVersion) -> Self {
        let (hash_format, hash) = version
            .file
            .best_hash()
            .map_or(("sha512", ""), |(format, hash)| (format.as_str(), hash));
        let (project_id, version_id) = (version.project_id.clone(), version.id.clone());
        let page_url = Some(project.page_url.clone()).filter(|url| !url.is_empty());
        let update = match project.source {
            SourceKind::Modrinth => Update {
                modrinth: Some(ModrinthUpdate {
                    mod_id: project_id,
                    version: version_id,
                }),
                ..Update::default()
            },
            SourceKind::Hangar => Update {
                hangar: Some(HangarUpdate {
                    project_id,
                    version_id,
                    page_url,
                }),
                ..Update::default()
            },
            // CurseForge's ids are numbers, so a version from there always parses.
            SourceKind::CurseForge => Update {
                curseforge: Some(CurseForgeUpdate {
                    project_id: project_id.parse().unwrap_or_default(),
                    file_id: version_id.parse().unwrap_or_default(),
                    page_url,
                }),
                ..Update::default()
            },
        };
        Self {
            name: project.title.clone(),
            filename: version.file.filename.clone(),
            // A server only cares that it runs there; packwiz reads "server" as "not on clients".
            side: if project.client_side == SideSupport::Unsupported {
                "server"
            } else {
                "both"
            }
            .into(),
            download: Download {
                url: version.file.url.clone(),
                hash_format: hash_format.into(),
                hash: hash.into(),
            },
            update,
        }
    }

    pub fn to_toml(&self) -> String {
        // Serialising plain strings into a table can't fail.
        toml::to_string(self).unwrap_or_default()
    }

    pub fn parse(text: &str) -> Option<Self> {
        toml::from_str(text).ok()
    }

    /// Where the file came from, when it was a source this app knows.
    pub fn origin(&self) -> Option<Origin> {
        let update = &self.update;
        if let Some(modrinth) = &update.modrinth {
            return Some(Origin {
                source: SourceKind::Modrinth,
                project_id: modrinth.mod_id.clone(),
                version_id: modrinth.version.clone(),
                page_url: Some(format!("https://modrinth.com/project/{}", modrinth.mod_id)),
            });
        }
        if let Some(hangar) = &update.hangar {
            return Some(Origin {
                source: SourceKind::Hangar,
                project_id: hangar.project_id.clone(),
                version_id: hangar.version_id.clone(),
                page_url: hangar.page_url.clone(),
            });
        }
        let curseforge = update.curseforge.as_ref()?;
        Some(Origin {
            source: SourceKind::CurseForge,
            project_id: curseforge.project_id.to_string(),
            version_id: curseforge.file_id.to_string(),
            page_url: curseforge.page_url.clone(),
        })
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

    fn project(source: SourceKind, title: &str, page_url: &str) -> Project {
        Project {
            source,
            id: "id".into(),
            slug: title.to_lowercase(),
            title: title.into(),
            description: String::new(),
            body: String::new(),
            icon_url: None,
            server_side: SideSupport::Optional,
            client_side: SideSupport::Optional,
            page_url: page_url.into(),
        }
    }

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
                sha256: None,
                sha512: Some("b".repeat(128)),
                manual_url: None,
            },
        }
    }

    #[test]
    fn writes_the_packwiz_format_and_reads_it_back() {
        let entry = IndexEntry::new(&project(SourceKind::Modrinth, "Lithium", ""), &version());
        let text = entry.to_toml();
        assert!(text.contains("name = \"Lithium\""), "{text}");
        assert!(text.contains("hash-format = \"sha512\""), "{text}");
        assert!(
            text.contains("[update.modrinth]") && text.contains("mod-id = \"gvQqBUqZ\""),
            "{text}"
        );
        assert_eq!(IndexEntry::parse(&text), Some(entry.clone()));
        assert_eq!(
            entry
                .origin()
                .map(|origin| (origin.source, origin.project_id, origin.version_id)),
            Some((SourceKind::Modrinth, "gvQqBUqZ".into(), "fWv5bnZk".into()))
        );
    }

    #[test]
    fn hangar_plugins_get_their_own_table_and_the_strongest_hash() {
        let mut plugin = version();
        (plugin.id, plugin.project_id) = ("33491".into(), "4".into());
        plugin.file.sha1 = None;
        plugin.file.sha512 = None;
        plugin.file.sha256 = Some("c".repeat(64));
        let page = "https://hangar.papermc.io/pop4959/Chunky";
        let entry = IndexEntry::new(&project(SourceKind::Hangar, "Chunky", page), &plugin);
        let text = entry.to_toml();
        assert!(text.contains("hash-format = \"sha256\""), "{text}");
        assert!(text.contains("[update.hangar]"), "{text}");
        assert!(
            text.contains("project-id = \"4\"") && text.contains("version-id = \"33491\""),
            "{text}"
        );
        assert!(!text.contains("[update.modrinth]"), "{text}");
        let origin = IndexEntry::parse(&text).unwrap().origin().unwrap();
        assert_eq!(
            (
                origin.source,
                origin.project_id.as_str(),
                origin.page_url.as_deref()
            ),
            (SourceKind::Hangar, "4", Some(page))
        );
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
            (
                entry.side.as_str(),
                entry.origin().map(|origin| origin.project_id)
            ),
            ("client", Some("AANobbMI".to_owned()))
        );
        let curseforge = "name = \"x\"\nfilename = \"x.jar\"\n[download]\nhash-format = \"sha1\"\nhash = \"a\"\nmode = \"metadata:curseforge\"\n[update.curseforge]\nproject-id = 1\nfile-id = 2\n";
        let origin = IndexEntry::parse(curseforge).unwrap().origin().unwrap();
        assert_eq!(
            (
                origin.source,
                origin.project_id.as_str(),
                origin.version_id.as_str()
            ),
            (SourceKind::CurseForge, "1", "2"),
            "as packwiz writes it, without an address"
        );
        let unknown = "name = \"x\"\nfilename = \"x.jar\"\n[download]\nhash-format = \"sha1\"\nhash = \"a\"\n[update.other]\nid = 1\n";
        assert_eq!(
            IndexEntry::parse(unknown).unwrap().origin(),
            None,
            "other sources are kept, not misread"
        );
        assert_eq!(IndexEntry::parse("not toml ["), None);
    }

    #[test]
    fn curseforge_files_use_packwiz_numbers_and_keep_their_page() {
        let mut file = version();
        (file.id, file.project_id) = ("5002".into(), "1002".into());
        let page = "https://www.curseforge.com/minecraft/mc-mods/cf-mod";
        let entry = IndexEntry::new(&project(SourceKind::CurseForge, "CF Mod", page), &file);
        let text = entry.to_toml();
        assert!(
            text.contains("[update.curseforge]")
                && text.contains("project-id = 1002")
                && text.contains("file-id = 5002"),
            "{text}"
        );
        let origin = IndexEntry::parse(&text).unwrap().origin().unwrap();
        assert_eq!(origin.page_url.as_deref(), Some(page));
    }

    #[test]
    fn file_names_come_from_slugs_and_stay_plain() {
        assert_eq!(file_name("fabric-api"), "fabric-api.pw.toml");
        assert_eq!(file_name("../Evil Mod"), "---evil-mod.pw.toml");
        assert_eq!(file_name(""), "unnamed.pw.toml");
        assert!(is_entry("lithium.pw.toml") && !is_entry("lithium.jar"));
    }
}
