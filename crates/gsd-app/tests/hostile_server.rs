//! What a compromised server can leave in its own folder, against the app that works in it as
//! root: links where the app writes, links and files without end where it reads, FIFOs that never
//! answer. Through the API, the way the app is really used: none of it may cost more than an
//! error. Before these were fixed, the first test overwrote the file outside, the second showed
//! its contents, and the third never returned.

#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::{
    os::unix::fs::symlink,
    path::{Path, PathBuf},
    time::Duration,
};

use axum::{body::Bytes, http::StatusCode};
use common::{TestApp, json_body};
use futures_util::stream;
use gsd_minecraft::worlds::service::WorldError;
use serde_json::{Value, json};
use tokio::time::timeout;

/// Any request here answers at once unless something waits on a FIFO.
const SOON: Duration = Duration::from_secs(20);
const SECRET: &str = "session-secret=hunter2\n";

async fn server_dir(app: &TestApp, id: &str) -> PathBuf {
    app.state
        .instances
        .registry()
        .get(id.parse().unwrap())
        .await
        .unwrap()
        .layout
        .server_dir()
}

/// A file outside every server folder, such as the app's own data.
fn outside_file(app: &TestApp) -> PathBuf {
    let path = app.dir.path().join("outside-the-server.txt");
    std::fs::write(&path, SECRET).unwrap();
    path
}

fn replace_with_link(path: &Path, target: &Path) {
    let _ = std::fs::remove_file(path);
    symlink(target, path).unwrap();
}

fn make_fifo(path: &Path) {
    let _ = std::fs::remove_file(path);
    let made = std::process::Command::new("mkfifo").arg(path).status().unwrap();
    assert!(made.success());
}

async fn finished(app: &TestApp, cookie: &str, started: axum::response::Response) -> Value {
    let status = started.status();
    let body = json_body(started).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    app.wait_for_job(body["job_id"].as_str().unwrap(), cookie).await
}

#[tokio::test]
async fn a_reinstall_and_a_properties_save_do_not_write_through_links() {
    let app = TestApp::new().await;
    let cookie = app.setup_owner().await;
    let id = app.installed_instance("Linked", &cookie).await;
    let server = server_dir(&app, &id).await;
    let outside = outside_file(&app);
    let not_there = app.dir.path().join("made-by-the-app.txt");

    // Everything a reinstall writes, and the names it used for work in progress.
    for name in ["server.jar", "eula.txt", "server.part", "server.jar.part"] {
        replace_with_link(&server.join(name), &outside);
    }
    replace_with_link(&server.join("server.properties"), &not_there);
    let job = finished(
        &app,
        &cookie,
        app.post(
            &format!("/api/v1/instances/{id}/install"),
            json!({}),
            Some(&cookie),
        )
        .await,
    )
    .await;
    assert_eq!(job["status"], "succeeded", "{job}");
    assert_eq!(std::fs::read_to_string(&outside).unwrap(), SECRET);
    assert!(!not_there.exists());
    for name in ["server.jar", "eula.txt"] {
        let kind = std::fs::symlink_metadata(server.join(name)).unwrap();
        assert!(kind.is_file(), "{name} is the real file now");
    }

    // The editor's save, with a link where its temporary file used to be made.
    std::fs::remove_file(server.join("server.properties")).unwrap();
    std::fs::write(server.join("server.properties"), "motd=Hi\n").unwrap();
    replace_with_link(&server.join("server.properties.gsd-tmp"), &outside);
    let url = format!("/api/v1/instances/{id}/properties");
    let revision = json_body(app.get(&url, Some(&cookie)).await).await["revision"].clone();
    let saved = app
        .put(
            &url,
            json!({ "revision": revision, "changes": { "motd": "Changed" } }),
            Some(&cookie),
        )
        .await;
    assert_eq!(saved.status(), StatusCode::OK);
    assert_eq!(std::fs::read_to_string(&outside).unwrap(), SECRET);
    assert!(
        std::fs::read_to_string(server.join("server.properties"))
            .unwrap()
            .contains("motd=Changed")
    );

    // And the start, which writes the control channel's settings into the same file.
    replace_with_link(&server.join("server.properties"), &outside);
    let started = app
        .post(&format!("/api/v1/instances/{id}/start"), json!({}), Some(&cookie))
        .await;
    assert_eq!(started.status(), StatusCode::ACCEPTED);
    app.wait_for_state(&id, &cookie, "running").await;
    assert_eq!(std::fs::read_to_string(&outside).unwrap(), SECRET);
    app.post(&format!("/api/v1/instances/{id}/stop"), json!({}), Some(&cookie))
        .await;
    app.wait_for_state(&id, &cookie, "stopped").await;
}

