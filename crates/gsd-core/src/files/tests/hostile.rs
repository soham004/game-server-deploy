//! What a compromised server can put in its folder to hurt the app that reads it: a FIFO that never
//! answers, a file without end, a link where the app is about to write. Each must cost an error at
//! most, and never a held thread, the app's memory, or a file outside the folder.

#![cfg(unix)]

use std::{os::unix::fs::symlink, time::Duration};

use tokio::time::timeout;

use super::*;

/// Long enough for any of these to finish; a read held by a FIFO would never.
const SOON: Duration = Duration::from_secs(10);

fn make_fifo(path: &Path) {
    let made = std::process::Command::new("mkfifo").arg(path).status().unwrap();
    assert!(made.success());
}

/// A file that says it holds `bytes` and takes no room on disk.
fn make_sparse(path: &Path, bytes: u64) {
    std::fs::File::create(path).unwrap().set_len(bytes).unwrap();
}

#[tokio::test]
async fn a_fifo_is_refused_at_once_wherever_a_file_is_read() {
    let f = fixture().await;
    make_fifo(&f.server().join("server.properties"));
    f.write("world/level.dat", "level");
    make_fifo(&f.server().join("world/session.lock"));

    let reads = async {
        assert!(f.files.read_text(p("server.properties")).await.is_err());
        assert!(f.files.read_bytes(p("server.properties")).await.is_err());
        assert!(f.files.read_optional(p("server.properties"), 1024).await.is_err());
        assert!(f.files.open_file(p("server.properties")).await.is_err());
        assert!(
            f.files
                .copy(p("server.properties"), p("copy.properties"))
                .await
                .is_err()
        );
        let saved = f
            .files
            .write_text(p("server.properties"), "motd=x\n".into(), None)
            .await;
        assert!(saved.is_err(), "{saved:?}");
        assert!(
            f.files
                .extract(p("server.properties"), p("out"), ExtractLimits::default())
                .await
                .is_err()
        );
    };
    timeout(SOON, reads).await.expect("no read waits on the FIFO");

    // Folders are zipped and copied without it, and still measured.
    let zipped = async {
        f.files.compress(vec![p("world")], p("world.zip")).await.unwrap();
        f.files.copy(p("world"), p("world-copy")).await.unwrap();
        f.files.folder_size(p("world")).await.unwrap()
    };
    assert_eq!(timeout(SOON, zipped).await.expect("no FIFO is opened"), 5);
    let file = std::fs::File::open(f.server().join("world.zip")).unwrap();
    let archive = zip::ZipArchive::new(file).unwrap();
    let names: Vec<String> = archive
        .file_names()
        .map(|name| name.unwrap().into_owned())
        .collect();
    assert!(names.iter().any(|name| name == "world/level.dat"), "{names:?}");
    assert!(
        !names.iter().any(|name| name.contains("session.lock")),
        "{names:?}"
    );
    assert!(!f.server().join("world-copy/session.lock").exists());
}

#[tokio::test]
async fn an_archive_is_not_unpacked_into_a_fifo() {
    let f = fixture().await;
    std::fs::create_dir(f.server().join("out")).unwrap();
    make_fifo(&f.server().join("out/level.dat"));
    std::fs::write(
        f.server().join("world.zip"),
        zip_with(&[("level.dat", Some(b"level"))]),
    )
    .unwrap();
    let unpacked = timeout(
        SOON,
        f.files
            .extract(p("world.zip"), p("out"), ExtractLimits::default()),
    )
    .await
    .expect("writing doesn't wait for a reader either");
    assert!(unpacked.is_err(), "{unpacked:?}");
}

#[tokio::test]
async fn a_file_larger_than_the_limit_is_refused_without_being_read() {
    let f = fixture().await;
    // 64 GiB: reading it whole would end the app.
    make_sparse(&f.server().join("server.properties"), 64 << 30);
    symlink("/dev/zero", f.server().join("endless.txt")).unwrap();

    assert!(matches!(
        f.files.read_text(p("server.properties")).await,
        Err(FilesError::TooLarge { .. })
    ));
    assert!(matches!(
        f.files.read_optional(p("server.properties"), 4096).await,
        Err(FilesError::TooLarge { .. })
    ));
    // Saving used to read the old file whole, to compare it with what the editor had opened.
    let over_it = f
        .files
        .write_text(p("server.properties"), "motd=x\n".into(), Some(revision(b"")))
        .await;
    assert!(matches!(over_it, Err(FilesError::Conflict(_))), "{over_it:?}");
    let as_new = f
        .files
        .write_text(p("server.properties"), "motd=x\n".into(), None)
        .await;
    assert!(matches!(as_new, Err(FilesError::AlreadyExists(_))), "{as_new:?}");
    // A link to a device is a link out of the folder: refused like any other.
    let endless = timeout(SOON, f.files.read_text(p("endless.txt"))).await.unwrap();
    assert!(endless.is_err(), "{endless:?}");
}

