//! File manager behaviour, including the security boundary: nothing may read or write outside the
//! server folder, whatever paths, symlinks or archives the (possibly compromised) server prepares.

#![allow(clippy::unwrap_used)]

mod hostile;

use std::{io::Write, path::Path};

use bytes::Bytes;
use futures_util::{StreamExt, stream};

use super::*;
use crate::instances::{Instance, InstanceLayout, definition::tests::sample};

struct Fixture {
    files: ServerFiles,
    layout: InstanceLayout,
    /// A directory next to the instance that must stay untouched (the symlink tests aim at it).
    #[cfg_attr(not(unix), allow(dead_code))]
    outside: std::path::PathBuf,
    _dir: tempfile::TempDir,
}

impl Fixture {
    fn server(&self) -> std::path::PathBuf {
        self.layout.server_dir()
    }

    fn write(&self, rel: &str, contents: &str) {
        let path = self.server().join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, contents).unwrap();
    }
}

async fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let layout = InstanceLayout::new(dir.path().join("instances/survival"));
    std::fs::create_dir_all(layout.server_dir()).unwrap();
    let outside = dir.path().join("outside");
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(outside.join("secret.txt"), "the database password").unwrap();
    let instance = Instance {
        definition: sample(),
        layout: layout.clone(),
    };
    Fixture {
        files: ServerFiles::open(&instance).await.unwrap(),
        layout,
        outside,
        _dir: dir,
    }
}

fn p(path: &str) -> SafeRelPath {
    SafeRelPath::parse(path).unwrap()
}

fn names(entries: &[Entry]) -> Vec<&str> {
    entries.iter().map(|e| e.name.as_str()).collect()
}

