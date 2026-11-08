//! Changing a server's version: upgrades keep the world and take a full backup first,
//! downgrades must be confirmed, loader builds change within a version, and requests that make
//! no sense are refused before anything happens.

#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use axum::http::StatusCode;
use common::{TestApp, json_body};
use gsd_core::ids::InstanceId;
use serde_json::{Value, json};

async fn create(app: &TestApp, body: Value, cookie: &str) -> String {
    let created = json_body(app.post("/api/v1/instances", body, Some(cookie)).await).await;
    let job = app
        .wait_for_job(created["job_id"].as_str().unwrap(), cookie)
        .await;
    assert_eq!(job["status"], "succeeded", "{job}");
    created["instance"]["id"].as_str().unwrap().to_owned()
}

fn server(name: &str, mc: &str, loader: &str) -> Value {
    json!({ "name": name, "mc_version": mc, "loader": loader, "max_memory_mb": 1024, "eula_accepted": true })
}

/// Starts a change and waits for its job.
async fn change(app: &TestApp, id: &str, body: Value, cookie: &str) -> Value {
    let response = app
        .post(
            &format!("/api/v1/instances/{id}/change-version"),
            body,
            Some(cookie),
        )
        .await;
    let status = response.status();
    let started = json_body(response).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{started}");
    app.wait_for_job(started["job_id"].as_str().unwrap(), cookie)
        .await
}

async fn instance(app: &TestApp, id: &str, cookie: &str) -> Value {
    json_body(app.get(&format!("/api/v1/instances/{id}"), Some(cookie)).await).await
}

#[tokio::test]
async fn an_upgrade_keeps_the_world_and_takes_a_full_backup_first() {
    let app = TestApp::new().await;
    let cookie = app.setup_owner().await;
    let id = create(&app, server("Upgraded", "26.2", "vanilla"), &cookie).await;
    let dir = app
        .state
        .instances
        .get(id.parse::<InstanceId>().unwrap())
        .await
        .unwrap()
        .layout
        .server_dir();
    std::fs::create_dir_all(dir.join("world")).unwrap();
    std::fs::write(dir.join("world/level.dat"), "my world").unwrap();
    std::fs::write(dir.join("server.properties"), "motd=kept\n").unwrap();

    let job = change(&app, &id, json!({ "mc_version": "26.3" }), &cookie).await;

    assert_eq!(job["status"], "succeeded", "{job}");
    let upgraded = instance(&app, &id, &cookie).await;
    assert_eq!(upgraded["software"], "Vanilla 26.3");
    assert_eq!(upgraded["server"]["mc_version"], "26.3");
    assert_eq!(upgraded["installed"], true);
    assert_eq!(
        std::fs::read_to_string(dir.join("world/level.dat")).unwrap(),
        "my world"
    );
    assert!(
        std::fs::read_to_string(dir.join("server.properties"))
            .unwrap()
            .contains("motd=kept")
    );

    let backups = json_body(
        app.get(&format!("/api/v1/instances/{id}/backups"), Some(&cookie))
            .await,
    )
    .await;
    let safety = &backups["backups"][0];
    assert_eq!(
        (safety["kind"].as_str(), safety["scope"].as_str()),
        (Some("pre_version_change"), Some("full"))
    );
    assert_eq!(safety["software"], "Vanilla 26.2", "taken before the change");
    assert_eq!(
        safety["note"],
        "Before changing from Vanilla 26.2 to Vanilla 26.3"
    );
    let log = job["log"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(Value::as_str)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        log.contains("Backing up everything first") && log.contains("Now on Vanilla 26.3"),
        "{log}"
    );
}

#[tokio::test]
async fn a_downgrade_must_be_confirmed() {
    let app = TestApp::new().await;
    let cookie = app.setup_owner().await;
    let id = create(&app, server("Downgraded", "26.3", "vanilla"), &cookie).await;

    let response = app
        .post(
            &format!("/api/v1/instances/{id}/change-version"),
            json!({ "mc_version": "26.2" }),
            Some(&cookie),
        )
        .await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(
        json_body(response).await["error"]["code"],
        "downgrade_not_confirmed"
    );
    assert_eq!(
        instance(&app, &id, &cookie).await["server"]["mc_version"],
        "26.3",
        "nothing changed"
    );

    let job = change(
        &app,
        &id,
        json!({ "mc_version": "26.2", "allow_downgrade": true }),
        &cookie,
    )
    .await;
    assert_eq!(job["status"], "succeeded", "{job}");
    assert_eq!(instance(&app, &id, &cookie).await["software"], "Vanilla 26.2");
}

#[tokio::test]
async fn a_loader_build_changes_within_the_same_minecraft_version() {
    let app = TestApp::new().await;
    let cookie = app.setup_owner().await;
    let mut body = server("Paper", "26.3", "paper");
    body["loader_version"] = json!("241");
    let id = create(&app, body, &cookie).await;
    assert_eq!(
        instance(&app, &id, &cookie).await["server"]["loader_version"],
        "241"
    );

    // No loader version: the recommended (newest stable) build.
    let job = change(&app, &id, json!({ "mc_version": "26.3" }), &cookie).await;

    assert_eq!(job["status"], "succeeded", "{job}");
    assert_eq!(
        instance(&app, &id, &cookie).await["server"]["loader_version"],
        "244"
    );
}

#[tokio::test]
async fn senseless_or_unsafe_requests_are_refused_before_anything_happens() {
    let app = TestApp::new().await;
    let cookie = app.setup_owner().await;
    let id = create(&app, server("Steady", "26.3", "vanilla"), &cookie).await;
    let url = format!("/api/v1/instances/{id}/change-version");

    let same = app
        .post(&url, json!({ "mc_version": "26.3" }), Some(&cookie))
        .await;
    assert_eq!(same.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(json_body(same).await["error"]["code"], "version_unchanged");
    let unknown = app
        .post(&url, json!({ "mc_version": "99.9" }), Some(&cookie))
        .await;
    assert_eq!(json_body(unknown).await["error"]["code"], "unknown_version");
    let typo = app
        .post(&url, json!({ "mc_versoin": "26.2" }), Some(&cookie))
        .await;
    assert_eq!(typo.status(), StatusCode::UNPROCESSABLE_ENTITY);

    let started = app
        .post(&format!("/api/v1/instances/{id}/start"), json!({}), Some(&cookie))
        .await;
    assert!(started.status().is_success());
    app.wait_for_state(&id, &cookie, "running").await;
    let running = app
        .post(
            &url,
            json!({ "mc_version": "26.2", "allow_downgrade": true }),
            Some(&cookie),
        )
        .await;
    assert_eq!(running.status(), StatusCode::CONFLICT);
    assert_eq!(json_body(running).await["error"]["code"], "instance_running");

    let backups = json_body(
        app.get(&format!("/api/v1/instances/{id}/backups"), Some(&cookie))
            .await,
    )
    .await;
    assert_eq!(
        backups["backups"],
        json!([]),
        "no safety backup for refused requests"
    );
}
