//! A server's state against the work that needs it stopped: a crash's pending restart, a start
//! that arrives while a job begins, and the server a copy is made from. Each of these once let a
//! server come up in the middle of a job that was changing its files. Servers are the fake Java
//! from `gsd_minecraft::test_support` (`crash` makes it exit with an error).

#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::time::Duration;

use axum::http::StatusCode;
use common::{TestApp, json_body};
use serde_json::{Value, json};

/// Longer than the first automatic restart's delay (5 seconds).
const PAST_THE_RESTART: Duration = Duration::from_secs(7);

async fn post(app: &TestApp, id: &str, action: &str, cookie: &str) -> (StatusCode, Value) {
    let response = app
        .post(
            &format!("/api/v1/instances/{id}/{action}"),
            json!({}),
            Some(cookie),
        )
        .await;
    let status = response.status();
    (status, json_body(response).await)
}

async fn state(app: &TestApp, id: &str, cookie: &str) -> Value {
    json_body(app.get(&format!("/api/v1/instances/{id}"), Some(cookie)).await).await["state"].clone()
}

/// Starts the server and makes it crash: it is then down, with a restart due in 5 seconds.
async fn crashed_with_a_restart_pending(app: &TestApp, id: &str, cookie: &str) {
    assert_eq!(post(app, id, "start", cookie).await.0, StatusCode::ACCEPTED);
    app.wait_for_state(id, cookie, "running").await;
    let crash = app
        .post(
            &format!("/api/v1/instances/{id}/command"),
            json!({ "command": "crash" }),
            Some(cookie),
        )
        .await;
    assert_eq!(crash.status(), StatusCode::NO_CONTENT);
    let crashed = app.wait_for_state(id, cookie, "crashed").await;
    assert!(
        crashed["state"]["restart_at"].is_string(),
        "an automatic restart is due: {crashed}"
    );
}

async fn finished(app: &TestApp, cookie: &str, started: &Value) -> Value {
    app.wait_for_job(started["job_id"].as_str().expect("a job"), cookie)
        .await
}

#[tokio::test]
async fn a_reinstall_cancels_a_crashs_pending_restart_instead_of_being_run_over_by_it() {
    let app = TestApp::new().await;
    let cookie = app.setup_owner().await;
    let id = app.installed_instance("Crashing", &cookie).await;
    crashed_with_a_restart_pending(&app, &id, &cookie).await;

    let (status, started) = post(&app, &id, "install", &cookie).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{started}");
    let job = finished(&app, &cookie, &started).await;
    assert_eq!(job["status"], "succeeded", "{job}");
    assert_eq!(state(&app, &id, &cookie).await["status"], "stopped");

    tokio::time::sleep(PAST_THE_RESTART).await;
    assert_eq!(
        state(&app, &id, &cookie).await["status"],
        "stopped",
        "the restart that was due never came"
    );
}

#[tokio::test]
async fn a_version_change_and_a_world_deletion_cancel_a_pending_restart_too() {
    let app = TestApp::new().await;
    let cookie = app.setup_owner().await;
    let id = app.installed_instance("Crashing", &cookie).await;
    let server = app
        .state
        .instances
        .get(id.parse().unwrap())
        .await
        .unwrap()
        .layout
        .server_dir();
    std::fs::create_dir_all(server.join("world")).unwrap();
    std::fs::write(server.join("world/level.dat"), "not a real level").unwrap();

    crashed_with_a_restart_pending(&app, &id, &cookie).await;
    let deleted = app
        .delete(&format!("/api/v1/instances/{id}/worlds/world"), Some(&cookie))
        .await;
    assert_eq!(deleted.status(), StatusCode::ACCEPTED);
    let job = finished(&app, &cookie, &json_body(deleted).await).await;
    assert_eq!(job["status"], "succeeded", "{job}");
    assert_eq!(state(&app, &id, &cookie).await["status"], "stopped");
    assert!(!server.join("world").exists());

    crashed_with_a_restart_pending(&app, &id, &cookie).await;
    let changed = app
        .post(
            &format!("/api/v1/instances/{id}/change-version"),
            json!({ "mc_version": "26.2", "allow_downgrade": true }),
            Some(&cookie),
        )
        .await;
    let status = changed.status();
    let started = json_body(changed).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{started}");
    let job = finished(&app, &cookie, &started).await;
    assert_eq!(job["status"], "succeeded", "{job}");

    tokio::time::sleep(PAST_THE_RESTART).await;
    assert_eq!(state(&app, &id, &cookie).await["status"], "stopped");
}