fn body(
    chunks: &[&'static [u8]],
) -> impl futures_util::Stream<Item = Result<Bytes, std::io::Error>> + Unpin + Send {
    stream::iter(
        chunks
            .iter()
            .map(|c| Ok(Bytes::from_static(c)))
            .collect::<Vec<_>>(),
    )
}

#[tokio::test]
async fn lists_folders_first_then_files_by_name() {
    let f = fixture().await;
    f.write("b.txt", "b");
    f.write("A.txt", "a");
    f.write("world/level.dat", "x");
    f.write("config/x.yml", "x");
    let entries = f.files.list(SafeRelPath::root()).await.unwrap();
    assert_eq!(names(&entries), ["config", "world", "A.txt", "b.txt"]);
    let file = entries.iter().find(|e| e.name == "A.txt").unwrap();
    assert_eq!((file.kind, file.size), (EntryKind::File, Some(1)));
    assert_eq!(file.path.as_str(), "A.txt");
    assert!(file.modified.is_some());
    let world = entries.iter().find(|e| e.name == "world").unwrap();
    assert_eq!((world.kind, world.size), (EntryKind::Directory, None));

    let inside = f.files.list(p("world")).await.unwrap();
    assert_eq!(inside[0].path.as_str(), "world/level.dat");
    assert!(matches!(
        f.files.list(p("missing")).await,
        Err(FilesError::NotFound(_))
    ));
    assert!(matches!(
        f.files.list(p("A.txt")).await,
        Err(FilesError::NotADirectory(_))
    ));
}

#[tokio::test]
async fn a_folders_size_counts_its_files_and_not_what_links_point_at() {
    let f = fixture().await;
    f.write("world/level.dat", "12345");
    f.write("world/region/r.0.0.mca", "1234567890");
    let mut deep = String::from("world/deep");
    for _ in 0..300 {
        deep.push_str("/d");
    }
    f.write(&format!("{deep}/x"), "1");
    #[cfg(unix)]
    std::os::unix::fs::symlink(&f.outside, f.server().join("world/elsewhere")).unwrap();
    assert_eq!(f.files.folder_size(p("world")).await.unwrap(), 16);
}

#[tokio::test]
async fn binary_files_are_read_and_saved_with_conflict_detection() {
    let f = fixture().await;
    std::fs::create_dir_all(f.server().join("world")).unwrap();
    std::fs::write(f.server().join("world/level.dat"), [0x1f, 0x8b, 0, 0xff]).unwrap();
    let (bytes, opened) = f.files.read_bytes(p("world/level.dat")).await.unwrap();
    assert_eq!(bytes, [0x1f, 0x8b, 0, 0xff]);
    f.files
        .write_bytes(p("world/level.dat"), vec![1, 2, 0, 3], Some(opened.clone()))
        .await
        .unwrap();
    assert_eq!(
        std::fs::read(f.server().join("world/level.dat")).unwrap(),
        [1, 2, 0, 3]
    );
    let stale = f
        .files
        .write_bytes(p("world/level.dat"), vec![9], Some(opened))
        .await;
    assert!(matches!(stale, Err(FilesError::Conflict(_))), "{stale:?}");
    assert_eq!(
        std::fs::read(f.server().join("world/level.dat")).unwrap(),
        [1, 2, 0, 3]
    );
}

#[tokio::test]
async fn reads_and_saves_text_with_conflict_detection() {
    let f = fixture().await;
    f.write("config/paper.yml", "a: 1\n");
    let opened = f.files.read_text(p("config/paper.yml")).await.unwrap();
    assert_eq!(opened.content, "a: 1\n");
    assert_eq!(opened.revision, revision(b"a: 1\n"));

    let saved = f
        .files
        .write_text(
            p("config/paper.yml"),
            "a: 2\n".into(),
            Some(opened.revision.clone()),
        )
        .await
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(f.server().join("config/paper.yml")).unwrap(),
        "a: 2\n"
    );
    assert_ne!(saved.revision, opened.revision);

    // Saving from the stale revision is refused and changes nothing.
    let stale = f
        .files
        .write_text(p("config/paper.yml"), "a: 3\n".into(), Some(opened.revision))
        .await;
    assert!(matches!(stale, Err(FilesError::Conflict(_))), "{stale:?}");
    assert_eq!(
        std::fs::read_to_string(f.server().join("config/paper.yml")).unwrap(),
        "a: 2\n"
    );

    // No revision = a new file, which never replaces an existing one.
    let blind = f.files.write_text(p("config/paper.yml"), "x".into(), None).await;
    assert!(matches!(blind, Err(FilesError::AlreadyExists(_))));
    f.files
        .write_text(p("config/new.yml"), "b: 1\n".into(), None)
        .await
        .unwrap();
    assert!(f.server().join("config/new.yml").is_file());
    let leftovers: Vec<_> = std::fs::read_dir(f.server().join("config"))
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .filter(|n| n.contains("gsd-tmp"))
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
}

#[tokio::test]
async fn refuses_binary_huge_and_folder_reads() {
    let f = fixture().await;
    std::fs::write(f.server().join("server.jar"), b"PK\x03\x04\x00\x00binary").unwrap();
    std::fs::write(f.server().join("latin1.txt"), b"caf\xe9").unwrap();
    std::fs::write(
        f.server().join("big.log"),
        vec![b'a'; usize::try_from(MAX_TEXT_BYTES).unwrap() + 1],
    )
    .unwrap();
    f.write("world/level.dat", "x");
    assert!(matches!(
        f.files.read_text(p("server.jar")).await,
        Err(FilesError::NotText(_))
    ));
    assert!(matches!(
        f.files.read_text(p("latin1.txt")).await,
        Err(FilesError::NotText(_))
    ));
    assert!(matches!(
        f.files.read_text(p("big.log")).await,
        Err(FilesError::TooLarge { .. })
    ));
    assert!(matches!(
        f.files.read_text(p("world")).await,
        Err(FilesError::IsADirectory(_))
    ));
    let too_big = "a".repeat(usize::try_from(MAX_TEXT_BYTES).unwrap() + 1);
    assert!(matches!(
        f.files.write_text(p("new.txt"), too_big, None).await,
        Err(FilesError::TooLarge { .. })
    ));
}

