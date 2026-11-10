//! Game rules through the API: a stopped server's world files (real ones from 26.3 and 1.20.1), a server
//! with no world yet, who may change them, and a running server's management protocol (a stand-in).

#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};

use axum::{http::StatusCode, response::Response};
use common::{TestApp, json_body};
use futures_util::{SinkExt, StreamExt};
use gsd_core::auth::{GlobalRole, Grant};
use gsd_minecraft::nbt::{self, Tag};
use serde_json::{Value, json};
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::Message;

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(format!(
        "{}/../gsd-minecraft/tests/fixtures/{name}",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap()
}

async fn server_dir(app: &TestApp, id: &str) -> PathBuf {
    let instance = app
        .state
        .instances
        .registry()
        .get(id.parse().unwrap())
        .await
        .unwrap();
    instance.layout.server_dir()
}

/// Puts a real world's file in place, as the server would have made it.
async fn world_file(app: &TestApp, id: &str, relative: &str, fixture_name: &str) -> PathBuf {
    let path = server_dir(app, id).await.join("world").join(relative);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, fixture(fixture_name)).unwrap();
    path
}

async fn gamerules(app: &TestApp, id: &str, cookie: &str) -> Value {
    json_body(
        app.get(&format!("/api/v1/instances/{id}/gamerules"), Some(cookie))
            .await,
    )
    .await
}

async fn change(app: &TestApp, id: &str, cookie: &str, changes: Value) -> Response {
    app.patch(
        &format!("/api/v1/instances/{id}/gamerules"),
        json!({ "changes": changes }),
        Some(cookie),
    )
    .await
}

async fn refused(app: &TestApp, id: &str, cookie: &str, changes: Value) -> (StatusCode, String) {
    let response = change(app, id, cookie, changes).await;
    let status = response.status();
    (
        status,
        json_body(response).await["error"]["code"]
            .as_str()
            .unwrap_or_default()
            .to_owned(),
    )
}

fn rule<'a>(view: &'a Value, name: &str) -> &'a Value {
    view["rules"]
        .as_array()
        .unwrap()
        .iter()
        .find(|rule| rule["name"] == name)
        .unwrap()
}

fn rules_in(path: &PathBuf, at: &[&str]) -> Vec<(String, Tag)> {
    nbt::read_file(&std::fs::read(path).unwrap())
        .unwrap()
        .1
        .compound_at(at)
        .unwrap()
        .0
        .clone()
}

