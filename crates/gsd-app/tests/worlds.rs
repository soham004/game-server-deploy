//! Worlds through the API, with real `level.dat` files: the list (a Paper-style world with its nether, an
//! older world), who sees seeds, making one active, downloads, uploads (a folder inside the zip, a zip
//! that isn't a world), deleting and resetting after a backup, and worlds that won't load.

#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::{io::Write, path::PathBuf};

use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use common::{TestApp, json_body};
use gsd_core::auth::{GlobalRole, Grant};
use serde_json::{Value, json};

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(format!(
        "{}/../gsd-minecraft/tests/fixtures/{name}",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap()
}

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

/// A server with `world` (26.3, with a Paper-style `world_nether`) and `old_world` (1.20.1).
async fn server_with_worlds(app: &TestApp, cookie: &str) -> (String, PathBuf) {
    let id = app.installed_instance("Worlds", cookie).await;
    let dir = server_dir(app, &id).await;
    for (folder, file) in [
        ("world", "level_26.3.dat"),
        ("world_nether", "level_26.3.dat"),
        ("old_world", "level_1.20.1.dat"),
    ] {
        std::fs::create_dir_all(dir.join(folder).join("region")).unwrap();
        std::fs::write(dir.join(folder).join("level.dat"), fixture(file)).unwrap();
        std::fs::write(dir.join(folder).join("region/r.0.0.mca"), vec![7_u8; 1000]).unwrap();
    }
    std::fs::create_dir_all(dir.join("logs")).unwrap();
    (id, dir)
}

async fn worlds(app: &TestApp, id: &str, cookie: &str) -> Value {
    json_body(
        app.get(&format!("/api/v1/instances/{id}/worlds"), Some(cookie))
            .await,
    )
    .await
}

fn world<'a>(view: &'a Value, folder: &str) -> &'a Value {
    view["worlds"]
        .as_array()
        .unwrap()
        .iter()
        .find(|w| w["folder"] == folder)
        .unwrap()
}

