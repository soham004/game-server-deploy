//! User management API: accounts, roles, the last-owner rule, password resets, sign-outs, and
//! per-instance grants taking effect.

#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use axum::{
    body::Body,
    http::{Request, StatusCode, header},
};
use common::{PASSWORD, TestApp, json_body, session_cookie};
use gsd_core::auth::GlobalRole;
use serde_json::{Value, json};

async fn patch(app: &TestApp, path: &str, body: Value, cookie: &str) -> axum::response::Response {
    let request = Request::patch(path)
        .header(header::COOKIE, cookie)
        .header("x-csrf", "1")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    app.send(request).await
}

async fn login_with(app: &TestApp, username: &str, password: &str) -> axum::response::Response {
    app.post(
        "/api/v1/auth/login",
        json!({ "username": username, "password": password }),
        None,
    )
    .await
}

#[tokio::test]
async fn owners_create_users_who_must_change_their_temporary_password() {
    let app = TestApp::new().await;
    let owner = app.setup_owner().await;

    let response = app
        .post(
            "/api/v1/users",
            json!({ "username": "friend", "role": "member" }),
            Some(&owner),
        )
        .await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let created = json_body(response).await;
    assert_eq!(created["user"]["must_change_password"], true);
    let temporary = created["temporary_password"].as_str().unwrap().to_owned();

    let login = login_with(&app, "friend", &temporary).await;
    assert_eq!(login.status(), StatusCode::OK);
    assert_eq!(json_body(login).await["user"]["must_change_password"], true);

    let taken = app
        .post(
            "/api/v1/users",
            json!({ "username": "Friend", "role": "member" }),
            Some(&owner),
        )
        .await;
    assert_eq!(json_body(taken).await["error"]["code"], "username_taken");

    let users = json_body(app.get("/api/v1/users", Some(&owner)).await).await;
    let rows: Vec<(String, String, i64)> = users
        .as_array()
        .unwrap()
        .iter()
        .map(|u| {
            (
                u["username"].as_str().unwrap().into(),
                u["role"].as_str().unwrap().into(),
                u["active_sessions"].as_i64().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        rows,
        [
            ("friend".into(), "member".into(), 1),
            ("owner".into(), "owner".into(), 1)
        ]
    );
}

#[tokio::test]
async fn admins_and_members_can_not_manage_users() {
    let app = TestApp::new().await;
    app.setup_owner().await;
    app.create_user("admin", GlobalRole::Admin).await;
    let admin = app.login("admin").await;
    assert_eq!(
        app.get("/api/v1/users", Some(&admin)).await.status(),
        StatusCode::FORBIDDEN
    );
    let create = app
        .post(
            "/api/v1/users",
            json!({ "username": "sneaky", "role": "owner" }),
            Some(&admin),
        )
        .await;
    assert_eq!(create.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn roles_status_and_the_last_owner_rule() {
    let app = TestApp::new().await;
    let owner = app.setup_owner().await;
    let users = json_body(app.get("/api/v1/users", Some(&owner)).await).await;
    let owner_id = users[0]["id"].as_str().unwrap().to_owned();
    let admin_id = app.create_user("admin", GlobalRole::Admin).await.to_string();

    // Own account: refused. Demoting the only owner: refused.
    let own = patch(
        &app,
        &format!("/api/v1/users/{owner_id}"),
        json!({ "role": "member" }),
        &owner,
    )
    .await;
    assert_eq!(json_body(own).await["error"]["code"], "own_account");
    let delete_self = app
        .delete(&format!("/api/v1/users/{owner_id}"), Some(&owner))
        .await;
    assert_eq!(delete_self.status(), StatusCode::CONFLICT);

    // Promote the admin; now it may demote the first owner.
    let promoted = json_body(
        patch(
            &app,
            &format!("/api/v1/users/{admin_id}"),
            json!({ "role": "owner" }),
            &owner,
        )
        .await,
    )
    .await;
    assert_eq!(promoted["role"], "owner");
    let new_owner = app.login("admin").await;
    let demoted = patch(
        &app,
        &format!("/api/v1/users/{owner_id}"),
        json!({ "role": "admin" }),
        &new_owner,
    )
    .await;
    assert_eq!(demoted.status(), StatusCode::OK);
    // The original owner, now an admin, lost user management at once.
    assert_eq!(
        app.get("/api/v1/users", Some(&owner)).await.status(),
        StatusCode::FORBIDDEN
    );
    // And the last remaining owner is protected.
    let last = patch(
        &app,
        &format!("/api/v1/users/{admin_id}"),
        json!({ "disabled": true }),
        &new_owner,
    )
    .await;
    assert_eq!(json_body(last).await["error"]["code"], "own_account");

    let unknown = patch(
        &app,
        "/api/v1/users/01M6VA0G5S7PFEFWDHTQ3VB0GS",
        json!({ "disabled": true }),
        &new_owner,
    )
    .await;
    assert_eq!(unknown.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn disable_reset_sign_out_and_delete() {
    let app = TestApp::new().await;
    let owner = app.setup_owner().await;
    let id = app.create_user("friend", GlobalRole::Member).await.to_string();
    let friend = app.login("friend").await;
    assert_eq!(
        app.get("/api/v1/me", Some(&friend)).await.status(),
        StatusCode::OK
    );

    patch(
        &app,
        &format!("/api/v1/users/{id}"),
        json!({ "disabled": true }),
        &owner,
    )
    .await;
    assert_eq!(
        app.get("/api/v1/me", Some(&friend)).await.status(),
        StatusCode::UNAUTHORIZED,
        "signed out"
    );
    assert_eq!(
        login_with(&app, "friend", PASSWORD).await.status(),
        StatusCode::UNAUTHORIZED
    );
    patch(
        &app,
        &format!("/api/v1/users/{id}"),
        json!({ "disabled": false }),
        &owner,
    )
    .await;

    let friend = session_cookie(&login_with(&app, "friend", PASSWORD).await);
    let reset = json_body(
        app.post(
            &format!("/api/v1/users/{id}/reset-password"),
            json!({}),
            Some(&owner),
        )
        .await,
    )
    .await;
    let temporary = reset["temporary_password"].as_str().unwrap();
    assert_eq!(
        app.get("/api/v1/me", Some(&friend)).await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        login_with(&app, "friend", PASSWORD).await.status(),
        StatusCode::UNAUTHORIZED
    );
    let friend = session_cookie(&login_with(&app, "friend", temporary).await);

    assert_eq!(
        app.post(&format!("/api/v1/users/{id}/sign-out"), json!({}), Some(&owner))
            .await
            .status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        app.get("/api/v1/me", Some(&friend)).await.status(),
        StatusCode::UNAUTHORIZED
    );

    assert_eq!(
        app.delete(&format!("/api/v1/users/{id}"), Some(&owner))
            .await
            .status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        login_with(&app, "friend", temporary).await.status(),
        StatusCode::UNAUTHORIZED
    );

    let actions: Vec<String> = sqlx::query_scalar("SELECT action FROM audit_log WHERE action LIKE 'user.%' OR action = 'auth.password_reset' ORDER BY id")
        .fetch_all(&app.pool)
        .await
        .unwrap();
    assert_eq!(
        actions,
        [
            "user.updated",
            "user.updated",
            "auth.password_reset",
            "user.signed_out",
            "user.deleted"
        ]
    );
}

#[tokio::test]
async fn grants_give_members_access_to_single_servers() {
    let app = TestApp::new().await;
    let owner = app.setup_owner().await;
    let created = json_body(
        app.post(
            "/api/v1/instances",
            json!({ "name": "Shared", "mc_version": "26.3", "loader": "vanilla", "max_memory_mb": 1024, "eula_accepted": true }),
            Some(&owner),
        )
        .await,
    )
    .await;
    let instance = created["instance"]["id"].as_str().unwrap().to_owned();
    app.wait_for_job(created["job_id"].as_str().unwrap(), &owner)
        .await;
    let member = app.create_user("member", GlobalRole::Member).await.to_string();
    let member_cookie = app.login("member").await;
    let grant_url = format!("/api/v1/users/{member}/grants/{instance}");

    assert_eq!(
        app.get("/api/v1/instances", Some(&member_cookie)).await.status(),
        StatusCode::OK
    );
    assert_eq!(
        json_body(app.get("/api/v1/instances", Some(&member_cookie)).await).await,
        json!([])
    );

    assert_eq!(
        app.put(&grant_url, json!({ "grant": "operator" }), Some(&owner))
            .await
            .status(),
        StatusCode::NO_CONTENT
    );
    let visible = json_body(app.get("/api/v1/instances", Some(&member_cookie)).await).await;
    assert_eq!(visible[0]["id"], instance.as_str());
    assert!(
        visible[0]["permissions"]
            .as_array()
            .unwrap()
            .contains(&json!("lifecycle"))
    );
    let grants = json_body(
        app.get(&format!("/api/v1/users/{member}/grants"), Some(&owner))
            .await,
    )
    .await;
    assert_eq!(grants[0]["instance_name"], "Shared");
    assert_eq!(grants[0]["grant"], "operator");

    let custom = app
        .put(
            &grant_url,
            json!({ "grant": "custom", "permissions": ["instance.view", "files.read"] }),
            Some(&owner),
        )
        .await;
    assert_eq!(custom.status(), StatusCode::NO_CONTENT);
    let files = app
        .get(
            &format!("/api/v1/instances/{instance}/files"),
            Some(&member_cookie),
        )
        .await;
    assert_eq!(files.status(), StatusCode::OK, "custom grant includes files.read");
    let start = app
        .post(
            &format!("/api/v1/instances/{instance}/start"),
            json!({}),
            Some(&member_cookie),
        )
        .await;
    assert_eq!(start.status(), StatusCode::FORBIDDEN, "but not lifecycle");

    let bad = app
        .put(&grant_url, json!({ "grant": "superuser" }), Some(&owner))
        .await;
    assert_eq!(json_body(bad).await["error"]["code"], "invalid_grant");
    let no_instance = app
        .put(
            &format!("/api/v1/users/{member}/grants/01M6VA0G5S7PFEFWDHTQ3VB0GS"),
            json!({ "grant": "viewer" }),
            Some(&owner),
        )
        .await;
    assert_eq!(no_instance.status(), StatusCode::NOT_FOUND);

    assert_eq!(
        app.delete(&grant_url, Some(&owner)).await.status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        json_body(app.get("/api/v1/instances", Some(&member_cookie)).await).await,
        json!([])
    );
}
