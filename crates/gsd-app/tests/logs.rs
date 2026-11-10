//! The log history API: which files count as logs, reading them (compressed ones too), search and
//! levels, paging, crash report summaries, and that this right reaches logs and nothing else.

#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::{io::Write as _, path::PathBuf};

use axum::http::StatusCode;
use common::{TestApp, json_body, text_body};
use flate2::{Compression, write::GzEncoder};
use gsd_core::auth::{GlobalRole, Grant};
use serde_json::{Value, json};

const LOG: &str = "[12:00:00] [main/INFO]: Starting
[12:00:01] [main/WARN]: Careful
[12:00:02] [main/ERROR]: Broke
java.lang.Error: boom
[12:00:03] [main/INFO]: Done
";

const CRASH: &str = "---- Minecraft Crash Report ----
Time: 2026-11-09 12:30:01
Description: Watching Server

java.lang.Error: Watchdog
\tat com.example.mod.Thing.run(Thing.java:1)
";

async fn server_with_logs(app: &TestApp, cookie: &str) -> (String, PathBuf) {
    let id = app.installed_instance("Logged", cookie).await;
    let parsed = id.parse().unwrap();
    let instance = app.state.instances.registry().get(parsed).await.unwrap();
    let dir = instance.layout.server_dir();
    std::fs::create_dir_all(dir.join("logs")).unwrap();
    std::fs::create_dir_all(dir.join("crash-reports")).unwrap();
    std::fs::write(dir.join("logs/latest.log"), LOG).unwrap();
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(b"[11:00:00] [main/INFO]: Yesterday\n").unwrap();
    std::fs::write(dir.join("logs/2026-11-08-1.log.gz"), encoder.finish().unwrap()).unwrap();
    std::fs::write(dir.join("crash-reports/crash-1-server.txt"), CRASH).unwrap();
    std::fs::write(
        dir.join("hs_err_pid42.log"),
        "#\n# A fatal error has been detected by the Java Runtime Environment:\n#\n#  SIGSEGV (0xb)\n#\n",
    )
    .unwrap();
    std::fs::write(dir.join("server.properties"), "rcon.password=secret\n").unwrap();
    std::fs::write(dir.join("logs/notes.txt"), "not a log").unwrap();
    (id, dir)
}

async fn read(app: &TestApp, id: &str, name_and_query: &str, cookie: &str) -> Value {
    let response = app
        .get(
            &format!("/api/v1/instances/{id}/logs/{name_and_query}"),
            Some(cookie),
        )
        .await;
    let status = response.status();
    let body = json_body(response).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body
}

fn texts(page: &Value) -> Vec<&str> {
    page["lines"]
        .as_array()
        .unwrap()
        .iter()
        .map(|line| line["text"].as_str().unwrap())
        .collect()
}

#[tokio::test]
async fn logs_are_listed_with_what_they_are() {
    let app = TestApp::new().await;
    let cookie = app.setup_owner().await;
    let (id, _) = server_with_logs(&app, &cookie).await;

    let list = json_body(
        app.get(&format!("/api/v1/instances/{id}/logs"), Some(&cookie))
            .await,
    )
    .await;
    let found: Vec<(&str, &str)> = list
        .as_array()
        .unwrap()
        .iter()
        .map(|file| (file["name"].as_str().unwrap(), file["kind"].as_str().unwrap()))
        .collect();
    assert_eq!(found[0], ("latest.log", "latest"), "{list}");
    assert_eq!(found.len(), 4, "{list}");
    for wanted in [
        ("2026-11-08-1.log.gz", "archive"),
        ("crash-1-server.txt", "crash"),
        ("hs_err_pid42.log", "jvm_crash"),
    ] {
        assert!(found.contains(&wanted), "{wanted:?} in {list}");
    }
    assert!(list[0]["size"].as_u64().unwrap() > 0);
}

#[tokio::test]
async fn a_server_that_has_never_run_has_no_logs() {
    let app = TestApp::new().await;
    let cookie = app.setup_owner().await;
    let id = app.installed_instance("Fresh", &cookie).await;
    let list = json_body(
        app.get(&format!("/api/v1/instances/{id}/logs"), Some(&cookie))
            .await,
    )
    .await;
    assert_eq!(list, json!([]));
}

