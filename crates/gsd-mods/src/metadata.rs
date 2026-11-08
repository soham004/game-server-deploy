//! What a jar says about itself: its id, name and version, from the descriptor its loader reads
//! (`fabric.mod.json`, `quilt.mod.json`, `META-INF/neoforge.mods.toml` or `mods.toml`,
//! `paper-plugin.yml` or `plugin.yml`). Jars come from the server folder and may be anything,
//! so every step tolerates garbage and falls back to nothing.

use std::io::{Read, Seek};

use serde::Serialize;

/// Descriptors bigger than this are not read (they are a few KB).
const MAX_DESCRIPTOR_BYTES: u64 = 1024 * 1024;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct JarInfo {
    /// The mod id or plugin name the loader knows it by.
    pub id: Option<String>,
    pub name: Option<String>,
    pub version: Option<String>,
}

/// Reads what the jar says about itself; an unreadable jar gives an empty [`JarInfo`].
pub fn read(jar: impl Read + Seek) -> JarInfo {
    let Ok(mut archive) = zip::ZipArchive::new(jar) else {
        return JarInfo::default();
    };
    let mut text = |name: &str| -> Option<String> {
        let entry = archive.by_name(name).ok()?;
        let mut content = String::new();
        entry
            .take(MAX_DESCRIPTOR_BYTES)
            .read_to_string(&mut content)
            .ok()?;
        Some(content)
    };
    if let Some(info) = text("fabric.mod.json").and_then(|json| fabric(&json)) {
        return info;
    }
    if let Some(info) = text("quilt.mod.json").and_then(|json| quilt(&json)) {
        return info;
    }
    for descriptor in ["META-INF/neoforge.mods.toml", "META-INF/mods.toml"] {
        if let Some(toml) = text(descriptor) {
            let manifest_version =
                text("META-INF/MANIFEST.MF").and_then(|manifest| implementation_version(&manifest));
            if let Some(info) = forge(&toml, manifest_version) {
                return info;
            }
        }
    }
    for descriptor in ["paper-plugin.yml", "plugin.yml"] {
        if let Some(info) = text(descriptor).and_then(|yaml| plugin(&yaml)) {
            return info;
        }
    }
    JarInfo::default()
}

fn fabric(json: &str) -> Option<JarInfo> {
    let value: serde_json::Value = serde_json::from_str(json).ok()?;
    Some(JarInfo {
        id: string(&value["id"]),
        name: string(&value["name"]),
        version: string(&value["version"]),
    })
}

fn quilt(json: &str) -> Option<JarInfo> {
    let value: serde_json::Value = serde_json::from_str(json).ok()?;
    let loader = &value["quilt_loader"];
    Some(JarInfo {
        id: string(&loader["id"]),
        name: string(&loader["metadata"]["name"]),
        version: string(&loader["version"]),
    })
}

/// The first `[[mods]]` entry. `${file.jarVersion}` means the manifest's Implementation-Version.
fn forge(text: &str, manifest_version: Option<String>) -> Option<JarInfo> {
    let value: toml::Table = toml::from_str(text).ok()?;
    let first = value.get("mods")?.as_array()?.first()?.as_table()?;
    let field = |key: &str| first.get(key).and_then(toml::Value::as_str).map(str::to_owned);
    let version = match field("version") {
        Some(version) if version.contains("${") => manifest_version,
        other => other,
    };
    Some(JarInfo {
        id: field("modId"),
        name: field("displayName"),
        version,
    })
}

/// `name` and `version` from a plugin descriptor (top-level YAML scalars).
fn plugin(yaml: &str) -> Option<JarInfo> {
    let scalar = |key: &str| {
        yaml.lines().find_map(|line| {
            let value = line.strip_prefix(key)?.strip_prefix(':')?.trim();
            let value = value.trim_matches(|c| c == '"' || c == '\'');
            (!value.is_empty()).then(|| value.to_owned())
        })
    };
    let name = scalar("name")?;
    Some(JarInfo {
        id: Some(name.clone()),
        name: Some(name),
        version: scalar("version"),
    })
}

