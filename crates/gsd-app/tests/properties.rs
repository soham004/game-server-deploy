//! The `server.properties` editor API: reading with the schema, changing values and the raw file,
//! conflicts, validation errors, app-managed keys, and who may see and change what.

#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use axum::http::StatusCode;
use common::{TestApp, json_body};
use gsd_core::{
    auth::{GlobalRole, Grant},
    ids::InstanceId,
};
use serde_json::{Value, json};

/// A real 26.3 server's generated file, as the test instance's `server.properties`.
const GENERATED: &str = include_str!("../../gsd-minecraft/tests/fixtures/server_26.3.properties");

async fn instance_with_generated_file(app: &TestApp, cookie: &str) -> (String, std::path::PathBuf) {
    let response = app
        .post(
            "/api/v1/instances",
            json!({ "name": "Props", "mc_version": "26.3", "loader": "vanilla", "max_memory_mb": 1024, "eula_accepted": true }),
            Some(cookie),
        )
        .await;
    let created = json_body(response).await;
    let job = app
        .wait_for_job(created["job_id"].as_str().unwrap(), cookie)
        .await;
    assert_eq!(job["status"], "succeeded");
    let id = created["instance"]["id"].as_str().unwrap().to_owned();
    let port = created["instance"]["port"].as_u64().unwrap();
    let path = app
        .state
        .config
        .instances_dir()
        .join(created["instance"]["slug"].as_str().unwrap())
        .join("server/server.properties");
    let text = GENERATED
        .replace("server-port=25565", &format!("server-port={port}"))
        .replace("query.port=25565", &format!("query.port={port}"))
        .replace("rcon.password=", "rcon.password=hunter2");
    std::fs::write(&path, text).unwrap();
    (id, path)
}

fn url(id: &str) -> String {
    format!("/api/v1/instances/{id}/properties")
}

#[tokio::test]
async fn reads_the_file_with_values_and_schema() {
    let app = TestApp::new().await;
    let cookie = app.setup_owner().await;
    let (id, _) = instance_with_generated_file(&app, &cookie).await;

    let body = json_body(app.get(&url(&id), Some(&cookie)).await).await;
    assert_eq!(body["exists"], true);
    assert_eq!(body["can_edit"], true);
    assert_eq!(body["restart_required"], false);
    assert_eq!(body["revision"].as_str().unwrap().len(), 64);
    assert!(
        body["text"]
            .as_str()
            .unwrap()
            .starts_with("#Minecraft server properties")
    );
    let value = |key: &str| {
        body["values"]
            .as_array()
            .unwrap()
            .iter()
            .find(|kv| kv["key"] == key)
            .map(|kv| kv["value"].clone())
    };
    assert_eq!(
        value("level-type"),
        Some(json!("minecraft:normal")),
        "escapes are decoded"
    );
    assert_eq!(
        value("rcon.password"),
        Some(json!("hunter2")),
        "owners see secrets"
    );

    let schema = body["schema"].as_array().unwrap();
    let difficulty = schema.iter().find(|s| s["key"] == "difficulty").unwrap();
    assert_eq!(difficulty["type"], "choice");
    assert_eq!(difficulty["category"], "general");
    assert!(
        schema
            .iter()
            .any(|s| s["key"] == "server-port" && s["managed"] == true)
    );
}

#[tokio::test]
async fn changes_are_saved_and_only_their_lines_change() {
    let app = TestApp::new().await;
    let cookie = app.setup_owner().await;
    let (id, path) = instance_with_generated_file(&app, &cookie).await;
    let before = std::fs::read_to_string(&path).unwrap();
    let revision = json_body(app.get(&url(&id), Some(&cookie)).await).await["revision"].clone();

    let response = app
        .put(
            &url(&id),
            json!({ "revision": revision, "changes": { "max-players": "8", "motd": "Weekend SMP", "custom.plugin-key": "on" } }),
            Some(&cookie),
        )
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;
    assert_ne!(body["revision"], revision);

    let after = std::fs::read_to_string(&path).unwrap();
    let expected = before
        .replace("max-players=20", "max-players=8")
        .replace("motd=A Minecraft Server", "motd=Weekend SMP")
        + "custom.plugin-key=on\n";
    assert_eq!(after, expected);
}