#[tokio::test]
async fn reading_filters_pages_and_decompresses() {
    let app = TestApp::new().await;
    let cookie = app.setup_owner().await;
    let (id, _) = server_with_logs(&app, &cookie).await;

    let whole = read(&app, &id, "latest.log", &cookie).await;
    assert_eq!(texts(&whole).len(), 5);
    assert_eq!(whole["lines"][2]["level"], "error");
    assert_eq!(
        whole["lines"][3]["level"], "error",
        "a stack trace takes the level above it"
    );
    assert_eq!(whole["earlier"], Value::Null);

    let errors = read(&app, &id, "latest.log?level=error", &cookie).await;
    assert_eq!(texts(&errors).len(), 2, "{errors}");

    let found = read(&app, &id, "latest.log?q=careful", &cookie).await;
    assert_eq!(texts(&found), ["[12:00:01] [main/WARN]: Careful"]);
    let found = read(&app, &id, "latest.log?q=Bro.e&regex=true", &cookie).await;
    assert_eq!(found["lines"][0]["number"], 3);

    let tail = read(&app, &id, "latest.log?limit=2", &cookie).await;
    assert_eq!(tail["lines"][0]["number"], 4);
    assert_eq!(tail["earlier"], 4);
    let before = read(&app, &id, "latest.log?limit=2&before=4", &cookie).await;
    assert_eq!(before["lines"][1]["number"], 3);

    let old = read(&app, &id, "2026-11-08-1.log.gz", &cookie).await;
    assert_eq!(texts(&old), ["[11:00:00] [main/INFO]: Yesterday"]);
}

#[tokio::test]
async fn a_bad_search_is_explained() {
    let app = TestApp::new().await;
    let cookie = app.setup_owner().await;
    let (id, _) = server_with_logs(&app, &cookie).await;
    for query in ["q=(&regex=true", "level=loud"] {
        let response = app
            .get(
                &format!("/api/v1/instances/{id}/logs/latest.log?{query}"),
                Some(&cookie),
            )
            .await;
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY, "{query}");
        assert_eq!(json_body(response).await["error"]["code"], "bad_log_search");
    }
}

#[tokio::test]
async fn crash_reports_come_with_a_summary() {
    let app = TestApp::new().await;
    let cookie = app.setup_owner().await;
    let (id, _) = server_with_logs(&app, &cookie).await;

    let report = read(&app, &id, "crash-1-server.txt", &cookie).await;
    assert_eq!(report["crash"]["description"], "Watching Server", "{report}");
    assert_eq!(report["crash"]["error"], "java.lang.Error: Watchdog");
    assert_eq!(report["crash"]["first_outside_code"], "com.example.mod");
    assert_eq!(texts(&report).len(), 6, "and the report itself");

    let jvm = read(&app, &id, "hs_err_pid42.log", &cookie).await;
    assert_eq!(jvm["crash"]["error"], "SIGSEGV (0xb)", "{jvm}");

    let plain = read(&app, &id, "latest.log", &cookie).await;
    assert_eq!(plain["crash"], Value::Null);
}

#[tokio::test]
async fn only_logs_can_be_reached_through_this() {
    let app = TestApp::new().await;
    let cookie = app.setup_owner().await;
    let (id, dir) = server_with_logs(&app, &cookie).await;
    std::os::unix::fs::symlink("/etc/passwd", dir.join("logs/linked.log")).unwrap();
    for name in [
        "server.properties",
        "notes.txt",
        "..%2Fserver.properties",
        "logs%2Flatest.log",
        "linked.log",
        "nothing.log",
    ] {
        let response = app
            .get(&format!("/api/v1/instances/{id}/logs/{name}"), Some(&cookie))
            .await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{name}");
        let download = app
            .get(
                &format!("/api/v1/instances/{id}/logs/{name}/download"),
                Some(&cookie),
            )
            .await;
        assert_eq!(download.status(), StatusCode::NOT_FOUND, "{name}");
    }
}

#[tokio::test]
async fn a_log_downloads_as_it_is() {
    let app = TestApp::new().await;
    let cookie = app.setup_owner().await;
    let (id, _) = server_with_logs(&app, &cookie).await;
    let response = app
        .get(
            &format!("/api/v1/instances/{id}/logs/latest.log/download"),
            Some(&cookie),
        )
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let disposition = response.headers()["content-disposition"].to_str().unwrap();
    assert!(disposition.starts_with("attachment"), "{disposition}");
    assert_eq!(text_body(response).await, LOG);
}

#[tokio::test]
async fn a_viewer_reads_logs_but_a_stranger_does_not() {
    let app = TestApp::new().await;
    let cookie = app.setup_owner().await;
    let (id, _) = server_with_logs(&app, &cookie).await;

    let viewer = app.create_user("viewer", GlobalRole::Member).await;
    app.state
        .auth
        .grants()
        .set(viewer, id.parse().unwrap(), Grant::Viewer)
        .await
        .unwrap();
    let viewer = app.login("viewer").await;
    assert_eq!(texts(&read(&app, &id, "latest.log", &viewer).await).len(), 5);

    app.create_user("stranger", GlobalRole::Member).await;
    let stranger = app.login("stranger").await;
    let denied = app
        .get(&format!("/api/v1/instances/{id}/logs"), Some(&stranger))
        .await;
    assert!(
        matches!(denied.status(), StatusCode::FORBIDDEN | StatusCode::NOT_FOUND),
        "{:?}",
        denied.status()
    );
    assert_eq!(
        app.get(&format!("/api/v1/instances/{id}/logs"), None)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
}
