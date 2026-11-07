//! End-to-end tests of the auth, setup and static-asset API.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode, header},
    routing::get,
};
use common::{PASSWORD, TestApp, json_body, session_cookie, text_body};
use gsd_app::{
    http::{
        self,
        authorize::{Authorized, AuthorizedGlobal, perm},
    },
    state::SetupGate,
};
use gsd_core::{
    auth::{GlobalRole, Grant},
    ids::InstanceId,
};
use serde_json::json;

#[tokio::test]
async fn health_and_meta_are_public() {
    let app = TestApp::new().await;
    assert_eq!(app.get("/api/v1/health", None).await.status(), StatusCode::OK);

    let meta = json_body(app.get("/api/v1/meta", None).await).await;
    assert_eq!(meta["name"], gsd_brand::NAME);
    assert_eq!(meta["slug"], gsd_brand::SLUG);
}

#[tokio::test]
async fn first_run_setup_signs_the_owner_in_once() {
    let app = TestApp::new().await;
    let status = json_body(app.get("/api/v1/setup/status", None).await).await;
    assert_eq!(status["setup_required"], true);
    assert_eq!(status["token_required"], false);

    let cookie = app.setup_owner().await;
    let me = json_body(app.get("/api/v1/me", Some(&cookie)).await).await;
    assert_eq!(me["user"]["username"], "owner");
    assert_eq!(me["user"]["role"], "owner");
    assert_eq!(me["all_instances"], true);
    assert!(
        me["global_permissions"]
            .as_array()
            .unwrap()
            .contains(&json!("users.manage"))
    );

    let again = app
        .post(
            "/api/v1/setup",
            json!({ "username": "x2", "password": PASSWORD }),
            None,
        )
        .await;
    assert_eq!(again.status(), StatusCode::CONFLICT);
    assert_eq!(json_body(again).await["error"]["code"], "already_set_up");
}

#[tokio::test]
async fn setup_requires_the_token_when_exposed_to_the_network() {
    let mut app = TestApp::new().await;
    app.state.setup_gate = SetupGate::with_token("let-me-in");
    let router = http::router(app.state.clone());
    let app = app.with_router(router);

    let status = json_body(app.get("/api/v1/setup/status", None).await).await;
    assert_eq!(status["token_required"], true);

    let without = app
        .post(
            "/api/v1/setup",
            json!({ "username": "owner", "password": PASSWORD }),
            None,
        )
        .await;
    assert_eq!(without.status(), StatusCode::FORBIDDEN);
    assert_eq!(json_body(without).await["error"]["code"], "setup_token_invalid");

    let with = app
        .post(
            "/api/v1/setup",
            json!({ "username": "owner", "password": PASSWORD, "setup_token": "let-me-in" }),
            None,
        )
        .await;
    assert_eq!(with.status(), StatusCode::CREATED);
}