#[tokio::test]
async fn nothing_shows_what_a_link_out_of_the_folder_points_at() {
    let app = TestApp::new().await;
    let cookie = app.setup_owner().await;
    let id = app.installed_instance("Leaky", &cookie).await;
    let server = server_dir(&app, &id).await;
    let outside = outside_file(&app);
    for name in [
        "server.properties",
        "ops.json",
        "whitelist.json",
        "banned-players.json",
        "usercache.json",
        "notes.txt",
    ] {
        replace_with_link(&server.join(name), &outside);
    }
    std::fs::create_dir_all(server.join("world")).unwrap();
    replace_with_link(&server.join("world/level.dat"), &outside);
    std::fs::create_dir_all(server.join("logs")).unwrap();
    replace_with_link(&server.join("logs/latest.log"), &outside);

    let instance = format!("/api/v1/instances/{id}");
    for path in [
        "/properties",
        "/players",
        "/worlds",
        "/gamerules",
        "/logs",
        "/logs/latest.log",
        "/logs/latest.log/download",
        "/files",
        "/files/text?path=notes.txt",
        "/files/download?path=notes.txt",
        "/files/download?path=logs",
        "/worlds/world/download",
        "/control",
        "/content",
        "/commands",
    ] {
        let response = timeout(SOON, app.get(&format!("{instance}{path}"), Some(&cookie)))
            .await
            .unwrap_or_else(|_| panic!("{path} never answered"));
        // Downloads are zips: looked at as bytes.
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body = String::from_utf8_lossy(&bytes);
        assert!(!body.contains("hunter2"), "{path} showed it: {body}");
    }

    // A backup leaves the links out, so its download holds nothing of the file either.
    let backup = finished(
        &app,
        &cookie,
        app.post(
            &format!("{instance}/backups"),
            json!({ "scope": "full" }),
            Some(&cookie),
        )
        .await,
    )
    .await;
    assert_eq!(backup["status"], "succeeded", "{backup}");
    let listed = json_body(app.get(&format!("{instance}/backups"), Some(&cookie)).await).await;
    let backup_id = listed["backups"][0]["id"].as_str().unwrap();
    let archive = app
        .get(&format!("{instance}/backups/{backup_id}/download"), Some(&cookie))
        .await;
    let bytes = axum::body::to_bytes(archive.into_body(), usize::MAX)
        .await
        .unwrap();
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes.to_vec())).unwrap();
    for index in 0..zip.len() {
        let mut entry = zip.by_index(index).unwrap();
        let mut contents = Vec::new();
        std::io::Read::read_to_end(&mut entry, &mut contents).unwrap();
        assert!(
            !String::from_utf8_lossy(&contents).contains("hunter2"),
            "{} holds it",
            entry.name().unwrap()
        );
    }
}

