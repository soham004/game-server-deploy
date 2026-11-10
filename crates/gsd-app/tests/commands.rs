//! Console autocomplete through the API: the data generator run as a job (the fake Java hands out the
//! real 26.3 reports), a failed one and a retry, a modded server's `help` learned while it runs, and
//! who may use it.

#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::path::PathBuf;

use axum::http::StatusCode;
use common::{TestApp, json_body};
use gsd_core::auth::{GlobalRole, Grant};
use serde_json::{Value, json};

async fn status(app: &TestApp, id: &str, cookie: &str) -> Value {
    json_body(
        app.get(&format!("/api/v1/instances/{id}/commands"), Some(cookie))
            .await,
    )
    .await
}

async fn prepare(app: &TestApp, id: &str, cookie: &str, retry: bool) -> Value {
    let response = app
        .post(
            &format!("/api/v1/instances/{id}/commands/prepare"),
            json!({ "retry": retry }),
            Some(cookie),
        )
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    json_body(response).await
}

async fn complete(app: &TestApp, id: &str, cookie: &str, input: &str) -> Value {
    let query = format!("input={}", urlencode(input));
    json_body(
        app.get(
            &format!("/api/v1/instances/{id}/commands/complete?{query}"),
            Some(cookie),
        )
        .await,
    )
    .await
}

fn urlencode(text: &str) -> String {
    text.bytes()
        .map(|byte| match byte {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' | b'.' => (byte as char).to_string(),
            other => format!("%{other:02X}"),
        })
        .collect()
}

