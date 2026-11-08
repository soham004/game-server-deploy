//! Backups API: backing up stopped and running servers, restoring (world and full), retention,
//! pins and notes, downloads, damaged archives, and who may do what. The servers are the fake
//! Java from `gsd_minecraft::test_support`, which answers `save-off`/`save-all flush`/`save-on`.

#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::{io::Read, path::PathBuf};

use axum::http::{StatusCode, header};
use common::{TestApp, json_body};
use gsd_core::{
    auth::{GlobalRole, Grant},
    ids::InstanceId,
};
use http_body_util::BodyExt;
use serde_json::{Value, json};

fn backups_url(id: &str) -> String {
    format!("/api/v1/instances/{id}/backups")
}

async fn server_dir(app: &TestApp, id: &str) -> PathBuf {
    let id: InstanceId = id.parse().unwrap();
    app.state.instances.get(id).await.unwrap().layout.server_dir()
}

fn write(dir: &std::path::Path, path: &str, contents: &str) {
    std::fs::create_dir_all(dir.join(path).parent().unwrap()).unwrap();
    std::fs::write(dir.join(path), contents).unwrap();
}

fn read(dir: &std::path::Path, path: &str) -> Option<String> {
    std::fs::read_to_string(dir.join(path)).ok()
}

/// Starts a backup and waits for it; returns the finished job.
async fn back_up(app: &TestApp, id: &str, body: Value, cookie: &str) -> Value {
    let response = app.post(&backups_url(id), body, Some(cookie)).await;
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    let started = json_body(response).await;
    app.wait_for_job(started["job_id"].as_str().unwrap(), cookie)
        .await
}

async fn restore(app: &TestApp, id: &str, backup: &str, body: Value, cookie: &str) -> Value {
    let response = app
        .post(
            &format!("{}/{backup}/restore", backups_url(id)),
            body,
            Some(cookie),
        )
        .await;
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    let started = json_body(response).await;
    app.wait_for_job(started["job_id"].as_str().unwrap(), cookie)
        .await
}

async fn list(app: &TestApp, id: &str, cookie: &str) -> Value {
    json_body(app.get(&backups_url(id), Some(cookie)).await).await
}

/// The newest backup's id.
async fn newest(app: &TestApp, id: &str, cookie: &str) -> String {
    list(app, id, cookie).await["backups"][0]["id"]
        .as_str()
        .unwrap()
        .to_owned()
}

/// The console lines the server or the app produced, in order.
async fn console(app: &TestApp, id: &str, cookie: &str) -> Vec<String> {
    let snapshot = json_body(
        app.get(&format!("/api/v1/instances/{id}/console"), Some(cookie))
            .await,
    )
    .await;
    snapshot["console"]
        .as_array()
        .unwrap()
        .iter()
        .map(|line| line["text"].as_str().unwrap().to_owned())
        .collect()
}

async fn send(app: &TestApp, id: &str, command: &str, cookie: &str) {
    let response = app
        .post(
            &format!("/api/v1/instances/{id}/command"),
            json!({ "command": command }),
            Some(cookie),
        )
        .await;
    assert!(response.status().is_success(), "{command}: {}", response.status());
}