#[tokio::test]
async fn fifos_and_files_without_end_hold_nothing_up() {
    let app = TestApp::new().await;
    let cookie = app.setup_owner().await;
    let id = app.installed_instance("Stuck", &cookie).await;
    let server = server_dir(&app, &id).await;
    std::fs::create_dir_all(server.join("world")).unwrap();
    std::fs::create_dir_all(server.join("logs")).unwrap();
    for name in [
        "ops.json",
        "whitelist.json",
        "usercache.json",
        "world/level.dat",
        "world/session.lock",
        "logs/latest.log",
        "notes.txt",
    ] {
        make_fifo(&server.join(name));
    }
    // 64 GiB that take no room on disk; read whole, they would take all the app's memory.
    let _ = std::fs::remove_file(server.join("server.properties"));
    std::fs::File::create(server.join("server.properties"))
        .unwrap()
        .set_len(64 << 30)
        .unwrap();
    replace_with_link(&server.join("endless.txt"), Path::new("/dev/zero"));

    let instance = format!("/api/v1/instances/{id}");
    for path in [
        "/properties",
        "/players",
        "/worlds",
        "/gamerules",
        "/logs",
        "/logs/latest.log",
        "/logs/latest.log/download",
        "/files",
        "/files/text?path=notes.txt",
        "/files/text?path=endless.txt",
        "/files/text?path=server.properties",
        "/files/download?path=notes.txt",
        "/files/download?path=world",
        "/worlds/world/download",
        "/control",
        "/content",
        "/commands",
    ] {
        let response = timeout(SOON, app.get(&format!("{instance}{path}"), Some(&cookie)))
            .await
            .unwrap_or_else(|_| panic!("{path} never answered"));
        // Streams (the zipped folder) must end too.
        timeout(SOON, axum::body::to_bytes(response.into_body(), usize::MAX))
            .await
            .unwrap_or_else(|_| panic!("{path} never finished"))
            .unwrap();
    }

    // A backup, a start (it writes the control settings) and a copy all get through. (Without the
    // 64 GiB file: a backup would faithfully pack every one of its zeros.)
    std::fs::remove_file(server.join("server.properties")).unwrap();
    let backup = timeout(SOON, async {
        finished(
            &app,
            &cookie,
            app.post(
                &format!("{instance}/backups"),
                json!({ "scope": "full" }),
                Some(&cookie),
            )
            .await,
        )
        .await
    })
    .await
    .expect("the backup doesn't wait on a FIFO");
    assert_eq!(backup["status"], "succeeded", "{backup}");

    let cloned = timeout(SOON, async {
        let started = app
            .post(
                &format!("{instance}/clone"),
                json!({ "name": "Copy", "include_worlds": true }),
                Some(&cookie),
            )
            .await;
        finished(&app, &cookie, started).await
    })
    .await
    .expect("the copy doesn't wait on a FIFO");
    assert_eq!(cloned["status"], "succeeded", "{cloned}");

    let started = timeout(
        SOON,
        app.post(&format!("{instance}/start"), json!({}), Some(&cookie)),
    )
    .await
    .expect("the start doesn't wait on a FIFO");
    assert_eq!(started.status(), StatusCode::ACCEPTED);
    app.wait_for_state(&id, &cookie, "running").await;
    app.post(&format!("{instance}/stop"), json!({}), Some(&cookie))
        .await;
    app.wait_for_state(&id, &cookie, "stopped").await;
}

fn world_zip() -> Vec<u8> {
    use std::io::Write;
    let level = std::fs::read(format!(
        "{}/../gsd-minecraft/tests/fixtures/level_1.20.1.dat",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap();
    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    zip.start_file("level.dat", zip::write::SimpleFileOptions::default())
        .unwrap();
    zip.write_all(&level).unwrap();
    zip.finish().unwrap().into_inner()
}

/// A world upload that fails cleans up after itself, and only after itself. It used to move the
/// folder of the world's name to the trash, even when the server (or another upload) had made that
/// folder while the upload was still arriving.
#[tokio::test]
async fn a_failed_world_upload_removes_only_what_it_made() {
    let app = TestApp::new().await;
    let cookie = app.setup_owner().await;
    let id = app.installed_instance("Uploads", &cookie).await;
    let server = server_dir(&app, &id).await;

    // The folder appears while the upload's body is still on its way.
    let made_meanwhile = server.join("survival");
    let body = stream::once({
        let made_meanwhile = made_meanwhile.clone();
        async move {
            std::fs::create_dir(&made_meanwhile).unwrap();
            std::fs::write(made_meanwhile.join("level.dat"), "the server's own").unwrap();
            Ok::<_, std::io::Error>(Bytes::from(world_zip()))
        }
    });
    let uploaded = app
        .state
        .worlds
        .upload(id.parse().unwrap(), "survival", Box::pin(body), false)
        .await;
    assert!(matches!(uploaded, Err(WorldError::Exists(_))), "{uploaded:?}");
    assert_eq!(
        std::fs::read_to_string(made_meanwhile.join("level.dat")).unwrap(),
        "the server's own",
        "the folder someone else made is as it was"
    );

    // Two uploads of the same name at once: one becomes the world, the other is told it exists.
    let upload_url = format!("/api/v1/instances/{id}/worlds/twice");
    let upload = || app.put_bytes(&upload_url, world_zip(), Some(&cookie));
    let (first, second) = tokio::join!(upload(), upload());
    let mut statuses = [first.status(), second.status()];
    statuses.sort();
    assert_eq!(statuses, [StatusCode::CREATED, StatusCode::CONFLICT]);
    assert!(server.join("twice/level.dat").is_file());

    let left: Vec<String> = std::fs::read_dir(&server)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .filter(|name| name.starts_with(".gsd"))
        .collect();
    assert_eq!(left, Vec::<String>::new(), "no upload left anything behind");
}