fn texts(completion: &Value) -> Vec<String> {
    completion["suggestions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["text"].as_str().unwrap().to_owned())
        .collect()
}

/// Prepares and waits for the generator's job; returns the status after it.
async fn generated(app: &TestApp, id: &str, cookie: &str) -> Value {
    let preparing = prepare(app, id, cookie, false).await;
    assert_eq!(preparing["vanilla"]["state"], "preparing", "{preparing}");
    let job = app
        .wait_for_job(preparing["vanilla"]["job_id"].as_str().unwrap(), cookie)
        .await;
    assert_eq!(job["kind"], "commands.prepare");
    assert_eq!(job["status"], "succeeded", "{job}");
    status(app, id, cookie).await
}

async fn instance_root(app: &TestApp, id: &str) -> PathBuf {
    let id = id.parse().unwrap();
    app.state
        .instances
        .get(id)
        .await
        .unwrap()
        .layout
        .root()
        .to_owned()
}

async fn start(app: &TestApp, id: &str, cookie: &str) {
    let started = app
        .post(&format!("/api/v1/instances/{id}/start"), json!({}), Some(cookie))
        .await;
    assert!(started.status().is_success(), "{:?}", started.status());
    app.wait_for_state(id, cookie, "running").await;
}

fn fake_java_home(app: &TestApp) -> PathBuf {
    app.state.config.runtimes_dir().join("temurin-25-x64")
}

#[tokio::test]
async fn a_vanilla_servers_commands_come_from_its_data_generator() {
    let app = TestApp::new().await;
    let cookie = app.setup_owner().await;
    let id = app.installed_instance("Vanilla", &cookie).await;

    let before = status(&app, &id, &cookie).await;
    assert_eq!(before["vanilla"]["state"], "missing");
    assert_eq!(before["learned"]["state"], "not_needed", "vanilla needs no help");
    assert_eq!(
        texts(&complete(&app, &id, &cookie, "gi").await),
        Vec::<String>::new()
    );

    let ready = generated(&app, &id, &cookie).await;
    assert_eq!(
        ready["vanilla"],
        json!({ "state": "ready", "mc_version": "26.3", "commands": 94 })
    );
    let root = instance_root(&app, &id).await;
    assert!(root.join("commands/vanilla.json").is_file());
    let leftovers: Vec<_> = std::fs::read_dir(root.join("tmp"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(
        leftovers,
        Vec::<std::ffi::OsString>::new(),
        "the work folder is removed"
    );
    // Preparing again starts nothing.
    assert_eq!(
        prepare(&app, &id, &cookie, false).await["vanilla"]["state"],
        "ready"
    );

    let give = complete(&app, &id, &cookie, "give @p dia").await;
    assert_eq!(&texts(&give)[..2], ["diamond", "diamond_axe"]);
    assert_eq!(give["suggestions"][0]["from"], 8);
    assert_eq!(
        give["signature"]["parts"][2],
        json!({ "text": "<item>", "current": true })
    );
    // Data-driven ids come from the jar itself.
    assert_eq!(
        texts(&complete(&app, &id, &cookie, "enchant @p sha").await),
        ["sharpness"]
    );

    // Who's online.
    start(&app, &id, &cookie).await;
    let sent = app
        .post(
            &format!("/api/v1/instances/{id}/command"),
            json!({ "command": "join Steve" }),
            Some(&cookie),
        )
        .await;
    assert_eq!(sent.status(), StatusCode::NO_CONTENT);
    let mut players = Vec::new();
    for _ in 0..100 {
        players = texts(&complete(&app, &id, &cookie, "give St").await);
        if !players.is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert_eq!(players, ["Steve"]);
}

#[tokio::test]
async fn a_failed_generator_is_reported_until_tried_again() {
    let app = TestApp::new().await;
    let cookie = app.setup_owner().await;
    let id = app.installed_instance("Failing", &cookie).await;
    let fail = fake_java_home(&app).join("fail-datagen");
    std::fs::write(&fail, "").unwrap();

    let preparing = prepare(&app, &id, &cookie, false).await;
    let job = app
        .wait_for_job(preparing["vanilla"]["job_id"].as_str().unwrap(), &cookie)
        .await;
    assert_eq!(job["status"], "failed");
    let failed = status(&app, &id, &cookie).await;
    assert_eq!(failed["vanilla"]["state"], "unavailable");
    assert!(
        failed["vanilla"]["reason"]
            .as_str()
            .unwrap()
            .contains("exit code 5"),
        "{failed}"
    );
    // Not tried again by itself.
    assert_eq!(
        prepare(&app, &id, &cookie, false).await["vanilla"]["state"],
        "unavailable"
    );
    // A vanilla server without the generator's tree learns from help instead.
    assert_eq!(failed["learned"]["state"], "stale");

    std::fs::remove_file(&fail).unwrap();
    let retried = prepare(&app, &id, &cookie, true).await;
    assert_eq!(retried["vanilla"]["state"], "preparing");
    app.wait_for_job(retried["vanilla"]["job_id"].as_str().unwrap(), &cookie)
        .await;
    assert_eq!(status(&app, &id, &cookie).await["vanilla"]["state"], "ready");
}

#[tokio::test]
async fn a_modded_servers_own_commands_are_learned_from_help_while_it_runs() {
    let app = TestApp::new().await;
    let cookie = app.setup_owner().await;
    let body = json!({ "name": "Modded", "mc_version": "26.3", "loader": "fabric", "max_memory_mb": 1024, "eula_accepted": true });
    let created = json_body(app.post("/api/v1/instances", body, Some(&cookie)).await).await;
    let job = app
        .wait_for_job(created["job_id"].as_str().unwrap(), &cookie)
        .await;
    assert_eq!(job["status"], "succeeded", "{job}");
    let id = created["instance"]["id"].as_str().unwrap().to_owned();

    // Stopped: the generator runs; help waits for the server.
    let ready = generated(&app, &id, &cookie).await;
    assert_eq!(ready["vanilla"]["state"], "ready");
    assert_eq!(ready["learned"]["state"], "stale");

    start(&app, &id, &cookie).await;
    let learning = prepare(&app, &id, &cookie, false).await;
    assert!(
        ["learning", "current"].contains(&learning["learned"]["state"].as_str().unwrap()),
        "{learning}"
    );
    let mut learned = Value::Null;
    for _ in 0..200 {
        learned = status(&app, &id, &cookie).await;
        if learned["learned"]["state"] == "current" {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert_eq!(learned["learned"]["state"], "current", "{learned}");
    // `give` and `xp` are the generator's already: only the mod's command is new.
    assert_eq!(learned["learned_commands"], 1);
    assert_eq!(texts(&complete(&app, &id, &cookie, "chunky st").await), ["start"]);
    let console = json_body(
        app.get(&format!("/api/v1/instances/{id}/console"), Some(&cookie))
            .await,
    )
    .await;
    assert!(
        console["console"]
            .as_array()
            .unwrap()
            .iter()
            .any(|line| line["text"] == "[autocomplete] > help"),
        "help shows in the console as the app's"
    );
    let saved: Value = serde_json::from_slice(
        &std::fs::read(instance_root(&app, &id).await.join("commands/learned.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(saved["commands"].as_array().unwrap().len(), 3);

    // Another mod: asked again.
    let mods = instance_root(&app, &id).await.join("server/mods");
    std::fs::create_dir_all(&mods).unwrap();
    std::fs::write(mods.join("another.jar"), "jar").unwrap();
    assert_eq!(status(&app, &id, &cookie).await["learned"]["state"], "stale");
}

#[tokio::test]
async fn only_those_who_may_type_commands_get_completions() {
    let app = TestApp::new().await;
    let cookie = app.setup_owner().await;
    let id = app.installed_instance("Rights", &cookie).await;
    let viewer = app.create_user("viewer", GlobalRole::Member).await;
    app.state
        .auth
        .grants()
        .set(viewer, id.parse().unwrap(), Grant::Viewer)
        .await
        .unwrap();
    let viewer = app.login("viewer").await;

    for path in ["commands", "commands/complete?input=gi"] {
        let response = app
            .get(&format!("/api/v1/instances/{id}/{path}"), Some(&viewer))
            .await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN, "{path}");
        let response = app.get(&format!("/api/v1/instances/{id}/{path}"), None).await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "{path}");
    }
    let response = app
        .post(
            &format!("/api/v1/instances/{id}/commands/prepare"),
            json!({}),
            Some(&viewer),
        )
        .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let long = "a".repeat(40_000);
    let response = app
        .get(
            &format!("/api/v1/instances/{id}/commands/complete?input={long}"),
            Some(&cookie),
        )
        .await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
}
