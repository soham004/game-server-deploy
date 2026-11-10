//! Players through the API: a stopped server's files, a running server's console (the fake server), the
//! management protocol (a stand-in on the server's control port), the visit history, who may do what, and
//! heads from Mojang (a mock).

#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::path::PathBuf;

use axum::{http::StatusCode, response::Response};
use base64::Engine;
use common::{TestApp, json_body};
use futures_util::{SinkExt, StreamExt};
use gsd_core::auth::{GlobalRole, Grant};
use gsd_minecraft::{players::names::offline_uuid, test_support::test_skin};
use serde_json::{Value, json};
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::Message;
use wiremock::{
    Mock, ResponseTemplate,
    matchers::{method, path},
};

const NOTCH: &str = "069a79f4-44e9-4726-a5be-fca90e38aaf5";

async fn act(app: &TestApp, id: &str, cookie: &str, body: Value) -> Response {
    app.post(
        &format!("/api/v1/instances/{id}/players/actions"),
        body,
        Some(cookie),
    )
    .await
}

/// Does it, and expects it to work; returns the answer.
async fn done(app: &TestApp, id: &str, cookie: &str, body: Value) -> Value {
    let response = act(app, id, cookie, body.clone()).await;
    let status = response.status();
    let answer = json_body(response).await;
    assert_eq!(status, StatusCode::OK, "{body}: {answer}");
    answer
}

async fn refused(app: &TestApp, id: &str, cookie: &str, body: Value) -> (StatusCode, String) {
    let response = act(app, id, cookie, body).await;
    let status = response.status();
    (
        status,
        json_body(response).await["error"]["code"]
            .as_str()
            .unwrap_or_default()
            .to_owned(),
    )
}

