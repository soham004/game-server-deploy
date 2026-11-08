//! The live event stream over a real TCP socket.

#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::time::Duration;

use axum::http::StatusCode;
use common::{TestApp, json_body};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::{Message, client::IntoClientRequest};

type Socket = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn serve(app: &TestApp) -> std::net::SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let router = gsd_app::http::router(app.state.clone());
    tokio::spawn(async move {
        axum::serve(
            listener,
            router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await
        .unwrap();
    });
    addr
}

async fn connect(
    addr: std::net::SocketAddr,
    cookie: Option<&str>,
) -> Result<Socket, tokio_tungstenite::tungstenite::Error> {
    let mut request = format!("ws://{addr}/api/v1/ws").into_client_request().unwrap();
    if let Some(cookie) = cookie {
        request.headers_mut().insert("cookie", cookie.parse().unwrap());
    }
    tokio_tungstenite::connect_async(request)
        .await
        .map(|(socket, _)| socket)
}

/// Next JSON frame matching `predicate`, skipping others.
async fn next_matching(socket: &mut Socket, predicate: impl Fn(&Value) -> bool) -> Value {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let message = tokio::time::timeout_at(deadline, socket.next())
            .await
            .expect("timed out")
            .unwrap()
            .unwrap();
        if let Message::Text(text) = message {
            let value: Value = serde_json::from_str(&text).unwrap();
            if predicate(&value) {
                return value;
            }
        }
    }
}

#[tokio::test]
async fn requires_a_session() {
    let app = TestApp::new().await;
    let addr = serve(&app).await;
    assert!(connect(addr, None).await.is_err());
}

#[tokio::test]
async fn only_pages_of_this_site_may_open_a_stream() {
    let app = TestApp::new().await;
    let cookie = app.setup_owner().await;
    let addr = serve(&app).await;
    let with_origin = |origin: String| {
        let mut request = format!("ws://{addr}/api/v1/ws").into_client_request().unwrap();
        let headers = request.headers_mut();
        headers.insert("cookie", cookie.parse().unwrap());
        headers.insert("origin", origin.parse().unwrap());
        request
    };

    let foreign = tokio_tungstenite::connect_async(with_origin("http://evil.test".into())).await;
    match foreign {
        Err(tokio_tungstenite::tungstenite::Error::Http(response)) => {
            assert_eq!(response.status(), StatusCode::FORBIDDEN);
        }
        other => panic!("a stream opened from another site: {:?}", other.map(|_| ())),
    }
    assert!(
        tokio_tungstenite::connect_async(with_origin(format!("http://{addr}")))
            .await
            .is_ok(),
        "the app's own pages"
    );
}

#[tokio::test]
async fn streams_instance_state_jobs_and_console() {
    let app = TestApp::new().await;
    let cookie = app.setup_owner().await;
    let addr = serve(&app).await;
    let mut socket = connect(addr, Some(&cookie)).await.unwrap();
    socket
        .send(Message::text(
            json!({ "op": "subscribe", "topics": ["instances", "jobs"] }).to_string(),
        ))
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await; // let the subscription land

    let created = json_body(
        app.post(
            "/api/v1/instances",
            json!({ "name": "Live", "mc_version": "26.3", "loader": "vanilla", "max_memory_mb": 1024, "eula_accepted": true }),
            Some(&cookie),
        )
        .await,
    )
    .await;
    let id = created["instance"]["id"].as_str().unwrap().to_owned();

    let job = next_matching(&mut socket, |v| {
        v["topic"] == "jobs" && v["data"]["status"] == "succeeded"
    })
    .await;
    assert_eq!(job["data"]["kind"], "instance.install");

    socket
        .send(Message::text(
            json!({ "op": "subscribe", "topics": [format!("console:{id}")] }).to_string(),
        ))
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    let start = app
        .post(&format!("/api/v1/instances/{id}/start"), json!({}), Some(&cookie))
        .await;
    assert_eq!(start.status(), StatusCode::ACCEPTED);

    let running = next_matching(&mut socket, |v| {
        v["type"] == "state" && v["data"]["status"] == "running"
    })
    .await;
    assert_eq!(running["instance_id"], id.as_str());
    assert_eq!(running["topic"], "instances");

    app.post(
        &format!("/api/v1/instances/{id}/command"),
        json!({ "command": "say over the wire" }),
        Some(&cookie),
    )
    .await;
    let line = next_matching(&mut socket, |v| {
        v["type"] == "console" && v["data"]["text"] == "[Server] over the wire"
    })
    .await;
    assert_eq!(line["topic"], format!("console:{id}"));

    app.post(&format!("/api/v1/instances/{id}/kill"), json!({}), Some(&cookie))
        .await;
}

#[tokio::test]
async fn members_do_not_receive_other_instances_events() {
    let app = TestApp::new().await;
    let owner = app.setup_owner().await;
    app.create_user("friend", gsd_core::auth::GlobalRole::Member)
        .await;
    let friend = app.login("friend").await;
    let addr = serve(&app).await;

    let mut socket = connect(addr, Some(&friend)).await.unwrap();
    socket
        .send(Message::text(
            json!({ "op": "subscribe", "topics": ["instances", "jobs"] }).to_string(),
        ))
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;

    let created = json_body(
        app.post(
            "/api/v1/instances",
            json!({ "name": "Private", "mc_version": "26.3", "loader": "vanilla", "max_memory_mb": 1024, "eula_accepted": true }),
            Some(&owner),
        )
        .await,
    )
    .await;
    app.wait_for_job(created["job_id"].as_str().unwrap(), &owner)
        .await;

    // Nothing about the private instance may arrive within a short window.
    // (Pings and timeouts are fine; any text frame is a leak.)
    let received = tokio::time::timeout(Duration::from_millis(500), socket.next()).await;
    if let Ok(Some(Ok(Message::Text(text)))) = received {
        panic!("member received {text}");
    }
}