#[tokio::test]
async fn login_logout_and_bad_credentials() {
    let app = TestApp::new().await;
    app.setup_owner().await;

    let wrong = app
        .post(
            "/api/v1/auth/login",
            json!({ "username": "owner", "password": "nope nope nope" }),
            None,
        )
        .await;
    assert_eq!(wrong.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(json_body(wrong).await["error"]["code"], "invalid_credentials");

    let cookie = app.login("owner").await;
    assert_eq!(
        app.get("/api/v1/me", Some(&cookie)).await.status(),
        StatusCode::OK
    );

    let logout = app.post("/api/v1/auth/logout", json!({}), Some(&cookie)).await;
    assert_eq!(logout.status(), StatusCode::NO_CONTENT);
    assert!(
        logout.headers()[header::SET_COOKIE]
            .to_str()
            .unwrap()
            .contains("Max-Age=0")
    );
    assert_eq!(
        app.get("/api/v1/me", Some(&cookie)).await.status(),
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn state_changing_requests_need_the_csrf_header_and_same_origin() {
    let app = TestApp::new().await;
    let no_header = Request::post("/api/v1/setup")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            json!({ "username": "owner", "password": PASSWORD }).to_string(),
        ))
        .unwrap();
    let response = app.send(no_header).await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert_eq!(json_body(response).await["error"]["code"], "csrf_failed");

    let cross_site = Request::post("/api/v1/auth/login")
        .header(header::CONTENT_TYPE, "application/json")
        .header("x-csrf", "1")
        .header(header::HOST, "panel.example.com")
        .header(header::ORIGIN, "https://evil.example")
        .body(Body::from("{}"))
        .unwrap();
    assert_eq!(app.send(cross_site).await.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn malformed_json_gets_a_json_error() {
    let app = TestApp::new().await;
    let request = Request::post("/api/v1/auth/login")
        .header(header::CONTENT_TYPE, "application/json")
        .header("x-csrf", "1")
        .body(Body::from("{not json"))
        .unwrap();
    let response = app.send(request).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(json_body(response).await["error"]["code"], "invalid_request_body");
}

#[tokio::test]
async fn password_change_is_enforced_after_a_reset() {
    let app = TestApp::new().await;
    app.setup_owner().await;
    let temporary = app.state.auth.reset_password("owner").await.unwrap();

    let login = app
        .post(
            "/api/v1/auth/login",
            json!({ "username": "owner", "password": temporary }),
            None,
        )
        .await;
    let cookie = session_cookie(&login);
    assert_eq!(json_body(login).await["user"]["must_change_password"], true);
    // Normal endpoints are refused until the password is changed.
    let blocked = app.get("/api/v1/instances", Some(&cookie)).await;
    assert_eq!(blocked.status(), StatusCode::FORBIDDEN);
    assert_eq!(
        json_body(blocked).await["error"]["code"],
        "password_change_required"
    );

    let change = app
        .post(
            "/api/v1/me/password",
            json!({ "current_password": temporary, "new_password": "a fresh passphrase" }),
            Some(&cookie),
        )
        .await;
    assert_eq!(change.status(), StatusCode::OK);
    let fresh_cookie = session_cookie(&change);
    let me = json_body(app.get("/api/v1/me", Some(&fresh_cookie)).await).await;
    assert_eq!(me["user"]["must_change_password"], false);
}

#[tokio::test]
async fn authorized_extractor_enforces_instance_grants() {
    async fn read_files(auth: Authorized<perm::FilesRead>) -> String {
        auth.instance_id.to_string()
    }
    async fn create_instance(_auth: AuthorizedGlobal<perm::InstanceCreate>) -> &'static str {
        "ok"
    }

    // Sign everyone in through the real API, then swap in a router with probe routes.
    let real = TestApp::new().await;
    let owner = real.setup_owner().await;
    let member_id = real.create_user("friend", GlobalRole::Member).await;
    let friend = real.login("friend").await;

    let probes = Router::new()
        .route("/instances/{instance_id}/files", get(read_files))
        .route("/instances/new", get(create_instance))
        .with_state(real.state.clone());
    let app = real.with_router(probes);

    let granted = InstanceId::generate();
    let viewer_only = InstanceId::generate();
    app.state
        .auth
        .grants()
        .set(member_id, granted, Grant::Manager)
        .await
        .unwrap();
    app.state
        .auth
        .grants()
        .set(member_id, viewer_only, Grant::Viewer)
        .await
        .unwrap();

    // Owner: implicit access everywhere.
    let path = |id: InstanceId| format!("/instances/{id}/files");
    assert_eq!(
        app.get(&path(InstanceId::generate()), Some(&owner))
            .await
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        app.get("/instances/new", Some(&owner)).await.status(),
        StatusCode::OK
    );

    // Member: allowed where granted, 403 where they can only view, 404 where they have nothing.
    let ok = app.get(&path(granted), Some(&friend)).await;
    assert_eq!(ok.status(), StatusCode::OK);
    assert_eq!(text_body(ok).await, granted.to_string());

    let forbidden = app.get(&path(viewer_only), Some(&friend)).await;
    assert_eq!(forbidden.status(), StatusCode::FORBIDDEN);
    assert_eq!(json_body(forbidden).await["error"]["code"], "missing_permission");

    let hidden = app.get(&path(InstanceId::generate()), Some(&friend)).await;
    assert_eq!(hidden.status(), StatusCode::NOT_FOUND);

    assert_eq!(
        app.get("/instances/new", Some(&friend)).await.status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        app.get(&path(granted), None).await.status(),
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn web_ui_is_served_and_templated() {
    let app = TestApp::new().await;

    let index = app.get("/", None).await;
    assert_eq!(index.status(), StatusCode::OK);
    assert!(
        index.headers()[header::CONTENT_SECURITY_POLICY]
            .to_str()
            .unwrap()
            .contains("script-src 'self'")
    );
    let html = text_body(index).await;
    assert!(html.contains(&format!("<title>{}</title>", gsd_brand::NAME)));
    assert!(!html.contains("{{APP_NAME}}"));

    // Client-side routes fall back to the shell; missing files are real 404s.
    assert_eq!(app.get("/instances/abc", None).await.status(), StatusCode::OK);
    assert_eq!(
        app.get("/js/missing.js", None).await.status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        app.get("/api/v1/nope", None).await.status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        app.get("/../Cargo.toml", None).await.status(),
        StatusCode::NOT_FOUND
    );

    let script = app.get("/js/main.js", None).await;
    assert!(
        script.headers()[header::CONTENT_TYPE]
            .to_str()
            .unwrap()
            .contains("javascript")
    );
    let etag = script.headers()[header::ETAG].clone();
    let cached = app
        .send(
            Request::get("/js/main.js")
                .header(header::IF_NONE_MATCH, etag)
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(cached.status(), StatusCode::NOT_MODIFIED);
}
