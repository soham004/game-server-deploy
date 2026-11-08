#![allow(clippy::unwrap_used)]

use std::{collections::BTreeMap, io::Read, path::Path};

use chrono::Utc;

use super::*;
use crate::{
    backups::model::{BackupKind, BackupScope},
    ids::{BackupId, InstanceId},
};

fn manifest() -> Manifest {
    Manifest {
        format: Manifest::FORMAT,
        backup_id: BackupId::generate(),
        instance_id: InstanceId::generate(),
        instance_name: "Survival".into(),
        game: "minecraft".into(),
        software: Some("Paper 26.3".into()),
        kind: BackupKind::Manual,
        scope: BackupScope::Full,
        created_at: Utc::now(),
        note: None,
        entries: vec![],
        files: 0,
        folders: 0,
        bytes: 0,
    }
}

/// A server folder: a world with a region file and an empty folder, config, logs, a library.
fn server() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    for (path, contents) in [
        ("world/level.dat", "level"),
        ("world/region/r.0.0.mca", "region data"),
        ("server.properties", "level-name=world\n"),
        ("logs/latest.log", "log"),
        ("libraries/lib.jar", "jar"),
    ] {
        std::fs::create_dir_all(root.join(path).parent().unwrap()).unwrap();
        std::fs::write(root.join(path), contents).unwrap();
    }
    std::fs::create_dir_all(root.join("world/data")).unwrap();
    dir
}

/// Every entry of a zip, with the contents of files (`None` for folders).
fn read_zip(path: &Path) -> BTreeMap<String, Option<String>> {
    let mut zip = zip::ZipArchive::new(File::open(path).unwrap()).unwrap();
    (0..zip.len())
        .map(|index| {
            let mut entry = zip.by_index(index).unwrap();
            let name = entry.name().unwrap().into_owned();
            if entry.is_dir() {
                (name, None)
            } else {
                let mut text = String::new();
                entry.read_to_string(&mut text).unwrap();
                (name, Some(text))
            }
        })
        .collect()
}

fn full() -> Selection {
    Selection::AllExcept(vec!["logs".into(), "libraries".into()])
}

#[test]
fn a_full_backup_holds_everything_but_the_exclusions_under_server() {
    let server = server();
    let out = tempfile::tempdir().unwrap();
    let path = out.path().join("backup.zip");

    let written = write_archive(
        server.path(),
        &full(),
        &path,
        manifest(),
        &CancellationToken::new(),
    )
    .unwrap();

    let entries = read_zip(&path);
    let names: Vec<&str> = entries.keys().map(String::as_str).collect();
    assert_eq!(
        names,
        [
            "gsd-backup.json",
            "server/server.properties",
            "server/world/",
            "server/world/data/",
            "server/world/level.dat",
            "server/world/region/",
            "server/world/region/r.0.0.mca",
        ]
    );
    assert_eq!(
        entries["server/world/region/r.0.0.mca"].as_deref(),
        Some("region data")
    );
    assert_eq!(written.manifest.entries, ["server.properties", "world"]);
    assert_eq!(
        (
            written.manifest.files,
            written.manifest.folders,
            written.manifest.bytes
        ),
        (3, 3, 33)
    );
}

#[test]
fn the_recorded_size_and_checksum_are_the_files_and_the_manifest_reads_back() {
    let server = server();
    let out = tempfile::tempdir().unwrap();
    let path = out.path().join("backup.zip");

    let written = write_archive(
        server.path(),
        &full(),
        &path,
        manifest(),
        &CancellationToken::new(),
    )
    .unwrap();

    assert_eq!(written.size_bytes, std::fs::metadata(&path).unwrap().len());
    assert_eq!(written.sha256, sha256_of(&path).unwrap());
    assert_eq!(read_manifest(&path).unwrap(), written.manifest);
}

#[test]
fn a_world_backup_holds_only_the_named_folders_that_exist() {
    let server = server();
    let out = tempfile::tempdir().unwrap();
    let path = out.path().join("world.zip");
    let worlds = Selection::Only(vec!["world_nether".into(), "world".into()]);

    let written = write_archive(
        server.path(),
        &worlds,
        &path,
        manifest(),
        &CancellationToken::new(),
    )
    .unwrap();

    assert_eq!(written.manifest.entries, ["world"]);
    assert!(
        read_zip(&path)
            .keys()
            .all(|name| name == "gsd-backup.json" || name.starts_with("server/world/"))
    );
}

#[cfg(unix)]
#[test]
fn links_planted_by_the_server_are_left_out() {
    let server = server();
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("secret.txt"), "secret").unwrap();
    std::os::unix::fs::symlink(outside.path(), server.path().join("escape")).unwrap();
    std::os::unix::fs::symlink(
        outside.path().join("secret.txt"),
        server.path().join("world/secret"),
    )
    .unwrap();
    let out = tempfile::tempdir().unwrap();
    let path = out.path().join("backup.zip");

    let written = write_archive(
        server.path(),
        &full(),
        &path,
        manifest(),
        &CancellationToken::new(),
    )
    .unwrap();

    let entries = read_zip(&path);
    assert!(
        !entries
            .keys()
            .any(|name| name.contains("escape") || name.contains("secret"))
    );
    assert!(!entries.values().any(|text| text.as_deref() == Some("secret")));
    assert!(!written.manifest.entries.contains(&"escape".to_owned()));
}

#[test]
fn a_cancelled_backup_leaves_no_file() {
    let server = server();
    let out = tempfile::tempdir().unwrap();
    let path = out.path().join("backup.zip");
    let cancel = CancellationToken::new();
    cancel.cancel();

    let error = write_archive(server.path(), &full(), &path, manifest(), &cancel).unwrap_err();

    assert_eq!(error.kind(), io::ErrorKind::Interrupted);
    assert!(!path.exists());
}

#[test]
fn an_existing_file_is_never_overwritten() {
    let server = server();
    let out = tempfile::tempdir().unwrap();
    let path = out.path().join("backup.zip");
    std::fs::write(&path, "an older backup").unwrap();

    let error = write_archive(
        server.path(),
        &full(),
        &path,
        manifest(),
        &CancellationToken::new(),
    )
    .unwrap_err();

    assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "an older backup");
}

#[test]
fn measures_only_what_would_be_archived() {
    let server = server();
    assert_eq!(measure(server.path(), &full()).unwrap(), 33);
    assert_eq!(
        measure(server.path(), &Selection::Only(vec!["world".into()])).unwrap(),
        16
    );
}

#[test]
fn a_zip_without_a_manifest_is_not_a_backup() {
    let out = tempfile::tempdir().unwrap();
    let path = out.path().join("other.zip");
    let mut zip = ZipWriter::new(File::create(&path).unwrap());
    zip.start_file("readme.txt", SimpleFileOptions::default())
        .unwrap();
    zip.write_all(b"hi").unwrap();
    zip.finish().unwrap();

    let error = read_manifest(&path).unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    assert!(error.to_string().contains("not a backup"), "{error}");
}