#[tokio::test]
async fn creates_renames_and_copies() {
    let f = fixture().await;
    f.write("plugins/a.jar", "jar");
    f.write("plugins/conf/a.yml", "conf");
    f.files.create_dir(p("backups")).await.unwrap();
    assert!(f.server().join("backups").is_dir());
    assert!(matches!(
        f.files.create_dir(p("backups")).await,
        Err(FilesError::AlreadyExists(_))
    ));

    f.files.copy(p("plugins"), p("plugins-copy")).await.unwrap();
    assert_eq!(
        std::fs::read_to_string(f.server().join("plugins-copy/conf/a.yml")).unwrap(),
        "conf"
    );
    f.files
        .rename(p("plugins-copy"), p("backups/plugins"))
        .await
        .unwrap();
    assert!(f.server().join("backups/plugins/a.jar").is_file());
    assert!(!f.server().join("plugins-copy").exists());

    let invalid = [
        (p("plugins"), p("plugins/inside")),
        (SafeRelPath::root(), p("x")),
        (p("plugins"), SafeRelPath::root()),
    ];
    for (from, to) in invalid {
        assert!(
            matches!(
                f.files.rename(from.clone(), to.clone()).await,
                Err(FilesError::InvalidOperation(_))
            ),
            "{from} → {to}"
        );
        assert!(
            matches!(
                f.files.copy(from.clone(), to.clone()).await,
                Err(FilesError::InvalidOperation(_))
            ),
            "{from} → {to}"
        );
    }
    assert!(matches!(
        f.files.rename(p("plugins"), p("backups")).await,
        Err(FilesError::AlreadyExists(_))
    ));
    assert!(matches!(
        f.files.copy(p("nope"), p("x")).await,
        Err(FilesError::NotFound(_))
    ));
}

#[tokio::test]
async fn deleting_moves_to_the_instance_trash_outside_the_server_folder() {
    let f = fixture().await;
    f.write("world/level.dat", "world");
    f.write("old.log", "log");
    f.files.delete(vec![p("world"), p("old.log")]).await.unwrap();
    assert!(!f.server().join("world").exists() && !f.server().join("old.log").exists());

    let trash: Vec<String> = std::fs::read_dir(f.layout.root().join("trash"))
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    assert_eq!(trash.len(), 2);
    assert!(trash.iter().any(|n| n.ends_with("-world")));
    // Deleting the same name again in the same second doesn't clash.
    f.write("old.log", "again");
    f.files.delete(vec![p("old.log")]).await.unwrap();
    assert_eq!(
        std::fs::read_dir(f.layout.root().join("trash")).unwrap().count(),
        3
    );

    assert!(matches!(
        f.files.delete(vec![SafeRelPath::root()]).await,
        Err(FilesError::InvalidOperation(_))
    ));
    assert!(matches!(
        f.files.delete(vec![p("missing")]).await,
        Err(FilesError::NotFound(_))
    ));
}

