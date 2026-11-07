//! Instance settings API: reading, changing, validation, permissions, and that changes reach
//! the server's launch command.

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

async fn installed(app: &TestApp, cookie: &str) -> String {
    let created = json_body(
        app.post(
            "/api/v1/instances",
            json!({ "name": "Tuned", "mc_version": "26.3", "loader": "vanilla", "max_memory_mb": 2048, "eula_accepted": true }),
            Some(cookie),
        )
        .await,
    )
    .await;
    let job = app
        .wait_for_job(created["job_id"].as_str().unwrap(), cookie)
        .await;
    assert_eq!(job["status"], "succeeded");
    created["instance"]["id"].as_str().unwrap().to_owned()
}

fn url(id: &str) -> String {
    format!("/api/v1/instances/{id}/settings")
}

async fn patch(app: &TestApp, id: &str, body: Value, cookie: &str) -> axum::response::Response {
    let request = axum::http::Request::patch(url(id))
        .header(axum::http::header::COOKIE, cookie)
        .header("x-csrf", "1")
        .header(axum::http::header::CONTENT_TYPE, "application/json")
        .body(axum::body::Body::from(body.to_string()))
        .unwrap();
    app.send(request).await
}

#[tokio::test]
async fn shows_settings_with_the_runtimes_to_choose_from() {
    let app = TestApp::new().await;
    let cookie = app.setup_owner().await;
    let id = installed(&app, &cookie).await;
    let view = json_body(app.get(&url(&id), Some(&cookie)).await).await;
    assert_eq!(view["name"], "Tuned");
    assert_eq!(view["java"]["max_memory_mb"], 2048);
    assert_eq!(view["java"]["runtime"], json!({ "kind": "auto" }));
    assert_eq!(view["behavior"]["auto_restart"], true);
    assert_eq!(
        view["java"]["extra_jvm_args"],
        json!([]),
        "the console flags are not shown"
    );
    assert_eq!(view["can_edit"], true);
    assert_eq!(view["can_use_custom_runtime"], true, "owners may");
    let runtimes = view["runtimes"].as_array().unwrap();
    assert!(
        runtimes
            .iter()
            .any(|r| r["id"] == "temurin-25-x64" && r["managed"] == true),
        "{runtimes:?}"
    );
}

#[tokio::test]
async fn changes_are_saved_and_used_by_the_next_start() {
    let app = TestApp::new().await;
    let cookie = app.setup_owner().await;
    let id = installed(&app, &cookie).await;
    let response = patch(
        &app,
        &id,
        json!({
            "name": "Tuned Up",
            "min_memory_mb": 1536,
            "max_memory_mb": 3072,
            "jvm_preset": "none",
            "extra_jvm_args": ["-Dcustom.flag=on"],
            "java_runtime": { "kind": "runtime", "id": "temurin-25-x64" },
            "auto_start": true,
            "stop_timeout_secs": 30
        }),
        &cookie,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let view = json_body(response).await;
    assert_eq!(view["name"], "Tuned Up");
    assert_eq!(view["java"]["extra_jvm_args"], json!(["-Dcustom.flag=on"]));
    assert_eq!(view["behavior"]["auto_start"], true);
    let listed = json_body(app.get(&format!("/api/v1/instances/{id}"), Some(&cookie)).await).await;
    assert_eq!(listed["name"], "Tuned Up");

    // The next start uses them (the fake java prints its arguments).
    app.post(&format!("/api/v1/instances/{id}/start"), json!({}), Some(&cookie))
        .await;
    app.wait_for_state(&id, &cookie, "running").await;
    let console = json_body(
        app.get(&format!("/api/v1/instances/{id}/console"), Some(&cookie))
            .await,
    )
    .await;
    let launched = console["console"]
        .as_array()
        .unwrap()
        .iter()
        .find_map(|l| l["text"].as_str().filter(|t| t.starts_with("Launched with:")))
        .unwrap()
        .to_owned();
    for expected in [
        "-Xms1536M",
        "-Xmx3072M",
        "-Dcustom.flag=on",
        "-Dterminal.jline=false",
    ] {
        assert!(launched.contains(expected), "{expected} in {launched}");
    }
    assert!(
        !launched.contains("-XX:+UseG1GC"),
        "preset none drops Aikar's flags: {launched}"
    );

    // Saving while running says a restart is needed.
    let running = json_body(patch(&app, &id, json!({ "auto_restart": false }), &cookie).await).await;
    assert_eq!(running["restart_required"], true);
    app.post(&format!("/api/v1/instances/{id}/kill"), json!({}), Some(&cookie))
        .await;
}

#[tokio::test]
async fn invalid_changes_are_refused_and_change_nothing() {
    let app = TestApp::new().await;
    let cookie = app.setup_owner().await;
    let id = installed(&app, &cookie).await;
    let cases = [
        (json!({ "max_memory_mb": 100 }), "invalid_settings"),
        (
            json!({ "min_memory_mb": 4096, "max_memory_mb": 2048 }),
            "invalid_settings",
        ),
        (json!({ "name": "" }), "invalid_settings"),
        (json!({ "extra_jvm_args": ["-Xmx16G"] }), "invalid_settings"),
        (
            json!({ "extra_jvm_args": ["-Djava.net.preferIPv6Addresses=system"] }),
            "invalid_settings",
        ),
        (
            json!({ "java_runtime": { "kind": "runtime", "id": "no-such-java" } }),
            "unknown_runtime",
        ),
        (json!({ "unknown_field": true }), "invalid_request_body"),
    ];
    for (body, code) in cases {
        let response = patch(&app, &id, body.clone(), &cookie).await;
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY, "{body}");
        assert_eq!(json_body(response).await["error"]["code"], code, "{body}");
    }
    let view = json_body(app.get(&url(&id), Some(&cookie)).await).await;
    assert_eq!(view["name"], "Tuned");
    assert_eq!(view["java"]["max_memory_mb"], 2048);
}

#[tokio::test]
async fn only_users_with_the_settings_permission_may_change_them() {
    let app = TestApp::new().await;
    let owner = app.setup_owner().await;
    let id = installed(&app, &owner).await;
    let instance: InstanceId = id.parse().unwrap();

    let viewer_id = app.create_user("viewer", GlobalRole::Member).await;
    app.state
        .auth
        .grants()
        .set(viewer_id, instance, Grant::Viewer)
        .await
        .unwrap();
    let viewer = app.login("viewer").await;
    let view = json_body(app.get(&url(&id), Some(&viewer)).await).await;
    assert_eq!(view["can_edit"], false);
    assert_eq!(view["can_use_custom_runtime"], false);
    assert_eq!(
        patch(&app, &id, json!({ "auto_start": true }), &viewer)
            .await
            .status(),
        StatusCode::FORBIDDEN
    );

    let manager_id = app.create_user("manager", GlobalRole::Member).await;
    app.state
        .auth
        .grants()
        .set(manager_id, instance, Grant::Manager)
        .await
        .unwrap();
    let manager = app.login("manager").await;
    assert_eq!(
        patch(&app, &id, json!({ "auto_start": true }), &manager)
            .await
            .status(),
        StatusCode::OK
    );

    let audit: Vec<String> =
        sqlx::query_scalar("SELECT details_json FROM audit_log WHERE action = 'instance.settings_changed'")
            .fetch_all(&app.pool)
            .await
            .unwrap();
    assert_eq!(audit, [r#"{"change":["auto_start"]}"#]);
}
