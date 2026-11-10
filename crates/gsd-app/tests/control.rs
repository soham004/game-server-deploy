//! The control channel's settings on a real start of the fake server: a port from the internal range, the
//! sandbox's allowance for it, the settings in `server.properties`, and no secret leaving the app.

#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use axum::http::StatusCode;
use common::{TestApp, json_body, text_body};
use gsd_core::instances::launch::build_process;
use gsd_core::java::JavaRuntime;
use serde_json::json;

async fn started_and_stopped(app: &TestApp, id: &str, cookie: &str) {
    let start = app
        .post(&format!("/api/v1/instances/{id}/start"), json!({}), Some(cookie))
        .await;
    assert!(start.status().is_success(), "{:?}", start.status());
    app.wait_for_state(id, cookie, "running").await;
    let stop = app
        .post(&format!("/api/v1/instances/{id}/stop"), json!({}), Some(cookie))
        .await;
    assert!(stop.status().is_success());
    app.wait_for_state(id, cookie, "stopped").await;
}

fn properties(app: &TestApp, slug: &str) -> String {
    std::fs::read_to_string(
        app.state
            .config
            .instances_dir()
            .join(slug)
            .join("server/server.properties"),
    )
    .unwrap()
}

fn value<'a>(text: &'a str, key: &str) -> Option<&'a str> {
    text.lines()
        .find_map(|line| line.strip_prefix(&format!("{key}=")))
}

#[tokio::test]
async fn a_start_gives_a_server_its_port_and_settings_and_keeps_them() {
    let app = TestApp::new().await;
    let cookie = app.setup_owner().await;
    let id = app.installed_instance("Controlled", &cookie).await;
    let parsed = id.parse().unwrap();
    let before = app.state.instances.registry().get(parsed).await.unwrap();
    assert_eq!(before.definition.ports.control, None, "not until it starts");

    started_and_stopped(&app, &id, &cookie).await;

    let instance = app.state.instances.registry().get(parsed).await.unwrap();
    let port = instance.definition.ports.control.expect("a control port");
    assert!(app.state.config.internal_ports.contains(port), "{port}");
    let saved = gsd_core::instances::InstanceDefinition::load(&instance.layout.definition_path()).unwrap();
    assert_eq!(saved.ports.control, Some(port), "it is in instance.toml");

    let java = JavaRuntime {
        id: "test".into(),
        major: 25,
        version: "25".into(),
        vendor: None,
        home: "/java".into(),
        executable: "/java/bin/java".into(),
        managed: true,
    };
    let process = build_process(&instance.definition, &instance.layout, &java);
    let sandbox = process.sandbox.unwrap();
    assert_eq!(
        sandbox.bind_tcp,
        [instance.definition.ports.game, port],
        "the sandbox lets it listen there"
    );

    let text = properties(&app, &instance.layout.slug());
    assert_eq!(value(&text, "management-server-enabled"), Some("true"), "{text}");
    assert_eq!(value(&text, "management-server-host"), Some("127.0.0.1"));
    assert_eq!(value(&text, "management-server-tls-enabled"), Some("false"));
    let port_text = port.to_string();
    assert_eq!(value(&text, "management-server-port"), Some(port_text.as_str()));
    let secret = value(&text, "management-server-secret").unwrap().to_owned();
    assert_eq!(secret.len(), 40);

    started_and_stopped(&app, &id, &cookie).await;
    let again = properties(&app, &instance.layout.slug());
    assert_eq!(
        value(&again, "management-server-secret"),
        Some(secret.as_str()),
        "the same one"
    );
}

#[tokio::test]
async fn servers_get_ports_and_secrets_of_their_own() {
    let app = TestApp::new().await;
    let cookie = app.setup_owner().await;
    let one = app.installed_instance("One", &cookie).await;
    let two = app.installed_instance("Two", &cookie).await;
    started_and_stopped(&app, &one, &cookie).await;
    started_and_stopped(&app, &two, &cookie).await;
    let registry = app.state.instances.registry();
    let first = registry.get(one.parse().unwrap()).await.unwrap();
    let second = registry.get(two.parse().unwrap()).await.unwrap();
    assert_ne!(first.definition.ports.control, second.definition.ports.control);
    let (a, b) = (
        properties(&app, &first.layout.slug()),
        properties(&app, &second.layout.slug()),
    );
    assert_ne!(
        value(&a, "management-server-secret"),
        value(&b, "management-server-secret")
    );
}

/// The file is the server's, so it can make it a link; the app, possibly root, mustn't write through that.
#[tokio::test]
async fn a_link_in_place_of_the_settings_is_not_followed() {
    let app = TestApp::new().await;
    let cookie = app.setup_owner().await;
    let id = app.installed_instance("Linked", &cookie).await;
    let instance = app
        .state
        .instances
        .registry()
        .get(id.parse().unwrap())
        .await
        .unwrap();
    let outside = app.dir.path().join("outside.properties");
    std::fs::write(&outside, "untouched=true\n").unwrap();
    let file = instance.layout.server_dir().join("server.properties");
    std::fs::remove_file(&file).unwrap();
    std::os::unix::fs::symlink(&outside, &file).unwrap();

    let start = app
        .post(&format!("/api/v1/instances/{id}/start"), json!({}), Some(&cookie))
        .await;
    assert!(start.status().is_success(), "the server still starts");
    assert_eq!(std::fs::read_to_string(&outside).unwrap(), "untouched=true\n");
    let _ = app
        .post(&format!("/api/v1/instances/{id}/kill"), json!({}), Some(&cookie))
        .await;
}

#[tokio::test]
async fn the_status_says_the_channel_and_never_the_secret() {
    let app = TestApp::new().await;
    let cookie = app.setup_owner().await;
    let id = app.installed_instance("Status", &cookie).await;
    let route = format!("/api/v1/instances/{id}/control");

    let stopped = json_body(app.get(&route, Some(&cookie)).await).await;
    assert_eq!(stopped["channel"], "management", "{stopped}");
    assert_eq!(stopped["connected"], false);
    assert_eq!(
        stopped["problem"],
        serde_json::Value::Null,
        "stopped isn't a problem"
    );

    let start = app
        .post(&format!("/api/v1/instances/{id}/start"), json!({}), Some(&cookie))
        .await;
    assert!(start.status().is_success());
    app.wait_for_state(&id, &cookie, "running").await;
    let response = app.get(&route, Some(&cookie)).await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = text_body(response).await;

    let instance = app
        .state
        .instances
        .registry()
        .get(id.parse().unwrap())
        .await
        .unwrap();
    let text = properties(&app, &instance.layout.slug());
    let secret = value(&text, "management-server-secret").unwrap();
    assert!(!body.contains(secret), "{body}");
    // The fake server has no management protocol, so this reports why it can't connect.
    let status: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(status["connected"], false);
    assert!(status["problem"].is_string(), "{status}");

    let stop = app
        .post(&format!("/api/v1/instances/{id}/stop"), json!({}), Some(&cookie))
        .await;
    assert!(stop.status().is_success());
    app.wait_for_state(&id, &cookie, "stopped").await;

    assert_eq!(app.get(&route, None).await.status(), StatusCode::UNAUTHORIZED);
}