#[tokio::test]
async fn stale_revisions_get_409_and_invalid_values_get_422_with_details() {
    let app = TestApp::new().await;
    let cookie = app.setup_owner().await;
    let (id, path) = instance_with_generated_file(&app, &cookie).await;
    let revision = json_body(app.get(&url(&id), Some(&cookie)).await).await["revision"].clone();

    let invalid = app
        .put(
            &url(&id),
            json!({ "revision": revision, "changes": { "difficulty": "nightmare", "view-distance": "2", "server-port": "1" } }),
            Some(&cookie),
        )
        .await;
    assert_eq!(invalid.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let error = &json_body(invalid).await["error"];
    assert_eq!(error["code"], "invalid_properties");
    let mut keys: Vec<&str> = error["details"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["key"].as_str().unwrap())
        .collect();
    keys.sort_unstable();
    assert_eq!(keys, ["difficulty", "server-port", "view-distance"]);

    std::fs::write(&path, "server-port=1\n").unwrap(); // someone else changed it
    let stale = app
        .put(
            &url(&id),
            json!({ "revision": revision, "changes": { "motd": "x" } }),
            Some(&cookie),
        )
        .await;
    assert_eq!(stale.status(), StatusCode::CONFLICT);
    assert_eq!(json_body(stale).await["error"]["code"], "file_changed");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "server-port=1\n");
}

#[tokio::test]
async fn the_raw_file_can_be_replaced() {
    let app = TestApp::new().await;
    let cookie = app.setup_owner().await;
    let (id, path) = instance_with_generated_file(&app, &cookie).await;
    let current = json_body(app.get(&url(&id), Some(&cookie)).await).await;
    let typed = current["text"].as_str().unwrap().replace(
        "pause-when-empty-seconds=60",
        "# never pause\npause-when-empty-seconds=0",
    );

    let response = app
        .put(
            &url(&id),
            json!({ "revision": current["revision"], "text": typed }),
            Some(&cookie),
        )
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), typed);

    let neither = app
        .put(&url(&id), json!({ "revision": "x" }), Some(&cookie))
        .await;
    assert_eq!(neither.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let both = app
        .put(
            &url(&id),
            json!({ "revision": "x", "text": "", "changes": {} }),
            Some(&cookie),
        )
        .await;
    assert_eq!(both.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn viewers_see_values_without_secrets_and_cannot_save() {
    let app = TestApp::new().await;
    let owner = app.setup_owner().await;
    let (id, path) = instance_with_generated_file(&app, &owner).await;
    let member = app.create_user("viewer", GlobalRole::Member).await;
    app.state
        .auth
        .grants()
        .set(member, id.parse::<InstanceId>().unwrap(), Grant::Viewer)
        .await
        .unwrap();
    let viewer = app.login("viewer").await;

    let body: Value = json_body(app.get(&url(&id), Some(&viewer)).await).await;
    assert_eq!(body["can_edit"], false);
    assert!(!body["text"].as_str().unwrap().contains("hunter2"));
    assert!(
        !body.to_string().contains("hunter2"),
        "the secret appears nowhere"
    );

    let response = app
        .put(
            &url(&id),
            json!({ "revision": body["revision"], "changes": { "motd": "x" } }),
            Some(&viewer),
        )
        .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert!(
        std::fs::read_to_string(&path)
            .unwrap()
            .contains("motd=A Minecraft Server")
    );

    app.create_user("stranger", GlobalRole::Member).await;
    let stranger = app.login("stranger").await;
    assert_eq!(
        app.get(&url(&id), Some(&stranger)).await.status(),
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn a_running_server_needs_a_restart_to_apply_changes() {
    let app = TestApp::new().await;
    let cookie = app.setup_owner().await;
    let (id, _) = instance_with_generated_file(&app, &cookie).await;
    let started = app
        .post(&format!("/api/v1/instances/{id}/start"), json!({}), Some(&cookie))
        .await;
    assert_eq!(started.status(), StatusCode::ACCEPTED);
    app.wait_for_state(&id, &cookie, "running").await;

    let body = json_body(app.get(&url(&id), Some(&cookie)).await).await;
    assert_eq!(body["restart_required"], true);
    let saved = json_body(
        app.put(
            &url(&id),
            json!({ "revision": body["revision"], "changes": { "motd": "x" } }),
            Some(&cookie),
        )
        .await,
    )
    .await;
    assert_eq!(saved["restart_required"], true);
    app.post(&format!("/api/v1/instances/{id}/kill"), json!({}), Some(&cookie))
        .await;
}