fn zip_of(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    for (name, bytes) in entries {
        if name.ends_with('/') {
            zip.add_directory(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
        } else {
            zip.start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(bytes).unwrap();
        }
    }
    zip.finish().unwrap().into_inner()
}

async fn upload(
    app: &TestApp,
    id: &str,
    cookie: &str,
    folder: &str,
    query: &str,
    zip: Vec<u8>,
) -> (StatusCode, Value) {
    let request = Request::put(format!("/api/v1/instances/{id}/worlds/{folder}{query}"))
        .header("cookie", cookie)
        .header("x-csrf", "1")
        .body(Body::from(zip))
        .unwrap();
    let response = app.send(request).await;
    let status = response.status();
    (status, json_body(response).await)
}

fn property(dir: &std::path::Path, key: &str) -> Option<String> {
    std::fs::read_to_string(dir.join("server.properties"))
        .unwrap_or_default()
        .lines()
        .find_map(|line| line.strip_prefix(&format!("{key}=")).map(str::to_owned))
}

#[tokio::test]
async fn worlds_are_listed_with_what_their_files_say() {
    let app = TestApp::new().await;
    let cookie = app.setup_owner().await;
    let (id, _) = server_with_worlds(&app, &cookie).await;

    let view = worlds(&app, &id, &cookie).await;
    assert_eq!(view["active"], "world");
    assert_eq!(view["running"], false);
    let folders: Vec<_> = view["worlds"]
        .as_array()
        .unwrap()
        .iter()
        .map(|w| w["folder"].as_str().unwrap())
        .collect();
    assert_eq!(
        folders,
        ["world", "old_world"],
        "the active one first; its nether is a part, logs isn't a world"
    );
    let active = world(&view, "world");
    assert_eq!(
        (active["active"].clone(), active["parts"].clone()),
        (json!(true), json!(["world_nether"]))
    );
    assert_eq!(active["version"], "26.3");
    assert_eq!(active["difficulty"], "easy");
    assert!(
        active["size_bytes"].as_u64().unwrap() > 2000,
        "both folders counted: {}",
        active["size_bytes"]
    );
    let old = world(&view, "old_world");
    assert_eq!(
        (old["version"].as_str(), old["seed"].as_str()),
        (Some("1.20.1"), Some("8984527508982599705"))
    );
    assert_eq!(old["game_mode"], "survival");

    let viewer = app.create_user("viewer", GlobalRole::Member).await;
    app.state
        .auth
        .grants()
        .set(viewer, id.parse().unwrap(), Grant::Viewer)
        .await
        .unwrap();
    let viewer = app.login("viewer").await;
    let seen = worlds(&app, &id, &viewer).await;
    assert_eq!(
        world(&seen, "old_world")["seed"],
        Value::Null,
        "seeds are for those who may read files"
    );
    assert_eq!(world(&seen, "old_world")["version"], "1.20.1");
    let denied = app
        .post(
            &format!("/api/v1/instances/{id}/worlds/old_world/activate"),
            json!({}),
            Some(&viewer),
        )
        .await;
    assert_eq!(denied.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn a_world_is_made_active_and_downloaded_while_stopped() {
    let app = TestApp::new().await;
    let cookie = app.setup_owner().await;
    let (id, dir) = server_with_worlds(&app, &cookie).await;

    let activated = app
        .post(
            &format!("/api/v1/instances/{id}/worlds/old_world/activate"),
            json!({}),
            Some(&cookie),
        )
        .await;
    assert_eq!(activated.status(), StatusCode::OK);
    assert_eq!(json_body(activated).await["restart_needed"], false);
    assert_eq!(property(&dir, "level-name").as_deref(), Some("old_world"));
    let missing = app
        .post(
            &format!("/api/v1/instances/{id}/worlds/nowhere/activate"),
            json!({}),
            Some(&cookie),
        )
        .await;
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);

    let download = app
        .get(
            &format!("/api/v1/instances/{id}/worlds/old_world/download"),
            Some(&cookie),
        )
        .await;
    assert_eq!(download.status(), StatusCode::OK);
    assert!(
        download.headers()["content-disposition"]
            .to_str()
            .unwrap()
            .contains("old_world.zip")
    );
    let bytes = axum::body::to_bytes(download.into_body(), usize::MAX)
        .await
        .unwrap();
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
    assert!(zip.by_name("old_world/level.dat").is_ok());

    let started = app
        .post(&format!("/api/v1/instances/{id}/start"), json!({}), Some(&cookie))
        .await;
    assert!(started.status().is_success());
    app.wait_for_state(&id, &cookie, "running").await;
    let running = app
        .get(
            &format!("/api/v1/instances/{id}/worlds/old_world/download"),
            Some(&cookie),
        )
        .await;
    assert_eq!(
        running.status(),
        StatusCode::CONFLICT,
        "a running server is still writing it"
    );
    let switched = app
        .post(
            &format!("/api/v1/instances/{id}/worlds/world/activate"),
            json!({}),
            Some(&cookie),
        )
        .await;
    assert_eq!(json_body(switched).await["restart_needed"], true);
    let stop = app
        .post(&format!("/api/v1/instances/{id}/stop"), json!({}), Some(&cookie))
        .await;
    assert!(stop.status().is_success());
    app.wait_for_state(&id, &cookie, "stopped").await;
}

#[tokio::test]
async fn an_uploaded_zip_becomes_a_world_only_if_it_is_one() {
    let app = TestApp::new().await;
    let cookie = app.setup_owner().await;
    let (id, dir) = server_with_worlds(&app, &cookie).await;
    let level = fixture("level_1.20.1.dat");

    // Zipped with its folder, on a Mac.
    let nested = zip_of(&[
        ("MyWorld/", b""),
        ("MyWorld/level.dat", &level),
        ("MyWorld/region/r.0.0.mca", b"x"),
        ("__MACOSX/", b""),
        ("__MACOSX/._MyWorld", b"x"),
    ]);
    let (status, body) = upload(&app, &id, &cookie, "imported", "?activate=true", nested).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(body["version"], "1.20.1");
    assert!(
        dir.join("imported/level.dat").is_file(),
        "the inner folder's contents are the world"
    );
    assert!(dir.join("imported/region/r.0.0.mca").is_file());
    assert_eq!(property(&dir, "level-name").as_deref(), Some("imported"));
    let leftovers: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .filter(|n| n.starts_with(".gsd"))
        .collect();
    assert_eq!(leftovers, Vec::<String>::new(), "no upload left behind");

    // Zipped from inside the folder.
    let flat = zip_of(&[("level.dat", &level)]);
    assert_eq!(
        upload(&app, &id, &cookie, "flat", "", flat.clone()).await.0,
        StatusCode::CREATED
    );
    assert_eq!(
        upload(&app, &id, &cookie, "flat", "", flat).await.1["error"]["code"],
        "world_exists"
    );

    let not_a_world = zip_of(&[("readme.txt", b"hello")]);
    let (status, body) = upload(&app, &id, &cookie, "junk", "", not_a_world).await;
    assert_eq!(
        (status, body["error"]["code"].as_str()),
        (StatusCode::UNPROCESSABLE_ENTITY, Some("not_a_world"))
    );
    assert!(!dir.join("junk").exists(), "it went to the trash");
    let (status, _) = upload(
        &app,
        &id,
        &cookie,
        ".hidden",
        "",
        zip_of(&[("level.dat", &level)]),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
}

async fn finished(app: &TestApp, cookie: &str, response: axum::response::Response) -> Value {
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    let job = json_body(response).await;
    app.wait_for_job(job["job_id"].as_str().unwrap(), cookie).await
}

#[tokio::test]
async fn worlds_are_deleted_and_reset_after_a_backup() {
    let app = TestApp::new().await;
    let cookie = app.setup_owner().await;
    let (id, dir) = server_with_worlds(&app, &cookie).await;

    let other = app
        .post(
            &format!("/api/v1/instances/{id}/worlds/old_world/reset"),
            json!({}),
            Some(&cookie),
        )
        .await;
    assert_eq!(json_body(other).await["error"]["code"], "world_not_active");

    let job = finished(
        &app,
        &cookie,
        app.delete(&format!("/api/v1/instances/{id}/worlds/old_world"), Some(&cookie))
            .await,
    )
    .await;
    assert_eq!(job["status"], "succeeded", "{job}");
    assert!(!dir.join("old_world").exists());

    let job = finished(
        &app,
        &cookie,
        app.post(
            &format!("/api/v1/instances/{id}/worlds/world/reset"),
            json!({ "seed": "glacier" }),
            Some(&cookie),
        )
        .await,
    )
    .await;
    assert_eq!(job["status"], "succeeded", "{job}");
    assert!(
        !dir.join("world").exists() && !dir.join("world_nether").exists(),
        "the world and its parts"
    );
    assert_eq!(property(&dir, "level-seed").as_deref(), Some("glacier"));
    assert_eq!(worlds(&app, &id, &cookie).await["worlds"], json!([]));

    let backups = json_body(
        app.get(&format!("/api/v1/instances/{id}/backups"), Some(&cookie))
            .await,
    )
    .await;
    let kinds: Vec<_> = backups["backups"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| (b["kind"].as_str().unwrap(), b["scope"].as_str().unwrap()))
        .collect();
    assert_eq!(
        kinds,
        [("pre_delete", "world"), ("pre_delete", "full")],
        "newest first: one before each"
    );

    let bad = app
        .post(
            &format!("/api/v1/instances/{id}/worlds/world/reset"),
            json!({ "seed": "a\nb" }),
            Some(&cookie),
        )
        .await;
    assert_eq!(bad.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn a_world_that_wont_load_is_listed_and_deleted_even_when_active() {
    let app = TestApp::new().await;
    let cookie = app.setup_owner().await;
    let (id, dir) = server_with_worlds(&app, &cookie).await;
    // The active world's level.dat is garbage; `crashed` lost its level.dat while saving and kept
    // level.dat_old; `half` is a first start that never finished, with only its lock.
    std::fs::write(dir.join("world/level.dat"), b"not a level").unwrap();
    std::fs::create_dir_all(dir.join("crashed")).unwrap();
    std::fs::write(dir.join("crashed/level.dat_old"), fixture("level_26.3.dat")).unwrap();
    std::fs::create_dir_all(dir.join("half/region")).unwrap();
    std::fs::write(dir.join("half/session.lock"), "☃").unwrap();

    let view = worlds(&app, &id, &cookie).await;
    let problem = |folder: &str| world(&view, folder)["problem"].as_str().unwrap().to_owned();
    assert!(
        problem("world").starts_with("its level.dat can't be read"),
        "{view}"
    );
    assert_eq!(problem("crashed"), "it has no level.dat");
    assert_eq!(problem("half"), "it has no level.dat");
    assert!(world(&view, "old_world")["problem"].is_null());

    let level_name = property(&dir, "level-name");
    let job = finished(
        &app,
        &cookie,
        app.delete(&format!("/api/v1/instances/{id}/worlds/world"), Some(&cookie))
            .await,
    )
    .await;
    assert_eq!(job["status"], "succeeded", "{job}");
    assert!(
        !dir.join("world").exists() && !dir.join("world_nether").exists(),
        "the world and its parts went"
    );
    assert_eq!(
        property(&dir, "level-name"),
        level_name,
        "level-name is left alone, so the next start makes a new world"
    );
    let backups = json_body(
        app.get(&format!("/api/v1/instances/{id}/backups"), Some(&cookie))
            .await,
    )
    .await;
    assert_eq!(backups["backups"][0]["kind"], "pre_delete");
    assert_eq!(backups["backups"][0]["scope"], "world", "the active world's");

    for folder in ["crashed", "half"] {
        let job = finished(
            &app,
            &cookie,
            app.delete(&format!("/api/v1/instances/{id}/worlds/{folder}"), Some(&cookie))
                .await,
        )
        .await;
        assert_eq!(job["status"], "succeeded", "{folder}: {job}");
        assert!(!dir.join(folder).exists());
    }
    let left: Vec<_> = worlds(&app, &id, &cookie).await["worlds"]
        .as_array()
        .unwrap()
        .iter()
        .map(|w| w["folder"].clone())
        .collect();
    assert_eq!(left, [json!("old_world")]);
    assert!(dir.join("logs").exists(), "logs was never a world");
}