/// The server (or a malicious upload) can plant symlinks; following one out of the server folder
/// must fail for every operation, and the target must stay untouched.
#[cfg(unix)]
#[tokio::test]
async fn symlinks_never_lead_outside_the_server_folder() {
    let f = fixture().await;
    std::os::unix::fs::symlink(&f.outside, f.server().join("escape")).unwrap();
    std::os::unix::fs::symlink(f.outside.join("secret.txt"), f.server().join("secret-link")).unwrap();
    std::os::unix::fs::symlink("/", f.server().join("root-link")).unwrap();

    let listing = f.files.list(SafeRelPath::root()).await.unwrap();
    assert!(
        listing.iter().all(|e| e.kind == EntryKind::Symlink),
        "links are listed, not followed"
    );

    let forbidden =
        |r: Result<(), FilesError>| matches!(r, Err(FilesError::Forbidden(_) | FilesError::NotFound(_)));
    assert!(forbidden(f.files.list(p("escape")).await.map(drop)));
    assert!(forbidden(f.files.list(p("root-link/etc")).await.map(drop)));
    assert!(forbidden(f.files.read_text(p("secret-link")).await.map(drop)));
    assert!(forbidden(
        f.files.read_text(p("escape/secret.txt")).await.map(drop)
    ));
    assert!(forbidden(f.files.open_file(p("secret-link")).await.map(drop)));
    assert!(forbidden(
        f.files
            .write_text(p("escape/planted.txt"), "x".into(), None)
            .await
            .map(drop)
    ));
    assert!(forbidden(f.files.create_dir(p("escape/new")).await));
    assert!(forbidden(
        f.files
            .upload(p("escape/up.bin"), body(&[b"x"]), false, 10)
            .await
            .map(drop)
    ));
    f.write("ok.txt", "ok");
    assert!(forbidden(f.files.copy(p("ok.txt"), p("escape/copied.txt")).await));
    assert!(forbidden(
        f.files.rename(p("ok.txt"), p("escape/moved.txt")).await
    ));
    assert!(forbidden(
        f.files.compress(vec![p("ok.txt")], p("escape/out.zip")).await
    ));

    // Deleting a link removes the link itself, never its target.
    f.files.delete(vec![p("secret-link")]).await.unwrap();
    assert_eq!(
        std::fs::read_to_string(f.outside.join("secret.txt")).unwrap(),
        "the database password"
    );
    let outside: Vec<_> = std::fs::read_dir(&f.outside).unwrap().collect();
    assert_eq!(outside.len(), 1, "nothing was written outside");
}

#[cfg(unix)]
#[tokio::test]
async fn symlinks_inside_the_server_folder_still_work() {
    let f = fixture().await;
    f.write("real/config.yml", "inside");
    std::os::unix::fs::symlink("real", f.server().join("alias")).unwrap();
    assert_eq!(
        f.files.read_text(p("alias/config.yml")).await.unwrap().content,
        "inside"
    );
}

#[tokio::test]
async fn uploads_appear_only_when_complete_and_within_the_limit() {
    let f = fixture().await;
    f.files.create_dir(p("mods")).await.unwrap();
    let entry = f
        .files
        .upload(
            p("mods/sodium.jar"),
            body(&[b"PK", b"\x03\x04", b" rest"]),
            false,
            100,
        )
        .await
        .unwrap();
    assert_eq!((entry.kind, entry.size), (EntryKind::File, Some(9)));
    assert_eq!(
        std::fs::read(f.server().join("mods/sodium.jar")).unwrap(),
        b"PK\x03\x04 rest"
    );

    assert!(matches!(
        f.files
            .upload(p("mods/sodium.jar"), body(&[b"new"]), false, 100)
            .await,
        Err(FilesError::AlreadyExists(_))
    ));
    f.files
        .upload(p("mods/sodium.jar"), body(&[b"new"]), true, 100)
        .await
        .unwrap();
    assert_eq!(std::fs::read(f.server().join("mods/sodium.jar")).unwrap(), b"new");

    let too_big = f
        .files
        .upload(p("mods/huge.jar"), body(&[b"12345", b"67890"]), false, 8)
        .await;
    assert!(
        matches!(too_big, Err(FilesError::TooLarge { limit: 8, .. })),
        "{too_big:?}"
    );
    assert!(matches!(
        f.files.upload(p("missing/x.jar"), body(&[b"x"]), false, 10).await,
        Err(FilesError::NotFound(_))
    ));
    let broken = stream::iter(vec![
        Ok(Bytes::from_static(b"part")),
        Err(std::io::Error::other("connection reset")),
    ]);
    assert!(
        f.files
            .upload(p("mods/broken.jar"), broken, false, 100)
            .await
            .is_err()
    );

    let names: Vec<String> = std::fs::read_dir(f.server().join("mods"))
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    assert_eq!(names, ["sodium.jar"], "no partial or temporary files are left");
}

