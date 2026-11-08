//! The trash: deleting a server (with or without its backups) and emptying entries older than
//! the retention, in the app's trash and in each server's own.

#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::path::Path;

use axum::http::StatusCode;
use chrono::{TimeDelta, Utc};
use common::{TestApp, json_body};
use gsd_core::{ids::InstanceId, trash};
use serde_json::json;

fn names(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

async fn backed_up(app: &TestApp, name: &str, cookie: &str) -> String {
    let id = app.installed_instance(name, cookie).await;
    let started = json_body(
        app.post(
            &format!("/api/v1/instances/{id}/backups"),
            json!({ "scope": "full" }),
            Some(cookie),
        )
        .await,
    )
    .await;
    let job = app
        .wait_for_job(started["job_id"].as_str().unwrap(), cookie)
        .await;
    assert_eq!(job["status"], "succeeded");
    id
}

#[tokio::test]
async fn deleting_a_server_keeps_its_backups_unless_asked() {
    let app = TestApp::new().await;
    let cookie = app.setup_owner().await;
    let kept = backed_up(&app, "Kept", &cookie).await;
    let gone = backed_up(&app, "Gone", &cookie).await;
    let backups = app.dir.path().join("backups");
    let trash_dir = app.dir.path().join("trash");

    let response = app
        .delete(&format!("/api/v1/instances/{kept}"), Some(&cookie))
        .await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert_eq!(
        names(&backups.join(&kept)).len(),
        1,
        "the archive stays where it was"
    );

    let response = app
        .delete(
            &format!("/api/v1/instances/{gone}?delete_backups=true"),
            Some(&cookie),
        )
        .await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert!(!backups.join(&gone).exists());
    let trashed = names(&trash_dir);
    assert_eq!(
        trashed.len(),
        3,
        "two servers and one folder of backups: {trashed:?}"
    );
    let backups_entry = trashed
        .iter()
        .find(|name| name.ends_with("-gone-backups"))
        .expect("the backups in the trash");
    assert!(
        trash::deleted_at(backups_entry).is_some(),
        "named with the deletion time"
    );
    assert_eq!(
        names(&trash_dir.join(backups_entry)).len(),
        1,
        "the archive is inside"
    );
    let rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM backups WHERE instance_id = ?")
        .bind(&gone)
        .fetch_one(&app.pool)
        .await
        .unwrap();
    assert_eq!(rows, 0);
}

#[tokio::test]
async fn old_entries_are_emptied_from_every_trash_and_recent_ones_stay() {
    let app = TestApp::new().await;
    let cookie = app.setup_owner().await;
    let id = app.installed_instance("Tidy", &cookie).await;
    let instance: InstanceId = id.parse().unwrap();
    let layout = app.state.instances.get(instance).await.unwrap().layout;

    // A file deleted through the file manager today goes to the server's own trash.
    let response = app
        .put(
            &format!("/api/v1/instances/{id}/files/text"),
            json!({ "path": "notes.txt", "content": "hi" }),
            Some(&cookie),
        )
        .await;
    assert!(response.status().is_success());
    let response = app
        .post(
            &format!("/api/v1/instances/{id}/files/delete"),
            json!({ "paths": ["notes.txt"] }),
            Some(&cookie),
        )
        .await;
    assert!(response.status().is_success());
    // Entries deleted long ago, in both places.
    let long_ago = Utc::now() - TimeDelta::days(30);
    std::fs::write(
        layout.trash_dir().join(trash::entry_name("old.txt", long_ago)),
        "x",
    )
    .unwrap();
    let app_trash = app.state.config.trash_dir();
    std::fs::create_dir_all(
        app_trash
            .join(trash::entry_name("old-server", long_ago))
            .join("server"),
    )
    .unwrap();
    std::fs::write(app_trash.join("left-here-by-hand"), "x").unwrap();

    let purged = gsd_app::serve::empty_trash(&app.state).await;

    assert_eq!(
        (purged.removed, purged.kept, purged.unknown),
        (2, 1, 1),
        "{purged:?}"
    );
    let left = names(&layout.trash_dir());
    assert_eq!(left.len(), 1);
    assert!(left[0].ends_with("-notes.txt"), "{left:?}");
    assert_eq!(names(&app_trash), ["left-here-by-hand"]);
}