#[tokio::test]
async fn reading_a_setting_stops_at_the_limit_given() {
    let f = fixture().await;
    f.write("server.properties", "level-name=world\n");
    assert_eq!(
        f.files
            .read_optional(p("server.properties"), 1024)
            .await
            .unwrap()
            .as_deref(),
        Some(b"level-name=world\n".as_slice())
    );
    assert!(matches!(
        f.files.read_optional(p("server.properties"), 4).await,
        Err(FilesError::TooLarge { .. })
    ));
    assert_eq!(f.files.read_optional(p("missing"), 1024).await.unwrap(), None);
    assert_eq!(f.files.read_optional(p("world"), 1024).await.unwrap(), None);
    std::fs::create_dir(f.server().join("world")).unwrap();
    assert!(matches!(
        f.files.read_optional(p("world"), 1024).await,
        Err(FilesError::IsADirectory(_))
    ));

    let server = f.server();
    assert_eq!(
        read_small_file(&server, "server.properties", 1024).as_deref(),
        Some(b"level-name=world\n".as_slice())
    );
    assert_eq!(read_small_file(&server, "server.properties", 4), None);
    assert_eq!(read_small_file(&server, "missing", 1024), None);
    symlink(f.outside.join("secret.txt"), server.join("link.txt")).unwrap();
    assert_eq!(read_small_file(&server, "link.txt", 1024), None);
}

#[tokio::test]
async fn a_whole_file_replaces_a_link_and_never_goes_through_it() {
    let f = fixture().await;
    let secret = f.outside.join("secret.txt");
    symlink(&secret, f.server().join("eula.txt")).unwrap();
    symlink(f.outside.join("not-there-yet"), f.server().join("dangling.txt")).unwrap();

    f.files
        .replace_file(p("eula.txt"), b"eula=true\n".to_vec())
        .await
        .unwrap();
    f.files
        .replace_file(p("dangling.txt"), b"x".to_vec())
        .await
        .unwrap();

    assert_eq!(std::fs::read_to_string(&secret).unwrap(), "the database password");
    assert!(!f.outside.join("not-there-yet").exists());
    for name in ["eula.txt", "dangling.txt"] {
        let kind = f.files.kind(p(name)).await.unwrap();
        assert_eq!(kind, Some(EntryKind::File), "{name} is the file now");
    }
    assert_eq!(
        std::fs::read_to_string(f.server().join("eula.txt")).unwrap(),
        "eula=true\n"
    );
    let left: Vec<_> = std::fs::read_dir(f.server())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .filter(|name| name.contains("gsd-tmp"))
        .collect();
    assert!(left.is_empty(), "{left:?}");
}

#[tokio::test]
async fn kinds_are_told_without_following_links() {
    let f = fixture().await;
    f.write("world/level.dat", "x");
    symlink(f.server().join("world"), f.server().join("link")).unwrap();
    make_fifo(&f.server().join("pipe"));
    for (path, kind) in [
        ("world", Some(EntryKind::Directory)),
        ("world/level.dat", Some(EntryKind::File)),
        ("link", Some(EntryKind::Symlink)),
        ("pipe", Some(EntryKind::Other)),
        ("missing", None),
    ] {
        assert_eq!(f.files.kind(p(path)).await.unwrap(), kind, "{path}");
    }
}

/// `open_folder` is how installs reach a server folder they only have the path of.
#[tokio::test]
async fn a_folder_opened_by_path_follows_the_same_rules() {
    let f = fixture().await;
    let files = ServerFiles::open_folder(f.server(), f.layout.trash_dir(), sample().sandbox)
        .await
        .unwrap();
    symlink(f.outside.join("secret.txt"), f.server().join("leak.txt")).unwrap();
    assert!(files.read_text(p("leak.txt")).await.is_err());
    files.write_text(p("ok.txt"), "fine".into(), None).await.unwrap();
    assert_eq!(
        std::fs::read_to_string(f.server().join("ok.txt")).unwrap(),
        "fine"
    );
}