fn zip_with(entries: &[(&str, Option<&[u8]>)]) -> Vec<u8> {
    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default();
    for (name, contents) in entries {
        match contents {
            Some(bytes) => {
                zip.start_file(*name, options).unwrap();
                zip.write_all(bytes).unwrap();
            }
            None => zip.add_directory(*name, options).unwrap(),
        }
    }
    zip.finish().unwrap().into_inner()
}

#[tokio::test]
async fn extracts_zip_archives_and_skips_unsafe_entries() {
    let f = fixture().await;
    let mut archive = zip_with(&[
        ("pack/", None),
        ("pack/mods/a.jar", Some(b"a")),
        ("pack/config/b.toml", Some(b"b = 1")),
        ("../escaped.txt", Some(b"evil")),
        ("/absolute.txt", Some(b"evil")),
    ]);
    // A symlink entry pointing outside.
    let mut zip = zip::ZipWriter::new_append(std::io::Cursor::new(&mut archive)).unwrap();
    zip.add_symlink(
        "pack/link",
        "/etc/passwd",
        zip::write::SimpleFileOptions::default(),
    )
    .unwrap();
    zip.finish().unwrap();
    std::fs::write(f.server().join("server-pack.zip"), &archive).unwrap();

    let summary = f
        .files
        .extract(p("server-pack.zip"), p("unpacked"), ExtractLimits::default())
        .await
        .unwrap();
    assert_eq!((summary.files, summary.skipped), (3, 2), "{summary:?}");
    assert_eq!(
        std::fs::read_to_string(f.server().join("unpacked/pack/config/b.toml")).unwrap(),
        "b = 1"
    );
    // `/absolute.txt` is read as relative (the leading slash is dropped), so it stays inside.
    assert!(f.server().join("unpacked/absolute.txt").is_file());
    assert!(!f.layout.root().join("escaped.txt").exists() && !f.server().join("escaped.txt").exists());
    assert!(
        std::fs::symlink_metadata(f.server().join("unpacked/pack/link")).is_err(),
        "links are not extracted"
    );
}

#[tokio::test]
async fn zip_bombs_hit_the_limits() {
    let f = fixture().await;
    let archive = zip_with(&[("big.bin", Some(&[0_u8; 4096]))]);
    std::fs::write(f.server().join("bomb.zip"), archive).unwrap();
    let tight = ExtractLimits {
        max_bytes: 1000,
        max_entries: 100,
    };
    let result = f.files.extract(p("bomb.zip"), p("out"), tight).await;
    assert!(
        matches!(result, Err(FilesError::BadArchive(_, ref detail)) if detail.contains("1000 bytes")),
        "{result:?}"
    );

    let many: Vec<(String, Option<&[u8]>)> = (0..5).map(|i| (format!("f{i}.txt"), Some(&b"x"[..]))).collect();
    let refs: Vec<(&str, Option<&[u8]>)> = many.iter().map(|(n, c)| (n.as_str(), *c)).collect();
    std::fs::write(f.server().join("many.zip"), zip_with(&refs)).unwrap();
    let few = ExtractLimits {
        max_bytes: 1000,
        max_entries: 3,
    };
    assert!(matches!(
        f.files.extract(p("many.zip"), p("out2"), few).await,
        Err(FilesError::BadArchive(_, ref detail)) if detail.contains("3 entries")
    ));
}