fn implementation_version(manifest: &str) -> Option<String> {
    manifest
        .lines()
        .find_map(|line| line.strip_prefix("Implementation-Version:"))
        .map(|version| version.trim().to_owned())
}

fn string(value: &serde_json::Value) -> Option<String> {
    value.as_str().filter(|text| !text.is_empty()).map(str::to_owned)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use std::io::{Cursor, Write};

    use super::*;

    /// A jar (zip) with these files.
    pub(crate) fn jar(files: &[(&str, &str)]) -> Vec<u8> {
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for (name, content) in files {
            zip.start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(content.as_bytes()).unwrap();
        }
        zip.finish().unwrap().into_inner()
    }

    fn info(id: &str, name: &str, version: &str) -> JarInfo {
        JarInfo {
            id: Some(id.into()),
            name: Some(name.into()),
            version: Some(version.into()),
        }
    }

    #[test]
    fn reads_fabric_and_quilt_descriptors() {
        let fabric = jar(&[(
            "fabric.mod.json",
            r#"{"schemaVersion":1,"id":"lithium","name":"Lithium","version":"0.26.3+mc26.3"}"#,
        )]);
        assert_eq!(
            read(Cursor::new(fabric)),
            info("lithium", "Lithium", "0.26.3+mc26.3")
        );
        let quilt = jar(&[(
            "quilt.mod.json",
            r#"{"quilt_loader":{"id":"qsl","version":"9.0","metadata":{"name":"QSL"}}}"#,
        )]);
        assert_eq!(read(Cursor::new(quilt)), info("qsl", "QSL", "9.0"));
    }

    #[test]
    fn reads_forge_and_neoforge_descriptors_with_the_manifest_version() {
        let neoforge = jar(&[
            (
                "META-INF/neoforge.mods.toml",
                "modLoader = \"javafml\"\n[[mods]]\nmodId = \"jei\"\ndisplayName = \"Just Enough Items\"\nversion = \"${file.jarVersion}\"\n",
            ),
            (
                "META-INF/MANIFEST.MF",
                "Manifest-Version: 1.0\r\nImplementation-Version: 26.3.0.12\r\n",
            ),
        ]);
        assert_eq!(
            read(Cursor::new(neoforge)),
            info("jei", "Just Enough Items", "26.3.0.12")
        );
        let forge = jar(&[(
            "META-INF/mods.toml",
            "[[mods]]\nmodId=\"create\"\ndisplayName=\"Create\"\nversion=\"6.0\"\n",
        )]);
        assert_eq!(read(Cursor::new(forge)), info("create", "Create", "6.0"));
    }

    #[test]
    fn reads_plugin_descriptors() {
        let paper = jar(&[(
            "plugin.yml",
            "name: Chunky\nversion: '1.4.40'\nmain: org.popcraft.chunky.ChunkyBukkit\napi-version: 1.21\n",
        )]);
        assert_eq!(read(Cursor::new(paper)), info("Chunky", "Chunky", "1.4.40"));
        let nested = jar(&[(
            "paper-plugin.yml",
            "name: \"Squaremap\"\ndependencies:\n  server:\n    version: x\nversion: 1.3\n",
        )]);
        assert_eq!(
            read(Cursor::new(nested)).version.as_deref(),
            Some("1.3"),
            "only top-level keys"
        );
    }

    #[test]
    fn garbage_gives_nothing_rather_than_an_error() {
        assert_eq!(read(Cursor::new(b"not a zip".to_vec())), JarInfo::default());
        assert_eq!(
            read(Cursor::new(jar(&[("fabric.mod.json", "{ broken")]))),
            JarInfo::default()
        );
        assert_eq!(read(Cursor::new(jar(&[("README.md", "hi")]))), JarInfo::default());
    }
}