#[tokio::test]
async fn a_stopped_26_3_world_is_read_and_changed_in_its_own_file() {
    let app = TestApp::new().await;
    let cookie = app.setup_owner().await;
    let id = app.installed_instance("Rules", &cookie).await;

    let none = gamerules(&app, &id, &cookie).await;
    assert_eq!(
        (none["values"].clone(), none["rules"].clone()),
        (Value::Null, json!([])),
        "no world yet"
    );
    assert_eq!(
        refused(&app, &id, &cookie, json!({ "keep_inventory": true })).await,
        (StatusCode::CONFLICT, "no_world".into())
    );

    let file = world_file(&app, &id, "data/minecraft/game_rules.dat", "game_rules_26.3.dat").await;
    let view = gamerules(&app, &id, &cookie).await;
    assert_eq!(
        (view["channel"].as_str(), view["values"].as_str()),
        (Some("files"), Some("files"))
    );
    assert_eq!(view["rules"].as_array().unwrap().len(), 58);
    let keep = rule(&view, "keep_inventory");
    assert_eq!(
        (
            keep["type"].as_str(),
            keep["value"].clone(),
            keep["default"].clone()
        ),
        (Some("bool"), json!(false), json!(false))
    );
    assert_eq!(keep["category"], "player");
    assert_eq!(view["rules"][0]["category"], "player", "grouped");

    let response = change(
        &app,
        &id,
        &cookie,
        json!({ "keep_inventory": true, "random_tick_speed": 10 }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let updated = json_body(response).await;
    assert_eq!(updated["via"], "files");
    assert_eq!(rule(&updated["gamerules"], "keep_inventory")["value"], true);
    let saved = rules_in(&file, &["data"]);
    assert!(saved.contains(&("minecraft:keep_inventory".into(), Tag::Byte(1))));
    assert!(saved.contains(&("minecraft:random_tick_speed".into(), Tag::Int(10))));
    assert_eq!(saved.len(), 58, "nothing else came or went");

    for (changes, code) in [
        (json!({ "no_such_rule": true }), "unknown_gamerule"),
        (json!({ "keep_inventory": 1 }), "wrong_gamerule_type"),
        (
            json!({ "random_tick_speed": 3_000_000_000_i64 }),
            "wrong_gamerule_type",
        ),
        (json!({}), "nothing_to_change"),
    ] {
        assert_eq!(
            refused(&app, &id, &cookie, changes.clone()).await,
            (StatusCode::UNPROCESSABLE_ENTITY, code.into()),
            "{changes}"
        );
    }
    assert!(
        rules_in(&file, &["data"]).contains(&("minecraft:random_tick_speed".into(), Tag::Int(10))),
        "refusals changed nothing"
    );
}

#[tokio::test]
async fn an_older_worlds_rules_are_changed_in_its_level_dat() {
    let app = TestApp::new().await;
    let cookie = app.setup_owner().await;
    let id = app.installed_instance("Old rules", &cookie).await;
    let file = world_file(&app, &id, "level.dat", "level_1.20.1.dat").await;

    let view = gamerules(&app, &id, &cookie).await;
    assert_eq!(view["rules"].as_array().unwrap().len(), 45);
    let rule_of = rule(&view, "doDaylightCycle");
    assert_eq!(
        rule_of["label"], "Day and night",
        "old names get the same description"
    );
    assert_eq!(rule(&view, "disableRaids")["default"], false);

    let response = change(
        &app,
        &id,
        &cookie,
        json!({ "keepInventory": true, "randomTickSpeed": 0 }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let saved = rules_in(&file, &["Data", "GameRules"]);
    assert!(saved.contains(&("keepInventory".into(), Tag::String("true".into()))));
    assert!(saved.contains(&("randomTickSpeed".into(), Tag::String("0".into()))));
    let rest = nbt::read_file(&std::fs::read(&file).unwrap()).unwrap().1;
    assert!(
        rest.compound_at(&["Data", "WorldGenSettings"]).is_some(),
        "the rest of level.dat is still there"
    );
}

#[tokio::test]
async fn viewers_see_the_rules_and_only_properties_write_changes_them() {
    let app = TestApp::new().await;
    let cookie = app.setup_owner().await;
    let id = app.installed_instance("Rights", &cookie).await;
    world_file(&app, &id, "data/minecraft/game_rules.dat", "game_rules_26.3.dat").await;

    let viewer = app.create_user("viewer", GlobalRole::Member).await;
    app.state
        .auth
        .grants()
        .set(viewer, id.parse().unwrap(), Grant::Operator)
        .await
        .unwrap();
    let viewer = app.login("viewer").await;
    assert_eq!(
        gamerules(&app, &id, &viewer).await["rules"]
            .as_array()
            .unwrap()
            .len(),
        58
    );
    assert_eq!(
        refused(&app, &id, &viewer, json!({ "keep_inventory": true })).await,
        (StatusCode::FORBIDDEN, "missing_permission".into()),
        "an operator grant has no properties.write"
    );
    assert_eq!(
        app.get(&format!("/api/v1/instances/{id}/gamerules"), None)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
}

/// A stand-in for the management protocol: two rules, kept in memory; refuses a number for a true/false.
async fn fake_management(port: u16, secret: String) -> Arc<Mutex<Vec<Value>>> {
    let rules = Arc::new(Mutex::new(vec![
        json!({ "key": "minecraft:keep_inventory", "type": "boolean", "value": false }),
        json!({ "key": "minecraft:random_tick_speed", "type": "integer", "value": 3 }),
    ]));
    let listener = TcpListener::bind(("127.0.0.1", port)).await.unwrap();
    let shared = rules.clone();
    tokio::spawn(async move {
        loop {
            let (stream, _) = listener.accept().await.unwrap();
            let (rules, secret) = (shared.clone(), secret.clone());
            tokio::spawn(async move {
                #[allow(clippy::result_large_err)]
                let check = |request: &tokio_tungstenite::tungstenite::handshake::server::Request,
                             response: tokio_tungstenite::tungstenite::handshake::server::Response| {
                    if request.headers().get("Authorization").is_some_and(|v| v == &format!("Bearer {secret}")) {
                        Ok(response)
                    } else {
                        let mut refused = tokio_tungstenite::tungstenite::handshake::server::ErrorResponse::new(None);
                        *refused.status_mut() = StatusCode::UNAUTHORIZED;
                        Err(refused)
                    }
                };
                let Ok(mut socket) = tokio_tungstenite::accept_hdr_async(stream, check).await else {
                    return;
                };
                while let Some(Ok(Message::Text(text))) = socket.next().await {
                    let request: Value = serde_json::from_str(text.as_str()).unwrap();
                    let reply = match request["method"].as_str().unwrap() {
                        "minecraft:gamerules" => json!({ "result": *rules.lock().unwrap() }),
                        "minecraft:gamerules/update" => {
                            let wanted = &request["params"]["gamerule"];
                            let key = format!("minecraft:{}", wanted["key"].as_str().unwrap());
                            let mut rules = rules.lock().unwrap();
                            let rule = rules.iter_mut().find(|rule| rule["key"] == key).unwrap();
                            if rule["type"] == "boolean" && !wanted["value"].is_boolean() {
                                json!({ "error": { "code": -32602, "message": "Invalid params" } })
                            } else {
                                rule["value"] = wanted["value"].clone();
                                json!({ "result": rule.clone() })
                            }
                        }
                        _ => json!({ "error": { "code": -32601, "message": "Method not found" } }),
                    };
                    let mut reply = reply;
                    reply["jsonrpc"] = "2.0".into();
                    reply["id"] = request["id"].clone();
                    socket.send(Message::text(reply.to_string())).await.unwrap();
                }
            });
        }
    });
    rules
}

#[tokio::test]
async fn a_running_server_is_changed_through_its_management_protocol() {
    let app = TestApp::new().await;
    let cookie = app.setup_owner().await;
    let id = app.installed_instance("Live rules", &cookie).await;
    let started = app
        .post(&format!("/api/v1/instances/{id}/start"), json!({}), Some(&cookie))
        .await;
    assert!(started.status().is_success());
    app.wait_for_state(&id, &cookie, "running").await;
    let instance = app
        .state
        .instances
        .registry()
        .get(id.parse().unwrap())
        .await
        .unwrap();
    let properties = std::fs::read_to_string(instance.layout.server_dir().join("server.properties")).unwrap();
    let secret = properties
        .lines()
        .find_map(|line| line.strip_prefix("management-server-secret="))
        .unwrap()
        .to_owned();
    let live = fake_management(instance.definition.ports.control.unwrap(), secret).await;

    let view = gamerules(&app, &id, &cookie).await;
    assert_eq!(
        (view["channel"].as_str(), view["values"].as_str()),
        (Some("management"), Some("server"))
    );
    assert_eq!(rule(&view, "random_tick_speed")["value"], 3);

    let response = change(&app, &id, &cookie, json!({ "random_tick_speed": 7 })).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(json_body(response).await["via"], "management");
    assert_eq!(live.lock().unwrap()[1]["value"], 7);

    let stop = app
        .post(&format!("/api/v1/instances/{id}/stop"), json!({}), Some(&cookie))
        .await;
    assert!(stop.status().is_success());
    app.wait_for_state(&id, &cookie, "stopped").await;
}