#[tokio::test]
async fn extracts_tar_gz_and_skips_traversal_names() {
    let f = fixture().await;
    let mut builder = tar::Builder::new(flate2::write::GzEncoder::new(
        Vec::new(),
        flate2::Compression::fast(),
    ));
    let mut header = tar::Header::new_gnu();
    header.set_size(5);
    header.set_mode(0o644);
    header.set_cksum();
    builder
        .append_data(&mut header, "world/level.dat", &b"level"[..])
        .unwrap();
    // A hand-made traversal entry (the tar crate refuses to write `..` itself).
    let mut evil = tar::Header::new_old();
    evil.as_old_mut().name[..14].copy_from_slice(b"../../evil.txt");
    evil.set_size(4);
    evil.set_mode(0o644);
    evil.set_entry_type(tar::EntryType::Regular);
    evil.set_cksum();
    builder.append(&evil, &b"evil"[..]).unwrap();
    let gz = builder.into_inner().unwrap().finish().unwrap();
    std::fs::write(f.server().join("world.tar.gz"), gz).unwrap();

    let summary = f
        .files
        .extract(p("world.tar.gz"), SafeRelPath::root(), ExtractLimits::default())
        .await
        .unwrap();
    assert_eq!((summary.files, summary.skipped), (1, 1), "{summary:?}");
    assert_eq!(
        std::fs::read_to_string(f.server().join("world/level.dat")).unwrap(),
        "level"
    );
    assert!(!f.layout.root().parent().unwrap().join("evil.txt").exists());
    assert!(matches!(
        f.files
            .extract(
                p("world/level.dat"),
                SafeRelPath::root(),
                ExtractLimits::default()
            )
            .await,
        Err(FilesError::InvalidOperation(_))
    ));
}

#[tokio::test]
async fn compressing_and_downloading_a_folder_round_trips() {
    let f = fixture().await;
    f.write("world/level.dat", "level");
    f.write("world/region/r.0.0.mca", "region");
    f.write("server.properties", "motd=x");
    f.files
        .compress(vec![p("world"), p("server.properties")], p("backup.zip"))
        .await
        .unwrap();
    assert!(matches!(
        f.files.compress(vec![p("world")], p("backup.zip")).await,
        Err(FilesError::AlreadyExists(_))
    ));
    assert!(matches!(
        f.files.compress(vec![p("world")], p("backup.tar")).await,
        Err(FilesError::InvalidOperation(_))
    ));
    assert!(matches!(
        f.files.compress(vec![p("world")], p("world/inside.zip")).await,
        Err(FilesError::InvalidOperation(_))
    ));

    f.files
        .extract(p("backup.zip"), p("restored"), ExtractLimits::default())
        .await
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(f.server().join("restored/world/region/r.0.0.mca")).unwrap(),
        "region"
    );
    assert_eq!(
        std::fs::read_to_string(f.server().join("restored/server.properties")).unwrap(),
        "motd=x"
    );

    // The streamed zip of a folder is a valid archive with the same files.
    let bytes: Vec<u8> = f
        .files
        .zip_folder(p("world"))
        .map(|chunk| chunk.unwrap().to_vec())
        .concat()
        .await;
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
    let mut names: Vec<String> = archive
        .file_names()
        .map(|name| name.unwrap().into_owned())
        .collect();
    names.sort();
    assert_eq!(
        names,
        [
            "world/",
            "world/level.dat",
            "world/region/",
            "world/region/r.0.0.mca"
        ]
    );
    let mut level = String::new();
    std::io::Read::read_to_string(&mut archive.by_name("world/level.dat").unwrap(), &mut level).unwrap();
    assert_eq!(level, "level");
}

#[tokio::test]
async fn downloads_open_files_and_reject_folders() {
    let f = fixture().await;
    f.write("logs/latest.log", "hello");
    let (mut file, size) = f.files.open_file(p("logs/latest.log")).await.unwrap();
    assert_eq!(size, 5);
    let mut text = String::new();
    tokio::io::AsyncReadExt::read_to_string(&mut file, &mut text)
        .await
        .unwrap();
    assert_eq!(text, "hello");
    assert!(matches!(
        f.files.open_file(p("logs")).await,
        Err(FilesError::IsADirectory(_))
    ));
}

#[test]
fn the_fixture_layout_is_what_production_uses() {
    // The trash lives next to server/, not inside it.
    let layout = InstanceLayout::new(Path::new("/data/instances/x").to_owned());
    assert_eq!(layout.server_dir(), Path::new("/data/instances/x/server"));
}
