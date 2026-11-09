//! What a modpack archive says about itself: Modrinth's `.mrpack` (`modrinth.index.json`),
//! `CurseForge`'s pack (`manifest.json`), or neither, which makes it a plain server folder in a zip.
//!
//! Everything here comes from an uploaded file, so it is untrusted: paths are checked when used,
//! and the downloads an `.mrpack` names are limited to the hosts its format allows.

use std::{collections::HashMap, io::Read};

use gsd_minecraft::LoaderKind;
use serde::{Deserialize, Serialize};

/// The largest index file read (the biggest real packs have a few thousand entries).
const MAX_INDEX_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum PackError {
    #[error("that isn't a zip archive")]
    NotAnArchive,
    #[error("this modpack can't be read: {0}")]
    Invalid(String),
    #[error("this modpack needs {0}, which isn't supported")]
    UnsupportedLoader(String),
    #[error("this modpack is for another game")]
    OtherGame,
}

/// What kind of archive it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PackFormat {
    Mrpack,
    Curseforge,
}

/// What a pack needs and brings, to confirm before creating a server for it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PackInfo {
    pub format: PackFormat,
    pub name: String,
    pub version: Option<String>,
    pub mc_version: String,
    pub loader: LoaderKind,
    pub loader_version: Option<String>,
    /// How many files it downloads for a server.
    pub files: usize,
}

/// A file an `.mrpack` downloads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MrpackFile {
    pub path: String,
    pub sha1: Option<String>,
    pub sha512: Option<String>,
    pub downloads: Vec<String>,
    pub size: u64,
}

/// A file a `CurseForge` pack names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CurseForgeFile {
    pub project: u64,
    pub file: u64,
}