/// A world can't be downloaded while a restart is due either: the server would come up, and write
/// to it, in the middle of the download.
#[tokio::test]
async fn a_world_is_not_downloaded_while_a_restart_is_due() {
    let app = TestApp::new().await;
    let cookie = app.setup_owner().await;
    let id = app.installed_instance("Crashing", &cookie).await;
    crashed_with_a_restart_pending(&app, &id, &cookie).await;

    // The fake server made `world` when it was told to; here the active world's folder is enough.
    let server = app
        .state
        .instances
        .get(id.parse().unwrap())
        .await
        .unwrap()
        .layout
        .server_dir();
    std::fs::create_dir_all(server.join("world")).unwrap();
    std::fs::write(server.join("world/level.dat"), "x").unwrap();
    let download = app
        .get(
            &format!("/api/v1/instances/{id}/worlds/world/download"),
            Some(&cookie),
        )
        .await;
    assert_eq!(json_body(download).await["error"]["code"], "instance_running");

    // Stopping cancels the restart, and then it downloads.
    assert_eq!(post(&app, &id, "stop", &cookie).await.0, StatusCode::ACCEPTED);
    app.wait_for_state(&id, &cookie, "stopped").await;
    let download = app
        .get(
            &format!("/api/v1/instances/{id}/worlds/world/download"),
            Some(&cookie),
        )
        .await;
    assert_eq!(download.status(), StatusCode::OK);
}

/// Starting takes the server's lock for as long as it takes. Before, a job could begin between
/// the start's "is anything running on it?" and the start itself.
#[tokio::test]
async fn a_start_and_a_job_can_not_both_have_the_server() {
    let app = TestApp::new().await;
    let cookie = app.setup_owner().await;
    let id = app.installed_instance("Held", &cookie).await;
    let instance = id.parse().unwrap();

    let held = app.state.instances.hold(instance).unwrap();
    for action in ["start", "restart"] {
        let (status, body) = post(&app, &id, action, &cookie).await;
        assert_eq!(
            (status, body["error"]["code"].as_str()),
            (StatusCode::CONFLICT, Some("instance_busy")),
            "{action}"
        );
    }
    let backup = app
        .post(
            &format!("/api/v1/instances/{id}/backups"),
            json!({ "scope": "full" }),
            Some(&cookie),
        )
        .await;
    assert_eq!(json_body(backup).await["error"]["code"], "instance_busy");
    let delete = app
        .delete(&format!("/api/v1/instances/{id}"), Some(&cookie))
        .await;
    assert_eq!(json_body(delete).await["error"]["code"], "instance_busy");
    assert_eq!(state(&app, &id, &cookie).await["status"], "stopped");

    drop(held);
    assert_eq!(post(&app, &id, "start", &cookie).await.0, StatusCode::ACCEPTED);
    app.wait_for_state(&id, &cookie, "running").await;
    assert!(
        app.state.instances.hold(instance).is_ok(),
        "the start let go of the server once it was under way"
    );
    // A running server can still be stopped and killed while something holds it.
    let _held = app.state.instances.hold(instance).unwrap();
    assert_eq!(post(&app, &id, "stop", &cookie).await.0, StatusCode::ACCEPTED);
    app.wait_for_state(&id, &cookie, "stopped").await;
}

