//! Recognising an existing server folder: which loader, which versions, which jar starts it.
//!
//! Only reads (names, `version.json` inside the server jar); the user confirms or corrects the
//! result before anything is imported.

use std::{fs, io::Read, path::Path};

use serde::Serialize;

use crate::{catalog::neoforge, loader::LoaderKind};

/// What a folder looks like. Every field may be unknown.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Detected {
    pub loader: Option<LoaderKind>,
    pub loader_version: Option<String>,
    pub mc_version: Option<String>,
    /// The jar that starts it (for jar-launched loaders).
    pub jar: Option<String>,
    /// Why the guess was made, or what is unclear (shown to the user).
    pub notes: Vec<String>,
}

/// Inspects `dir` (an existing server folder).
pub fn detect(dir: &Path) -> Detected {
    let jars = top_level_jars(dir);
    let mut detected = detect_loader(dir, &jars);
    if detected.mc_version.is_none() {
        detected.mc_version = jars.iter().find_map(|jar| version_in_jar(&dir.join(jar)));
    }
    if detected.loader.is_none() {
        detected
            .notes
            .push("No server software recognised; choose it below.".into());
    }
    if detected.mc_version.is_none() {
        detected
            .notes
            .push("The Minecraft version could not be read; choose it below.".into());
    }
    detected
}

fn detect_loader(dir: &Path, jars: &[String]) -> Detected {
    if let Some(version) = only_subdir(&dir.join("libraries/net/neoforged/neoforge")) {
        return Detected {
            loader: Some(LoaderKind::NeoForge),
            mc_version: neoforge::minecraft_version(&version),
            loader_version: Some(version),
            notes: vec!["Found NeoForge libraries.".into()],
            ..Detected::default()
        };
    }
    if let Some(maven) = only_subdir(&dir.join("libraries/net/minecraftforge/forge")) {
        let (mc, version) = maven
            .split_once('-')
            .map_or((None, Some(maven.clone())), |(mc, v)| {
                (Some(mc.to_owned()), Some(v.to_owned()))
            });
        return Detected {
            loader: Some(LoaderKind::Forge),
            mc_version: mc,
            loader_version: version,
            notes: vec!["Found Forge libraries.".into()],
            ..Detected::default()
        };
    }
    let has = |name: &str| dir.join(name).exists();
    let by_launcher = [
        ("quilt-server-launch.jar", LoaderKind::Quilt),
        ("fabric-server-launch.jar", LoaderKind::Fabric),
    ];
    for (launcher, loader) in by_launcher {
        if has(launcher) {
            return Detected {
                loader: Some(loader),
                jar: Some(launcher.into()),
                notes: vec![format!("Found {launcher}.")],
                ..Detected::default()
            };
        }
    }
    let jar = main_jar(jars);
    let (loader, note) = if has("purpur.yml") || jars.iter().any(|j| j.starts_with("purpur")) {
        (Some(LoaderKind::Purpur), "Found Purpur's configuration.")
    } else if has("config/paper-global.yml")
        || has("paper.yml")
        || has("bukkit.yml")
        || jars.iter().any(|j| j.starts_with("paper"))
    {
        (Some(LoaderKind::Paper), "Found Paper's configuration.")
    } else if jar.is_some() {
        (
            Some(LoaderKind::Vanilla),
            "Found a server jar and no mod or plugin loader.",
        )
    } else {
        (None, "")
    };
    Detected {
        loader,
        jar,
        notes: if note.is_empty() {
            vec![]
        } else {
            vec![note.into()]
        },
        ..Detected::default()
    }
}

/// The jar to launch a vanilla-style server with: `server.jar`, else the only jar there is.
fn main_jar(jars: &[String]) -> Option<String> {
    if jars.iter().any(|jar| jar == "server.jar") {
        return Some("server.jar".into());
    }
    match jars {
        [only] => Some(only.clone()),
        _ => None,
    }
}

fn top_level_jars(dir: &Path) -> Vec<String> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut jars: Vec<String> = entries
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|t| t.is_file()))
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| {
            Path::new(name)
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("jar"))
        })
        .collect();
    jars.sort();
    jars
}

/// The single folder inside `dir` (a library's version folder), if there is exactly one.
fn only_subdir(dir: &Path) -> Option<String> {
    let mut dirs: Vec<String> = fs::read_dir(dir)
        .ok()?
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|t| t.is_dir()))
        .filter_map(|entry| entry.file_name().into_string().ok())
        .collect();
    match dirs.len() {
        1 => dirs.pop(),
        _ => None,
    }
}