/// What to fetch, and which folders of the archive to copy on top.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PackContents {
    Mrpack(Vec<MrpackFile>),
    Curseforge(Vec<CurseForgeFile>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pack {
    pub info: PackInfo,
    pub contents: PackContents,
    /// Folders in the archive whose contents go into the server, in order (later ones win).
    pub overrides: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MrpackIndex {
    game: String,
    name: String,
    #[serde(default)]
    version_id: Option<String>,
    files: Vec<MrpackEntry>,
    dependencies: HashMap<String, String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MrpackEntry {
    path: String,
    #[serde(default)]
    hashes: HashMap<String, String>,
    #[serde(default)]
    env: Option<MrpackEnv>,
    #[serde(default)]
    downloads: Vec<String>,
    #[serde(default)]
    file_size: u64,
}

#[derive(Debug, Deserialize)]
struct MrpackEnv {
    #[serde(default)]
    server: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CurseForgeManifest {
    minecraft: ManifestMinecraft,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    files: Vec<ManifestFile>,
    #[serde(default)]
    overrides: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ManifestMinecraft {
    version: String,
    #[serde(default)]
    mod_loaders: Vec<ManifestLoader>,
}

#[derive(Debug, Deserialize)]
struct ManifestLoader {
    id: String,
    #[serde(default)]
    primary: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ManifestFile {
    #[serde(rename = "projectID")]
    project_id: u64,
    #[serde(rename = "fileID")]
    file_id: u64,
    #[serde(default = "required_by_default")]
    required: bool,
}

fn required_by_default() -> bool {
    true
}

/// Reads a pack from an archive; `Ok(None)` for a zip that is neither kind of pack.
pub fn read<R: Read + std::io::Seek>(archive: R) -> Result<Option<Pack>, PackError> {
    let mut archive = zip::ZipArchive::new(archive).map_err(|_| PackError::NotAnArchive)?;
    if let Some(text) = entry_text(&mut archive, "modrinth.index.json")? {
        return read_mrpack(&text).map(Some);
    }
    if let Some(text) = entry_text(&mut archive, "manifest.json")? {
        return read_manifest(&text).map(Some);
    }
    Ok(None)
}

fn entry_text<R: Read + std::io::Seek>(
    archive: &mut zip::ZipArchive<R>,
    name: &str,
) -> Result<Option<String>, PackError> {
    let entry = match archive.by_name(name) {
        Ok(entry) => entry,
        Err(zip::result::ZipError::FileNotFound) => return Ok(None),
        Err(error) => return Err(PackError::Invalid(error.to_string())),
    };
    let mut text = String::new();
    entry
        .take(MAX_INDEX_BYTES)
        .read_to_string(&mut text)
        .map_err(|error| PackError::Invalid(format!("{name}: {error}")))?;
    Ok(Some(text))
}

fn read_mrpack(text: &str) -> Result<Pack, PackError> {
    let index: MrpackIndex = serde_json::from_str(text)
        .map_err(|error| PackError::Invalid(format!("modrinth.index.json: {error}")))?;
    if index.game != "minecraft" {
        return Err(PackError::OtherGame);
    }
    let mc_version = index
        .dependencies
        .get("minecraft")
        .cloned()
        .ok_or_else(|| PackError::Invalid("it doesn't say which Minecraft it is for".into()))?;
    let (loader, loader_version) = mrpack_loader(&index.dependencies)?;
    // A file with no opinion on servers goes in: leaving out a mod a server needs is worse.
    let files: Vec<MrpackFile> = index
        .files
        .into_iter()
        .filter(|file| {
            file.env
                .as_ref()
                .and_then(|env| env.server.as_deref())
                .is_none_or(|server| server != "unsupported")
        })
        .map(|file| MrpackFile {
            sha1: file.hashes.get("sha1").cloned(),
            sha512: file.hashes.get("sha512").cloned(),
            path: file.path,
            downloads: file.downloads,
            size: file.file_size,
        })
        .collect();
    Ok(Pack {
        info: PackInfo {
            format: PackFormat::Mrpack,
            name: index.name,
            version: index.version_id,
            mc_version,
            loader,
            loader_version,
            files: files.len(),
        },
        contents: PackContents::Mrpack(files),
        overrides: vec!["overrides".into(), "server-overrides".into()],
    })
}

fn mrpack_loader(dependencies: &HashMap<String, String>) -> Result<(LoaderKind, Option<String>), PackError> {
    let known = [
        ("fabric-loader", LoaderKind::Fabric),
        ("quilt-loader", LoaderKind::Quilt),
        ("forge", LoaderKind::Forge),
        ("neoforge", LoaderKind::NeoForge),
    ];
    for (name, loader) in known {
        if let Some(version) = dependencies.get(name) {
            return Ok((loader, Some(version.clone())));
        }
    }
    match dependencies.keys().find(|name| *name != "minecraft") {
        Some(other) => Err(PackError::UnsupportedLoader(other.clone())),
        None => Ok((LoaderKind::Vanilla, None)),
    }
}

fn read_manifest(text: &str) -> Result<Pack, PackError> {
    let manifest: CurseForgeManifest =
        serde_json::from_str(text).map_err(|error| PackError::Invalid(format!("manifest.json: {error}")))?;
    let loaders = &manifest.minecraft.mod_loaders;
    let primary = loaders.iter().find(|loader| loader.primary).or(loaders.first());
    let (loader, loader_version) = match primary {
        Some(loader) => manifest_loader(&loader.id)?,
        None => (LoaderKind::Vanilla, None),
    };
    let files: Vec<CurseForgeFile> = manifest
        .files
        .iter()
        .filter(|file| file.required)
        .map(|file| CurseForgeFile {
            project: file.project_id,
            file: file.file_id,
        })
        .collect();
    Ok(Pack {
        info: PackInfo {
            format: PackFormat::Curseforge,
            name: manifest.name.unwrap_or_else(|| "CurseForge pack".into()),
            version: manifest.version,
            mc_version: manifest.minecraft.version,
            loader,
            loader_version,
            files: files.len(),
        },
        contents: PackContents::Curseforge(files),
        overrides: vec![manifest.overrides.unwrap_or_else(|| "overrides".into())],
    })
}

/// `forge-47.2.0`, `fabric-0.15.7`, `neoforge-21.1.1`: the loader, and its version after the first dash.
fn manifest_loader(id: &str) -> Result<(LoaderKind, Option<String>), PackError> {
    let (name, version) = id.split_once('-').unwrap_or((id, ""));
    let loader = match name {
        "forge" => LoaderKind::Forge,
        "fabric" => LoaderKind::Fabric,
        "quilt" => LoaderKind::Quilt,
        "neoforge" => LoaderKind::NeoForge,
        other => return Err(PackError::UnsupportedLoader(other.to_owned())),
    };
    Ok((
        loader,
        Some(version.to_owned()).filter(|version| !version.is_empty()),
    ))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use std::io::{Cursor, Write};

    use serde_json::json;

    use super::*;

    fn zip_of(entries: &[(&str, String)]) -> Cursor<Vec<u8>> {
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for (name, text) in entries {
            zip.start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(text.as_bytes()).unwrap();
        }
        zip.finish().unwrap()
    }

    fn mrpack(dependencies: &serde_json::Value) -> Cursor<Vec<u8>> {
        let index = json!({
            "formatVersion": 1, "game": "minecraft", "versionId": "1.2", "name": "My Pack",
            "dependencies": dependencies,
            "files": [
                { "path": "mods/both.jar", "hashes": { "sha1": "a", "sha512": "b" }, "env": { "client": "required", "server": "required" }, "downloads": ["https://cdn.modrinth.com/x"], "fileSize": 3 },
                { "path": "mods/no-opinion.jar", "hashes": { "sha512": "c" }, "downloads": ["https://cdn.modrinth.com/y"] },
                { "path": "mods/client-only.jar", "hashes": { "sha512": "d" }, "env": { "client": "required", "server": "unsupported" }, "downloads": ["https://cdn.modrinth.com/z"] },
            ],
        });
        zip_of(&[("modrinth.index.json", index.to_string())])
    }

    #[test]
    fn an_mrpack_says_what_it_needs_and_leaves_out_client_only_files() {
        let pack = read(mrpack(&json!({ "minecraft": "26.3", "fabric-loader": "0.17.0" })))
            .unwrap()
            .unwrap();
        assert_eq!(pack.info.format, PackFormat::Mrpack);
        assert_eq!(pack.info.name, "My Pack");
        assert_eq!(pack.info.version.as_deref(), Some("1.2"));
        assert_eq!(pack.info.mc_version, "26.3");
        assert_eq!(pack.info.loader, LoaderKind::Fabric);
        assert_eq!(pack.info.loader_version.as_deref(), Some("0.17.0"));
        assert_eq!(pack.info.files, 2);
        assert_eq!(pack.overrides, ["overrides", "server-overrides"]);
        let PackContents::Mrpack(files) = pack.contents else {
            panic!("an mrpack");
        };
        let paths: Vec<&str> = files.iter().map(|file| file.path.as_str()).collect();
        assert_eq!(paths, ["mods/both.jar", "mods/no-opinion.jar"]);
    }

    #[test]
    fn every_loader_maps_and_an_unknown_one_is_named() {
        for (key, loader) in [
            ("quilt-loader", LoaderKind::Quilt),
            ("forge", LoaderKind::Forge),
            ("neoforge", LoaderKind::NeoForge),
        ] {
            let pack = read(mrpack(&json!({ "minecraft": "26.3", key: "1" })))
                .unwrap()
                .unwrap();
            assert_eq!(pack.info.loader, loader);
        }
        let vanilla = read(mrpack(&json!({ "minecraft": "26.3" }))).unwrap().unwrap();
        assert_eq!(vanilla.info.loader, LoaderKind::Vanilla);
        let unknown = read(mrpack(&json!({ "minecraft": "26.3", "rift": "1" }))).unwrap_err();
        assert!(unknown.to_string().contains("rift"), "{unknown}");
    }

    #[test]
    fn a_curseforge_manifest_keeps_only_required_files() {
        let manifest = json!({
            "minecraft": { "version": "26.3", "modLoaders": [{ "id": "forge-61.0.1", "primary": false }, { "id": "neoforge-21.1.1", "primary": true }] },
            "manifestType": "minecraftModpack", "name": "CF Pack", "version": "2.0", "overrides": "extras",
            "files": [{ "projectID": 1, "fileID": 2, "required": true }, { "projectID": 3, "fileID": 4, "required": false }],
        });
        let pack = read(zip_of(&[("manifest.json", manifest.to_string())]))
            .unwrap()
            .unwrap();
        assert_eq!(pack.info.format, PackFormat::Curseforge);
        assert_eq!(pack.info.loader, LoaderKind::NeoForge, "the primary one");
        assert_eq!(pack.info.loader_version.as_deref(), Some("21.1.1"));
        assert_eq!(pack.info.files, 1);
        assert_eq!(pack.overrides, ["extras"]);
        assert_eq!(
            pack.contents,
            PackContents::Curseforge(vec![CurseForgeFile { project: 1, file: 2 }])
        );
    }

    #[test]
    fn other_archives_and_broken_packs_are_told_apart() {
        assert_eq!(
            read(zip_of(&[("server.jar", "x".into())])).unwrap(),
            None,
            "a plain zip"
        );
        assert!(matches!(
            read(Cursor::new(b"not a zip".to_vec())),
            Err(PackError::NotAnArchive)
        ));
        let broken = zip_of(&[("modrinth.index.json", "{".into())]);
        assert!(matches!(read(broken), Err(PackError::Invalid(_))));
        let other = zip_of(&[(
            "modrinth.index.json",
            json!({ "game": "terraria", "name": "x", "files": [], "dependencies": {} }).to_string(),
        )]);
        assert!(matches!(read(other), Err(PackError::OtherGame)));
    }
}