#[tokio::test]
async fn the_server_a_copy_is_made_from_is_held_and_must_be_stopped() {
    let app = TestApp::new().await;
    let cookie = app.setup_owner().await;
    let id = app.installed_instance("Source", &cookie).await;
    let instance = id.parse().unwrap();
    let clone_url = format!("/api/v1/instances/{id}/clone");
    let clone = |name: &'static str| {
        app.post(
            &clone_url,
            json!({ "name": name, "include_worlds": true }),
            Some(&cookie),
        )
    };
    let count = || async {
        json_body(app.get("/api/v1/instances", Some(&cookie)).await)
            .await
            .as_array()
            .unwrap()
            .len()
    };

    // Something else is working on the source: no copy, and no half-made server left behind.
    let held = app.state.instances.hold(instance).unwrap();
    let refused = clone("While busy").await;
    assert_eq!(json_body(refused).await["error"]["code"], "instance_busy");
    assert_eq!(count().await, 1);
    drop(held);

    // A restart is due: the copy cancels it rather than copy a server that is coming up.
    crashed_with_a_restart_pending(&app, &id, &cookie).await;
    let copied = clone("After a crash").await;
    assert_eq!(copied.status(), StatusCode::ACCEPTED);
    let job = finished(&app, &cookie, &json_body(copied).await).await;
    assert_eq!(job["status"], "succeeded", "{job}");
    assert_eq!(state(&app, &id, &cookie).await["status"], "stopped");
    assert!(
        app.state.instances.hold(instance).is_ok(),
        "the source is free again once the copy is done"
    );

    assert_eq!(post(&app, &id, "start", &cookie).await.0, StatusCode::ACCEPTED);
    app.wait_for_state(&id, &cookie, "running").await;
    let refused = clone("While running").await;
    assert_eq!(json_body(refused).await["error"]["code"], "instance_running");
    assert_eq!(count().await, 2);
    assert_eq!(post(&app, &id, "stop", &cookie).await.0, StatusCode::ACCEPTED);
    app.wait_for_state(&id, &cookie, "stopped").await;
}

#[tokio::test]
async fn a_server_with_a_restart_due_is_deleted_without_coming_back() {
    let app = TestApp::new().await;
    let cookie = app.setup_owner().await;
    let id = app.installed_instance("Going", &cookie).await;
    crashed_with_a_restart_pending(&app, &id, &cookie).await;

    let deleted = app
        .delete(&format!("/api/v1/instances/{id}"), Some(&cookie))
        .await;
    assert_eq!(deleted.status(), StatusCode::NO_CONTENT);
    assert!(
        app.state
            .instances
            .supervisors()
            .get(id.parse().unwrap())
            .is_none()
    );
    assert!(!app.state.instances.is_busy(id.parse().unwrap()));
}

/// Stop means stop: asked for while a restart is stopping the server, the server stays down.
#[tokio::test]
async fn a_stop_right_after_a_restart_wins() {
    let app = TestApp::new().await;
    let cookie = app.setup_owner().await;
    let id = app.installed_instance("Restarting", &cookie).await;
    assert_eq!(post(&app, &id, "start", &cookie).await.0, StatusCode::ACCEPTED);
    app.wait_for_state(&id, &cookie, "running").await;

    let handle = app
        .state
        .instances
        .supervisors()
        .get(id.parse().unwrap())
        .unwrap();
    // Both reach the supervisor before the server has had time to exit.
    let (restarted, stopped) = tokio::join!(app.state.instances.restart(id.parse().unwrap()), async {
        handle
            .wait_for(Duration::from_secs(3), |state| state.name() == "stopping")
            .await;
        handle.stop().await
    });
    restarted.unwrap();
    // The fake server exits at once, so the stop may arrive after it is back up; either way it
    // is a stop, and the server ends up stopped.
    stopped.unwrap();
    app.wait_for_state(&id, &cookie, "stopped").await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(state(&app, &id, &cookie).await["status"], "stopped");
}
