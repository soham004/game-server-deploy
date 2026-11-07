//! Cloning and importing servers through the API.

#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use axum::http::StatusCode;
use common::{TestApp, json_body};
use gsd_core::auth::GlobalRole;
use serde_json::{Value, json};

async fn installed(app: &TestApp, cookie: &str) -> Value {
    let created = json_body(
        app.post(
            "/api/v1/instances",
            json!({ "name": "Original", "mc_version": "26.3", "loader": "vanilla", "max_memory_mb": 1024, "eula_accepted": true }),
            Some(cookie),
        )
        .await,
    )
    .await;
    let job = app
        .wait_for_job(created["job_id"].as_str().unwrap(), cookie)
        .await;
    assert_eq!(job["status"], "succeeded");
    created["instance"].clone()
}

#[tokio::test]
async fn clones_a_stopped_server_and_refuses_a_running_one() {
    let app = TestApp::new().await;
    let cookie = app.setup_owner().await;
    let original = installed(&app, &cookie).await;
    let id = original["id"].as_str().unwrap();
    let dir = app
        .state
        .config
        .instances_dir()
        .join(original["slug"].as_str().unwrap())
        .join("server");
    std::fs::create_dir_all(dir.join("world")).unwrap();
    std::fs::write(dir.join("world/level.dat"), "world").unwrap();

    let response = app
        .post(
            &format!("/api/v1/instances/{id}/clone"),
            json!({ "name": "Copy", "include_worlds": true }),
            Some(&cookie),
        )
        .await;
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    let created = json_body(response).await;
    let job = app
        .wait_for_job(created["job_id"].as_str().unwrap(), &cookie)
        .await;
    assert_eq!(job["status"], "succeeded", "{job}");
    let copy_dir = app
        .state
        .config
        .instances_dir()
        .join(created["instance"]["slug"].as_str().unwrap())
        .join("server");
    assert_eq!(
        std::fs::read_to_string(copy_dir.join("world/level.dat")).unwrap(),
        "world"
    );
    assert_ne!(created["instance"]["port"], original["port"]);

    // The copy starts like any server.
    let copy_id = created["instance"]["id"].as_str().unwrap();
    app.post(
        &format!("/api/v1/instances/{copy_id}/start"),
        json!({}),
        Some(&cookie),
    )
    .await;
    app.wait_for_state(copy_id, &cookie, "running").await;

    let running = app
        .post(
            &format!("/api/v1/instances/{copy_id}/clone"),
            json!({ "name": "Copy 2" }),
            Some(&cookie),
        )
        .await;
    assert_eq!(running.status(), StatusCode::CONFLICT);
    assert_eq!(json_body(running).await["error"]["code"], "instance_running");
    app.post(
        &format!("/api/v1/instances/{copy_id}/kill"),
        json!({}),
        Some(&cookie),
    )
    .await;
}

#[tokio::test]
async fn imports_a_folder_and_it_runs() {
    let app = TestApp::new().await;
    let cookie = app.setup_owner().await;
    let imports = app.state.config.imports_dir();
    std::fs::create_dir_all(imports.join("old-server/world")).unwrap();
    std::fs::write(imports.join("old-server/server.jar"), "jar").unwrap();
    std::fs::write(imports.join("old-server/world/level.dat"), "world").unwrap();

    let view = json_body(app.get("/api/v1/imports", Some(&cookie)).await).await;
    assert_eq!(view["folder"], imports.display().to_string());
    assert_eq!(view["candidates"][0]["folder"], "old-server");
    assert_eq!(view["candidates"][0]["detected"]["loader"], "vanilla");
    assert_eq!(view["candidates"][0]["detected"]["jar"], "server.jar");

    let response = app
        .post(
            "/api/v1/imports",
            json!({
                "folder": "old-server", "name": "Old server", "mc_version": "26.3", "loader": "vanilla",
                "jar": "server.jar", "max_memory_mb": 1024, "eula_accepted": true
            }),
            Some(&cookie),
        )
        .await;
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    let created = json_body(response).await;
    let job = app
        .wait_for_job(created["job_id"].as_str().unwrap(), &cookie)
        .await;
    assert_eq!(job["status"], "succeeded", "{job}");
    let id = created["instance"]["id"].as_str().unwrap();
    app.post(&format!("/api/v1/instances/{id}/start"), json!({}), Some(&cookie))
        .await;
    app.wait_for_state(id, &cookie, "running").await;
    app.post(&format!("/api/v1/instances/{id}/kill"), json!({}), Some(&cookie))
        .await;

    let missing = app
        .post(
            "/api/v1/imports",
            json!({ "folder": "../instances", "name": "x", "mc_version": "26.3", "loader": "vanilla", "max_memory_mb": 1024, "eula_accepted": true }),
            Some(&cookie),
        )
        .await;
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    assert_eq!(json_body(missing).await["error"]["code"], "import_not_found");
}

#[tokio::test]
async fn members_can_not_clone_or_import() {
    let app = TestApp::new().await;
    let owner = app.setup_owner().await;
    let original = installed(&app, &owner).await;
    app.create_user("member", GlobalRole::Member).await;
    let member = app.login("member").await;
    assert_eq!(
        app.get("/api/v1/imports", Some(&member)).await.status(),
        StatusCode::FORBIDDEN
    );
    let clone = app
        .post(
            &format!("/api/v1/instances/{}/clone", original["id"].as_str().unwrap()),
            json!({ "name": "x" }),
            Some(&member),
        )
        .await;
    assert_eq!(clone.status(), StatusCode::FORBIDDEN);
}