/// Waits until the fake server has handled everything sent so far (it answers in order).
async fn settle(app: &TestApp, id: &str, cookie: &str) {
    send(app, id, "say settled", cookie).await;
    for _ in 0..200 {
        if console(app, id, cookie)
            .await
            .iter()
            .any(|line| line.contains("[Server] settled"))
        {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    panic!("the server never answered");
}

#[tokio::test]
async fn backs_up_a_stopped_server_then_lists_downloads_and_deletes_it() {
    let app = TestApp::new().await;
    let cookie = app.setup_owner().await;
    let id = app.installed_instance("Survival", &cookie).await;
    let dir = server_dir(&app, &id).await;
    write(&dir, "world/level.dat", "first world");

    let job = back_up(
        &app,
        &id,
        json!({ "scope": "world", "note": "  before the raid  " }),
        &cookie,
    )
    .await;
    assert_eq!(job["status"], "succeeded", "{job}");

    let listed = list(&app, &id, &cookie).await;
    let backup = &listed["backups"][0];
    assert_eq!(backup["kind"], "manual");
    assert_eq!(backup["scope"], "world");
    assert_eq!(backup["note"], "before the raid");
    assert_eq!(backup["software"], "Vanilla 26.3");
    assert_eq!(backup["pinned"], false);
    assert_eq!(listed["total_bytes"], backup["size_bytes"]);
    assert_eq!(listed["policy"], json!({ "keep": 10, "keep_safety": 3 }));
    let file_name = backup["file_name"].as_str().unwrap();
    assert!(file_name.ends_with("-manual-world.zip"), "{file_name}");

    // The download is the archive: the world under server/, and the manifest.
    let backup_id = backup["id"].as_str().unwrap();
    let response = app
        .get(
            &format!("{}/{backup_id}/download", backups_url(&id)),
            Some(&cookie),
        )
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let disposition = response.headers()[header::CONTENT_DISPOSITION]
        .to_str()
        .unwrap()
        .to_owned();
    assert!(
        disposition.contains(&format!("survival-{file_name}")),
        "{disposition}"
    );
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(bytes.len() as u64, backup["size_bytes"].as_u64().unwrap());
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
    let mut level = String::new();
    zip.by_name("server/world/level.dat")
        .unwrap()
        .read_to_string(&mut level)
        .unwrap();
    assert_eq!(level, "first world");
    assert!(zip.by_name("gsd-backup.json").is_ok());
    assert!(zip.by_name("server/server.properties").is_err(), "a world backup");

    let archive = app.dir.path().join("backups").join(&id).join(file_name);
    assert!(archive.is_file());
    let response = app
        .delete(&format!("{}/{backup_id}", backups_url(&id)), Some(&cookie))
        .await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert!(!archive.exists());
    assert_eq!(list(&app, &id, &cookie).await["backups"], json!([]));
}

#[tokio::test]
async fn restoring_a_world_backup_replaces_only_the_world_and_keeps_a_safety_backup() {
    let app = TestApp::new().await;
    let cookie = app.setup_owner().await;
    let id = app.installed_instance("Survival", &cookie).await;
    let dir = server_dir(&app, &id).await;
    write(&dir, "world/level.dat", "the original");
    assert_eq!(
        back_up(&app, &id, json!({ "scope": "world" }), &cookie).await["status"],
        "succeeded"
    );
    let original = newest(&app, &id, &cookie).await;

    write(&dir, "world/level.dat", "griefed");
    write(&dir, "world/griefer-was-here.txt", "x");
    write(&dir, "notes.txt", "kept: not part of the world");

    let job = restore(&app, &id, &original, json!({}), &cookie).await;
    assert_eq!(job["status"], "succeeded", "{job}");

    assert_eq!(read(&dir, "world/level.dat").as_deref(), Some("the original"));
    assert_eq!(read(&dir, "world/griefer-was-here.txt"), None);
    assert_eq!(
        read(&dir, "notes.txt").as_deref(),
        Some("kept: not part of the world")
    );
    assert!(read(&dir, "server.properties").is_some());

    // The state before the restore was saved first, so the restore can be undone.
    let backups = list(&app, &id, &cookie).await["backups"].clone();
    assert_eq!(backups.as_array().unwrap().len(), 2);
    assert_eq!(backups[0]["kind"], "pre_restore");
    assert_eq!(backups[0]["scope"], "world");
    assert!(
        backups[0]["note"]
            .as_str()
            .unwrap()
            .starts_with("Before restoring the backup from")
    );
    let undo = backups[0]["id"].as_str().unwrap();
    assert_eq!(
        restore(&app, &id, undo, json!({ "safety_backup": false }), &cookie).await["status"],
        "succeeded"
    );
    assert_eq!(read(&dir, "world/level.dat").as_deref(), Some("griefed"));
    assert_eq!(
        list(&app, &id, &cookie).await["backups"]
            .as_array()
            .unwrap()
            .len(),
        2,
        "no new safety backup"
    );

    // Nothing is left in the staging folders.
    let instance_root = dir.parent().unwrap();
    assert!(!instance_root.join("restore").exists());
    assert!(!instance_root.join("restore-old").exists());
}

#[tokio::test]
async fn a_full_restore_puts_back_everything_but_keeps_libraries_and_logs() {
    let app = TestApp::new().await;
    let cookie = app.setup_owner().await;
    let id = app.installed_instance("Modded", &cookie).await;
    let dir = server_dir(&app, &id).await;
    write(&dir, "world/level.dat", "world");
    write(&dir, "plugins/good.jar", "good plugin");
    write(&dir, "libraries/lib.jar", "library v1");
    write(&dir, "logs/latest.log", "old log");
    assert_eq!(
        back_up(&app, &id, json!({ "scope": "full" }), &cookie).await["status"],
        "succeeded"
    );
    let full = newest(&app, &id, &cookie).await;

    std::fs::remove_file(dir.join("plugins/good.jar")).unwrap();
    write(&dir, "plugins/bad.jar", "bad plugin");
    write(&dir, "libraries/lib.jar", "library v2");
    write(&dir, "logs/latest.log", "new log");
    write(&dir, "server.properties", "broken config");

    let job = restore(&app, &id, &full, json!({}), &cookie).await;
    assert_eq!(job["status"], "succeeded", "{job}");

    assert_eq!(read(&dir, "plugins/good.jar").as_deref(), Some("good plugin"));
    assert_eq!(read(&dir, "plugins/bad.jar"), None);
    assert_ne!(read(&dir, "server.properties").as_deref(), Some("broken config"));
    assert_eq!(read(&dir, "world/level.dat").as_deref(), Some("world"));
    // Not in a full backup, so the current ones stay.
    assert_eq!(read(&dir, "libraries/lib.jar").as_deref(), Some("library v2"));
    assert_eq!(read(&dir, "logs/latest.log").as_deref(), Some("new log"));
}

#[tokio::test]
async fn a_running_server_saves_and_pauses_for_the_backup_and_restarts_after_a_restore() {
    let app = TestApp::new().await;
    let cookie = app.setup_owner().await;
    let id = app.installed_instance("Live", &cookie).await;
    let dir = server_dir(&app, &id).await;
    let started = app
        .post(&format!("/api/v1/instances/{id}/start"), json!({}), Some(&cookie))
        .await;
    assert!(started.status().is_success());
    app.wait_for_state(&id, &cookie, "running").await;
    send(&app, &id, "write saved by the game", &cookie).await;
    settle(&app, &id, &cookie).await;

    let job = back_up(&app, &id, json!({ "scope": "world" }), &cookie).await;
    assert_eq!(job["status"], "succeeded", "{job}");
    settle(&app, &id, &cookie).await;
    let lines = console(&app, &id, &cookie).await;
    let position = |needle: &str| {
        lines
            .iter()
            .position(|line| line.contains(needle))
            .unwrap_or_else(|| panic!("{needle:?} not in {lines:#?}"))
    };
    assert!(
        lines.iter().any(|line| line == "[backup] > save-off"),
        "{lines:#?}"
    );
    assert!(position("save-off") < position("Saved the game"));
    assert!(position("Saved the game") < position("save-on"));
    assert!(position("save-on") < position("Automatic saving is now enabled"));
    let backup = newest(&app, &id, &cookie).await;

    send(&app, &id, "write changed later", &cookie).await;
    settle(&app, &id, &cookie).await;
    let job = restore(&app, &id, &backup, json!({}), &cookie).await;
    assert_eq!(job["status"], "succeeded", "{job}");
    assert_eq!(
        read(&dir, "world/level.dat").as_deref(),
        Some("saved by the game")
    );
    // It was running, so it runs again.
    app.wait_for_state(&id, &cookie, "running").await;
    let steps: Vec<&str> = job["log"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert!(
        steps.iter().any(|line| line.contains("Stopping the server")),
        "{steps:#?}"
    );
    assert!(
        steps.iter().any(|line| line.contains("Starting the server")),
        "{steps:#?}"
    );
}

#[tokio::test]
async fn a_server_that_never_confirms_its_save_fails_the_backup_and_saving_is_turned_back_on() {
    let app = TestApp::new().await;
    let cookie = app.setup_owner().await;
    let id = app.installed_instance("Stuck", &cookie).await;
    std::fs::write(app.state.config.runtimes_dir().join("temurin-25-x64/no-save"), "").unwrap();
    app.post(&format!("/api/v1/instances/{id}/start"), json!({}), Some(&cookie))
        .await;
    app.wait_for_state(&id, &cookie, "running").await;

    let job = back_up(&app, &id, json!({ "scope": "world" }), &cookie).await;

    assert_eq!(job["status"], "failed");
    assert!(
        job["error"].as_str().unwrap().contains("didn't confirm the save"),
        "{job}"
    );
    settle(&app, &id, &cookie).await;
    assert!(
        console(&app, &id, &cookie)
            .await
            .iter()
            .any(|line| line == "[backup] > save-on")
    );
    assert_eq!(list(&app, &id, &cookie).await["backups"], json!([]));
    let leftovers = std::fs::read_dir(app.dir.path().join("backups").join(&id))
        .unwrap()
        .count();
    assert_eq!(leftovers, 0, "no partial archive");
}

#[tokio::test]
async fn retention_keeps_the_newest_backups_and_every_pinned_one() {
    let app = TestApp::new().await;
    let cookie = app.setup_owner().await;
    let id = app.installed_instance("Kept", &cookie).await;
    let settings = app
        .patch(
            &format!("/api/v1/instances/{id}/settings"),
            json!({ "keep_backups": 2 }),
            Some(&cookie),
        )
        .await;
    assert_eq!(json_body(settings).await["backups"]["keep"], 2);

    back_up(&app, &id, json!({ "scope": "world", "note": "keep me" }), &cookie).await;
    let pinned = newest(&app, &id, &cookie).await;
    let response = app
        .patch(
            &format!("{}/{pinned}", backups_url(&id)),
            json!({ "pinned": true }),
            Some(&cookie),
        )
        .await;
    assert_eq!(json_body(response).await["pinned"], true);
    for _ in 0..3 {
        assert_eq!(
            back_up(&app, &id, json!({ "scope": "world" }), &cookie).await["status"],
            "succeeded"
        );
    }

    let backups = list(&app, &id, &cookie).await["backups"].clone();
    let backups = backups.as_array().unwrap();
    assert_eq!(backups.len(), 3, "two newest plus the pinned one: {backups:#?}");
    assert!(backups.iter().any(|backup| backup["id"] == pinned.as_str()));
    let on_disk = std::fs::read_dir(app.dir.path().join("backups").join(&id))
        .unwrap()
        .count();
    assert_eq!(on_disk, 3, "expired archives are deleted too");

    // An empty note removes it; unknown backups are 404.
    let response = app
        .patch(
            &format!("{}/{pinned}", backups_url(&id)),
            json!({ "note": "" }),
            Some(&cookie),
        )
        .await;
    assert_eq!(json_body(response).await["note"], Value::Null);
    let missing = gsd_core::ids::BackupId::generate();
    let response = app
        .patch(
            &format!("{}/{missing}", backups_url(&id)),
            json!({ "pinned": true }),
            Some(&cookie),
        )
        .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_damaged_or_missing_archive_is_refused_before_anything_changes() {
    let app = TestApp::new().await;
    let cookie = app.setup_owner().await;
    let id = app.installed_instance("Careful", &cookie).await;
    let dir = server_dir(&app, &id).await;
    write(&dir, "world/level.dat", "backed up");
    back_up(&app, &id, json!({ "scope": "world" }), &cookie).await;
    let backup = list(&app, &id, &cookie).await["backups"][0].clone();
    let archive = app
        .dir
        .path()
        .join("backups")
        .join(&id)
        .join(backup["file_name"].as_str().unwrap());
    write(&dir, "world/level.dat", "current");

    let mut bytes = std::fs::read(&archive).unwrap();
    let middle = bytes.len() / 2;
    bytes[middle] ^= 0xff;
    std::fs::write(&archive, bytes).unwrap();
    let job = restore(&app, &id, backup["id"].as_str().unwrap(), json!({}), &cookie).await;
    assert_eq!(job["status"], "failed");
    assert!(job["error"].as_str().unwrap().contains("damaged"), "{job}");
    assert_eq!(read(&dir, "world/level.dat").as_deref(), Some("current"));
    assert_eq!(
        list(&app, &id, &cookie).await["backups"]
            .as_array()
            .unwrap()
            .len(),
        1,
        "no safety backup yet"
    );

    std::fs::remove_file(&archive).unwrap();
    let response = app
        .post(
            &format!("{}/{}/restore", backups_url(&id), backup["id"].as_str().unwrap()),
            json!({}),
            Some(&cookie),
        )
        .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(json_body(response).await["error"]["code"], "backup_file_missing");
}

#[tokio::test]
async fn requests_are_checked_before_a_job_starts() {
    let app = TestApp::new().await;
    let cookie = app.setup_owner().await;
    let id = app.installed_instance("Strict", &cookie).await;

    let long_note = "x".repeat(201);
    let response = app
        .post(
            &backups_url(&id),
            json!({ "scope": "world", "note": long_note }),
            Some(&cookie),
        )
        .await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(json_body(response).await["error"]["code"], "invalid_note");
    let response = app
        .post(&backups_url(&id), json!({ "scope": "everything" }), Some(&cookie))
        .await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let response = app
        .patch(
            &format!("/api/v1/instances/{id}/settings"),
            json!({ "keep_backups": 0 }),
            Some(&cookie),
        )
        .await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn operators_back_up_but_only_restorers_restore_delete_or_download() {
    let app = TestApp::new().await;
    let owner = app.setup_owner().await;
    let id = app.installed_instance("Shared", &owner).await;
    let instance: InstanceId = id.parse().unwrap();
    for (name, grant) in [("operator", Grant::Operator), ("viewer", Grant::Viewer)] {
        let user = app.create_user(name, GlobalRole::Member).await;
        app.state.auth.grants().set(user, instance, grant).await.unwrap();
    }
    app.create_user("stranger", GlobalRole::Member).await;
    let operator = app.login("operator").await;
    let viewer = app.login("viewer").await;
    let stranger = app.login("stranger").await;

    let job = back_up(&app, &id, json!({ "scope": "world" }), &operator).await;
    assert_eq!(job["status"], "succeeded");
    let backup = newest(&app, &id, &viewer).await;
    let one = format!("{}/{backup}", backups_url(&id));

    let pin = app.patch(&one, json!({ "pinned": true }), Some(&operator)).await;
    assert_eq!(pin.status(), StatusCode::OK);
    let forbidden = [
        app.post(&backups_url(&id), json!({ "scope": "world" }), Some(&viewer))
            .await,
        app.post(&format!("{one}/restore"), json!({}), Some(&operator))
            .await,
        app.delete(&one, Some(&operator)).await,
        app.get(&format!("{one}/download"), Some(&operator)).await,
    ];
    for response in forbidden {
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }
    let hidden = app.get(&backups_url(&id), Some(&stranger)).await;
    assert_eq!(
        hidden.status(),
        StatusCode::NOT_FOUND,
        "the server's existence doesn't leak"
    );
}