/// Mojang's server jars (and Paper's) carry `version.json` with the Minecraft version.
fn version_in_jar(jar: &Path) -> Option<String> {
    let file = fs::File::open(jar).ok()?;
    let mut archive = zip::ZipArchive::new(file).ok()?;
    let mut entry = archive.by_name("version.json").ok()?;
    let mut text = String::new();
    entry.by_ref().take(64 * 1024).read_to_string(&mut text).ok()?;
    let json: serde_json::Value = serde_json::from_str(&text).ok()?;
    json["id"]
        .as_str()
        .or_else(|| json["name"].as_str())
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    fn folder(files: &[&str]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for file in files {
            let path = dir.path().join(file);
            if file.ends_with('/') {
                fs::create_dir_all(&path).unwrap();
            } else {
                fs::create_dir_all(path.parent().unwrap()).unwrap();
                fs::write(path, "").unwrap();
            }
        }
        dir
    }

    fn jar_with_version(path: &Path, version_json: &str) {
        let mut zip = zip::ZipWriter::new(fs::File::create(path).unwrap());
        zip.start_file("version.json", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(version_json.as_bytes()).unwrap();
        zip.finish().unwrap();
    }

    #[test]
    fn recognises_neoforge_and_forge_by_their_libraries() {
        let neo = detect(
            folder(&[
                "libraries/net/neoforged/neoforge/26.3.0.101/",
                "user_jvm_args.txt",
            ])
            .path(),
        );
        assert_eq!(neo.loader, Some(LoaderKind::NeoForge));
        assert_eq!(neo.loader_version.as_deref(), Some("26.3.0.101"));
        assert_eq!(neo.mc_version.as_deref(), Some("26.3"));

        let forge = detect(folder(&["libraries/net/minecraftforge/forge/1.20.1-47.3.0/"]).path());
        assert_eq!(forge.loader, Some(LoaderKind::Forge));
        assert_eq!(
            (forge.mc_version.as_deref(), forge.loader_version.as_deref()),
            (Some("1.20.1"), Some("47.3.0"))
        );
    }

    #[test]
    fn recognises_launcher_jars_and_plugin_servers() {
        let quilt = detect(folder(&["quilt-server-launch.jar", "server.jar"]).path());
        assert_eq!(
            (quilt.loader, quilt.jar.as_deref()),
            (Some(LoaderKind::Quilt), Some("quilt-server-launch.jar"))
        );
        let fabric = detect(folder(&["fabric-server-launch.jar"]).path());
        assert_eq!(fabric.loader, Some(LoaderKind::Fabric));
        let purpur = detect(folder(&["purpur-26.3-2657.jar", "purpur.yml", "bukkit.yml"]).path());
        assert_eq!(
            (purpur.loader, purpur.jar.as_deref()),
            (Some(LoaderKind::Purpur), Some("purpur-26.3-2657.jar"))
        );
        let paper = detect(folder(&["paper.jar", "config/paper-global.yml"]).path());
        assert_eq!(
            (paper.loader, paper.jar.as_deref()),
            (Some(LoaderKind::Paper), Some("paper.jar"))
        );
        let vanilla = detect(folder(&["server.jar", "eula.txt"]).path());
        assert_eq!(vanilla.loader, Some(LoaderKind::Vanilla));
    }

    #[test]
    fn reads_the_minecraft_version_from_the_server_jar() {
        let dir = folder(&["eula.txt"]);
        jar_with_version(
            &dir.path().join("server.jar"),
            r#"{"id":"26.3","name":"26.3","java_version":25}"#,
        );
        let detected = detect(dir.path());
        assert_eq!(detected.mc_version.as_deref(), Some("26.3"));
        assert!(!detected.notes.iter().any(|n| n.contains("could not be read")));
    }

    #[test]
    fn says_what_it_could_not_tell() {
        let empty = detect(folder(&["world/level.dat"]).path());
        assert_eq!(empty.loader, None);
        assert_eq!(empty.mc_version, None);
        assert_eq!(empty.notes.len(), 2, "{:?}", empty.notes);

        let ambiguous = detect(folder(&["a.jar", "b.jar"]).path());
        assert_eq!(
            ambiguous.jar, None,
            "two jars and no server.jar: the user must choose"
        );
        let not_a_zip = detect(folder(&["server.jar"]).path());
        assert_eq!(not_a_zip.mc_version, None, "an empty jar is not an error");
    }
}
