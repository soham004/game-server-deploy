//! Guessing passwords at the sign-in endpoint is slowed down: after a few wrong ones for a
//! username from an address, the next attempt is refused with `429` and told how long to wait,
//! without the password being looked at.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::net::SocketAddr;

use axum::{
    body::Body,
    extract::ConnectInfo,
    http::{Request, StatusCode, header},
    response::Response,
};
use common::{PASSWORD, TestApp, json_body};
use gsd_app::http::throttle::FREE_ATTEMPTS;
use gsd_core::auth::GlobalRole;
use serde_json::json;

/// A sign-in attempt as if from `address`.
async fn sign_in(app: &TestApp, address: &str, username: &str, password: &str) -> Response {
    let peer: SocketAddr = format!("{address}:50000").parse().unwrap();
    let mut request = Request::post("/api/v1/auth/login")
        .header(header::CONTENT_TYPE, "application/json")
        .header("x-csrf", "1")
        .body(Body::from(
            json!({ "username": username, "password": password }).to_string(),
        ))
        .unwrap();
    request.extensions_mut().insert(ConnectInfo(peer));
    app.send(request).await
}

async fn guess_wrong(app: &TestApp, address: &str, username: &str, times: u32) {
    for attempt in 0..times {
        let response = sign_in(app, address, username, "not the password").await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "attempt {attempt}");
    }
}

#[tokio::test]
async fn wrong_passwords_soon_have_to_wait_and_the_right_one_too() {
    let app = TestApp::new().await;
    app.setup_owner().await;
    guess_wrong(&app, "203.0.113.9", "owner", FREE_ATTEMPTS + 1).await;

    let refused = sign_in(&app, "203.0.113.9", "owner", "not the password").await;
    assert_eq!(refused.status(), StatusCode::TOO_MANY_REQUESTS);
    let wait: u64 = refused.headers()[header::RETRY_AFTER]
        .to_str()
        .unwrap()
        .parse()
        .unwrap();
    assert!((1..=30).contains(&wait), "{wait}");
    let body = json_body(refused).await;
    assert_eq!(body["error"]["code"], "too_many_attempts");
    assert!(
        body["error"]["message"].as_str().unwrap().contains("seconds"),
        "{body}"
    );

    // Even the right password waits: otherwise each refusal would still be a guess answered.
    let right = sign_in(&app, "203.0.113.9", "OWNER", PASSWORD).await;
    assert_eq!(right.status(), StatusCode::TOO_MANY_REQUESTS);
    assert!(right.headers().get(header::SET_COOKIE).is_none());
}

#[tokio::test]
async fn guessing_at_a_name_does_not_lock_its_owner_out() {
    let app = TestApp::new().await;
    app.setup_owner().await;
    app.create_user("friend", GlobalRole::Member).await;
    guess_wrong(&app, "203.0.113.9", "owner", FREE_ATTEMPTS + 1).await;

    let from_home = sign_in(&app, "198.51.100.7", "owner", PASSWORD).await;
    assert_eq!(from_home.status(), StatusCode::OK, "another address");
    let other_account = sign_in(&app, "203.0.113.9", "friend", PASSWORD).await;
    assert_eq!(
        other_account.status(),
        StatusCode::OK,
        "another account from the guessing address"
    );
}

#[tokio::test]
async fn signing_in_starts_the_count_again() {
    let app = TestApp::new().await;
    app.setup_owner().await;
    for _ in 0..3 {
        guess_wrong(&app, "203.0.113.9", "owner", FREE_ATTEMPTS).await;
        let right = sign_in(&app, "203.0.113.9", "owner", PASSWORD).await;
        assert_eq!(right.status(), StatusCode::OK);
    }
}