async fn players(app: &TestApp, id: &str, cookie: &str) -> Value {
    json_body(
        app.get(&format!("/api/v1/instances/{id}/players"), Some(cookie))
            .await,
    )
    .await
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

async fn file(app: &TestApp, id: &str, name: &str) -> String {
    std::fs::read_to_string(server_dir(app, id).await.join(name)).unwrap_or_default()
}

async fn command(app: &TestApp, id: &str, cookie: &str, line: &str) {
    let sent = app
        .post(
            &format!("/api/v1/instances/{id}/command"),
            json!({ "command": line }),
            Some(cookie),
        )
        .await;
    assert_eq!(sent.status(), StatusCode::NO_CONTENT);
}

async fn start(app: &TestApp, id: &str, cookie: &str) {
    let started = app
        .post(&format!("/api/v1/instances/{id}/start"), json!({}), Some(cookie))
        .await;
    assert!(started.status().is_success(), "{:?}", started.status());
    app.wait_for_state(id, cookie, "running").await;
}

async fn stop(app: &TestApp, id: &str, cookie: &str) {
    let stopped = app
        .post(&format!("/api/v1/instances/{id}/stop"), json!({}), Some(cookie))
        .await;
    assert!(stopped.status().is_success());
    app.wait_for_state(id, cookie, "stopped").await;
}

/// Polls the view until `check` holds.
async fn players_until(app: &TestApp, id: &str, cookie: &str, check: impl Fn(&Value) -> bool) -> Value {
    for _ in 0..100 {
        let view = players(app, id, cookie).await;
        if check(&view) {
            return view;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    panic!(
        "the players view never got there: {}",
        players(app, id, cookie).await
    );
}

/// Mojang has an account called `name`, which it finds whatever the case it is asked in.
async fn mock_account(app: &TestApp, name: &str, uuid: &str) {
    Mock::given(method("GET"))
        .and(wiremock::matchers::path_regex(format!(
            "(?i)^/users/profiles/minecraft/{name}$"
        )))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({ "id": uuid.replace('-', ""), "name": name })),
        )
        .mount(&app.upstream)
        .await;
}

#[tokio::test]
async fn a_stopped_servers_lists_are_changed_in_its_files() {
    let app = TestApp::new().await;
    let cookie = app.setup_owner().await;
    let id = app.installed_instance("Stopped", &cookie).await;
    mock_account(&app, "Notch", NOTCH).await;

    let empty = players(&app, &id, &cookie).await;
    assert_eq!(empty["channel"], "files");
    assert_eq!(empty["online_mode"], true);
    assert_eq!(empty["operators"], json!([]));

    // A name becomes its account: Mojang's spelling and UUID.
    let allowed = done(&app, &id, &cookie, json!({ "action": "allow", "name": "notch" })).await;
    assert_eq!(allowed["via"], "files");
    assert_eq!(
        allowed["players"]["allowlist"]["players"],
        json!([{ "uuid": NOTCH, "name": "Notch" }])
    );
    assert!(file(&app, &id, "whitelist.json").await.contains(NOTCH));

    done(
        &app,
        &id,
        &cookie,
        json!({ "action": "op", "name": "Notch", "level": 2 }),
    )
    .await;
    let ops: Value = serde_json::from_str(&file(&app, &id, "ops.json").await).unwrap();
    assert_eq!(
        ops,
        json!([{ "uuid": NOTCH, "name": "Notch", "level": 2, "bypassesPlayerLimit": false }])
    );

    let until = "2030-01-02T03:04:05Z";
    let banned = done(
        &app,
        &id,
        &cookie,
        json!({ "action": "ban", "name": "Notch", "reason": "Testing", "expires": until }),
    )
    .await;
    assert_eq!(banned["players"]["bans"][0]["expires"], "2030-01-02T03:04:05Z");
    assert_eq!(banned["players"]["bans"][0]["source"], "owner");
    let bans = file(&app, &id, "banned-players.json").await;
    assert!(
        bans.contains("\"expires\": \"2030-01-02 03:04:05 +0000\""),
        "the game's own format: {bans}"
    );

    done(
        &app,
        &id,
        &cookie,
        json!({ "action": "ban_ip", "target": "10.0.0.1", "reason": "Spam" }),
    )
    .await;
    assert!(
        file(&app, &id, "banned-ips.json")
            .await
            .contains("\"expires\": \"forever\"")
    );

    let on = done(&app, &id, &cookie, json!({ "action": "allowlist", "on": true })).await;
    assert_eq!(on["players"]["allowlist"]["on"], true);
    assert!(
        file(&app, &id, "server.properties")
            .await
            .contains("white-list=true")
    );

    for undo in [
        json!({ "action": "pardon", "name": "NOTCH" }),
        json!({ "action": "pardon_ip", "ip": "10.0.0.1" }),
        json!({ "action": "deop", "name": "Notch" }),
        json!({ "action": "disallow", "name": "Notch" }),
    ] {
        done(&app, &id, &cookie, undo).await;
    }
    let after = players(&app, &id, &cookie).await;
    for list in [
        &after["operators"],
        &after["bans"],
        &after["ip_bans"],
        &after["allowlist"]["players"],
    ] {
        assert_eq!(list, &json!([]), "{after}");
    }
}

#[tokio::test]
async fn what_a_stopped_server_cant_do_or_doesnt_know_is_refused() {
    let app = TestApp::new().await;
    let cookie = app.setup_owner().await;
    let id = app.installed_instance("Refusals", &cookie).await;
    assert_eq!(
        refused(&app, &id, &cookie, json!({ "action": "kick", "name": "Notch" })).await,
        (StatusCode::CONFLICT, "not_running".into())
    );
    assert_eq!(
        refused(&app, &id, &cookie, json!({ "action": "op", "name": "@a" })).await,
        (StatusCode::UNPROCESSABLE_ENTITY, "invalid_player_action".into())
    );
    Mock::given(method("GET"))
        .and(path("/users/profiles/minecraft/Nobody_At_All"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&app.upstream)
        .await;
    assert_eq!(
        refused(
            &app,
            &id,
            &cookie,
            json!({ "action": "allow", "name": "Nobody_At_All" })
        )
        .await,
        (StatusCode::NOT_FOUND, "no_such_account".into())
    );
}

#[tokio::test]
async fn offline_mode_servers_use_the_offline_uuid_without_asking_mojang() {
    let app = TestApp::new().await;
    let cookie = app.setup_owner().await;
    let id = app.installed_instance("Offline", &cookie).await;
    std::fs::write(
        server_dir(&app, &id).await.join("server.properties"),
        "online-mode=false\n",
    )
    .unwrap();

    let allowed = done(&app, &id, &cookie, json!({ "action": "allow", "name": "Steve" })).await;
    assert_eq!(allowed["players"]["online_mode"], false);
    assert_eq!(
        allowed["players"]["allowlist"]["players"][0]["uuid"],
        offline_uuid("Steve")
    );
    // In the server's name cache too, so the running server finds this entry again by name (it
    // lowercases names it hasn't cached, which is another UUID in offline mode).
    let cache: Value = serde_json::from_str(&file(&app, &id, "usercache.json").await).unwrap();
    assert_eq!(cache[0]["name"], "Steve");
    assert_eq!(cache[0]["uuid"], offline_uuid("Steve"));
    assert!(cache[0]["expiresOn"].as_str().unwrap().ends_with("+0000"));
}

#[tokio::test]
async fn a_running_server_is_changed_through_its_console_when_the_management_protocol_is_unreachable() {
    let app = TestApp::new().await;
    let cookie = app.setup_owner().await;
    let id = app.installed_instance("Console", &cookie).await;
    start(&app, &id, &cookie).await;

    command(&app, &id, &cookie, "join Steve").await;
    let on = players_until(&app, &id, &cookie, |view| {
        view["online"].as_array().is_some_and(|online| !online.is_empty())
    })
    .await;
    assert_eq!(on["channel"], "management", "a 26.3 server has the protocol");
    assert_eq!(on["online"][0]["name"], "Steve");
    assert!(on["online"][0]["since"].is_string(), "{on}");
    assert_eq!(on["known"][0]["online"], true);

    // Nothing listens on its control port, so the console does it.
    let opped = done(&app, &id, &cookie, json!({ "action": "op", "name": "Steve" })).await;
    assert_eq!(opped["via"], "console");
    assert_eq!(opped["players"]["operators"][0]["name"], "Steve");
    let console: Vec<String> = json_body(
        app.get(&format!("/api/v1/instances/{id}/console"), Some(&cookie))
            .await,
    )
    .await["console"]
        .as_array()
        .unwrap()
        .iter()
        .map(|line| line["text"].as_str().unwrap_or_default().to_owned())
        .collect();
    assert!(
        console.iter().any(|line| line == "[owner] > op Steve"),
        "typed as its user: {console:?}"
    );

    // What only the protocol can say isn't guessed at.
    let (status, code) = refused(
        &app,
        &id,
        &cookie,
        json!({ "action": "op", "name": "Steve", "level": 2 }),
    )
    .await;
    assert_eq!(
        (status, code.as_str()),
        (StatusCode::BAD_GATEWAY, "control_unavailable")
    );

    let kicked = done(
        &app,
        &id,
        &cookie,
        json!({ "action": "kick", "name": "Steve", "reason": "Later" }),
    )
    .await;
    assert_eq!(
        kicked["players"]["online"],
        json!([]),
        "confirmed by the leave line"
    );
    assert_eq!(
        refused(&app, &id, &cookie, json!({ "action": "kick", "name": "Steve" })).await,
        (StatusCode::CONFLICT, "player_not_online".into())
    );

    let visits = json_body(
        app.get(
            &format!("/api/v1/instances/{id}/players/Steve/sessions"),
            Some(&cookie),
        )
        .await,
    )
    .await;
    assert_eq!(visits.as_array().unwrap().len(), 1, "{visits}");
    assert!(visits[0]["left_at"].is_string());

    // A visit that was going on when the server stopped ends with it.
    command(&app, &id, &cookie, "join Alex").await;
    players_until(&app, &id, &cookie, |view| {
        view["online"].as_array().is_some_and(|online| online.len() == 1)
    })
    .await;
    stop(&app, &id, &cookie).await;
    let stopped = players_until(&app, &id, &cookie, |view| {
        view["known"]
            .as_array()
            .is_some_and(|known| known.iter().all(|player| player["online"] == false))
    })
    .await;
    assert_eq!(stopped["channel"], "files");
    let alex = json_body(
        app.get(
            &format!("/api/v1/instances/{id}/players/Alex/sessions"),
            Some(&cookie),
        )
        .await,
    )
    .await;
    assert!(alex[0]["left_at"].is_string(), "{alex}");
    assert_eq!(alex[0]["online"], false);
}

/// A stand-in for the management protocol on `port`: it wants `secret`, does `operators/add` by writing the
/// server's `ops.json` as the game would, and kicks nobody.
async fn fake_management(port: u16, secret: String, server_dir: PathBuf) {
    let listener = TcpListener::bind(("127.0.0.1", port)).await.unwrap();
    tokio::spawn(async move {
        loop {
            let (stream, _) = listener.accept().await.unwrap();
            let secret = secret.clone();
            let server_dir = server_dir.clone();
            tokio::spawn(async move {
                #[allow(clippy::result_large_err)]
                let check = |request: &tokio_tungstenite::tungstenite::handshake::server::Request,
                             response: tokio_tungstenite::tungstenite::handshake::server::Response| {
                    let good = request.headers().get("Authorization").is_some_and(|value| value == &format!("Bearer {secret}"));
                    if good {
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
                    let result = match request["method"].as_str().unwrap() {
                        "minecraft:operators/add" => {
                            let operator = &request["params"]["add"][0];
                            let entry = json!([{ "uuid": offline_uuid(operator["player"]["name"].as_str().unwrap()), "name": operator["player"]["name"], "level": operator["permissionLevel"], "bypassesPlayerLimit": false }]);
                            std::fs::write(server_dir.join("ops.json"), entry.to_string()).unwrap();
                            entry
                        }
                        _ => json!([]),
                    };
                    let reply = json!({ "jsonrpc": "2.0", "id": request["id"], "result": result });
                    socket.send(Message::text(reply.to_string())).await.unwrap();
                }
            });
        }
    });
}

#[tokio::test]
async fn the_management_protocol_is_used_when_it_answers() {
    let app = TestApp::new().await;
    let cookie = app.setup_owner().await;
    let id = app.installed_instance("Managed", &cookie).await;
    start(&app, &id, &cookie).await;
    let instance = app
        .state
        .instances
        .registry()
        .get(id.parse().unwrap())
        .await
        .unwrap();
    let properties = file(&app, &id, "server.properties").await;
    let secret = properties
        .lines()
        .find_map(|line| line.strip_prefix("management-server-secret="))
        .unwrap()
        .to_owned();
    fake_management(
        instance.definition.ports.control.unwrap(),
        secret,
        instance.layout.server_dir(),
    )
    .await;

    let opped = done(
        &app,
        &id,
        &cookie,
        json!({ "action": "op", "name": "Alex", "level": 3 }),
    )
    .await;
    assert_eq!(opped["via"], "management");
    assert_eq!(
        opped["players"]["operators"][0]["level"], 3,
        "a level the console can't give"
    );
    assert_eq!(
        refused(&app, &id, &cookie, json!({ "action": "kick", "name": "Nobody" })).await,
        (StatusCode::CONFLICT, "player_not_online".into()),
        "the protocol kicked nobody"
    );
    stop(&app, &id, &cookie).await;
}

#[tokio::test]
async fn levels_above_two_need_console_write_and_banned_addresses_need_players_manage() {
    let app = TestApp::new().await;
    let cookie = app.setup_owner().await;
    let id = app.installed_instance("Rights", &cookie).await;
    std::fs::write(
        server_dir(&app, &id).await.join("server.properties"),
        "online-mode=false\n",
    )
    .unwrap();
    done(
        &app,
        &id,
        &cookie,
        json!({ "action": "ban_ip", "target": "10.0.0.9" }),
    )
    .await;

    let moderator = app.create_user("moderator", GlobalRole::Member).await;
    app.state
        .auth
        .grants()
        .set(moderator, id.parse().unwrap(), Grant::Moderator)
        .await
        .unwrap();
    let moderator = app.login("moderator").await;
    assert_eq!(
        refused(&app, &id, &moderator, json!({ "action": "op", "name": "Steve" })).await,
        (StatusCode::FORBIDDEN, "missing_permission".into()),
        "the default level is 4"
    );
    done(
        &app,
        &id,
        &moderator,
        json!({ "action": "op", "name": "Steve", "level": 2 }),
    )
    .await;
    assert_eq!(
        players(&app, &id, &moderator).await["ip_bans"]
            .as_array()
            .unwrap()
            .len(),
        1
    );

    let viewer = app.create_user("viewer", GlobalRole::Member).await;
    app.state
        .auth
        .grants()
        .set(viewer, id.parse().unwrap(), Grant::Viewer)
        .await
        .unwrap();
    let viewer = app.login("viewer").await;
    let seen = players(&app, &id, &viewer).await;
    assert_eq!(seen["operators"][0]["name"], "Steve");
    assert_eq!(
        seen["ip_bans"],
        json!([]),
        "addresses are for those who manage players"
    );
    assert_eq!(
        refused(&app, &id, &viewer, json!({ "action": "deop", "name": "Steve" }))
            .await
            .0,
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn heads_come_from_skins_and_need_a_session() {
    let app = TestApp::new().await;
    let cookie = app.setup_owner().await;
    let skin_url = format!("{}/texture/0a1b2c", app.upstream.uri());
    let textures = base64::engine::general_purpose::STANDARD
        .encode(json!({ "textures": { "SKIN": { "url": skin_url } } }).to_string());
    Mock::given(method("GET"))
        .and(path(format!(
            "/session/minecraft/profile/{}",
            NOTCH.replace('-', "")
        )))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({ "properties": [{ "name": "textures", "value": textures }] })),
        )
        .mount(&app.upstream)
        .await;
    Mock::given(method("GET"))
        .and(path("/texture/0a1b2c"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(test_skin(64)))
        .mount(&app.upstream)
        .await;

    let head = app
        .get(&format!("/api/v1/players/{NOTCH}/head.png"), Some(&cookie))
        .await;
    assert_eq!(head.status(), StatusCode::OK);
    assert_eq!(head.headers()["content-type"], "image/png");
    let bytes = axum::body::to_bytes(head.into_body(), usize::MAX).await.unwrap();
    assert!(bytes.starts_with(b"\x89PNG"));

    let offline = app
        .get(
            &format!("/api/v1/players/{}/head.png", offline_uuid("Steve")),
            Some(&cookie),
        )
        .await;
    assert_eq!(offline.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        app.get(&format!("/api/v1/players/{NOTCH}/head.png"), None)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
}
