//! The file manager API: every route, permissions, download headers, and the containment
//! guarantees seen from the outside.

#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use axum::{
    body::Body,
    http::{Request, StatusCode, header},
};
use common::{TestApp, json_body};
use gsd_core::{
    auth::{GlobalRole, Grant},
    ids::InstanceId,
};
use http_body_util::BodyExt;
use serde_json::{Value, json};

struct Server {
    id: String,
    dir: std::path::PathBuf,
}

async fn server(app: &TestApp, cookie: &str) -> Server {
    let created = json_body(
        app.post(
            "/api/v1/instances",
            json!({ "name": "Files", "mc_version": "26.3", "loader": "vanilla", "max_memory_mb": 1024, "eula_accepted": true }),
            Some(cookie),
        )
        .await,
    )
    .await;
    let job = app
        .wait_for_job(created["job_id"].as_str().unwrap(), cookie)
        .await;
    assert_eq!(job["status"], "succeeded");
    let slug = created["instance"]["slug"].as_str().unwrap();
    Server {
        id: created["instance"]["id"].as_str().unwrap().to_owned(),
        dir: app.state.config.instances_dir().join(slug).join("server"),
    }
}

fn url(server: &Server, route: &str) -> String {
    format!("/api/v1/instances/{}/files{route}", server.id)
}

async fn upload(
    app: &TestApp,
    server: &Server,
    path: &str,
    bytes: &'static [u8],
    overwrite: bool,
    cookie: &str,
) -> axum::response::Response {
    let request = Request::put(format!(
        "{}?path={path}&overwrite={overwrite}",
        url(server, "/upload")
    ))
    .header(header::COOKIE, cookie)
    .header("x-csrf", "1")
    .header(header::CONTENT_TYPE, "application/octet-stream")
    .body(Body::from(bytes))
    .unwrap();
    app.send(request).await
}

async fn bytes(response: axum::response::Response) -> Vec<u8> {
    response.into_body().collect().await.unwrap().to_bytes().to_vec()
}

#[tokio::test]
async fn browse_and_edit_text() {
    let app = TestApp::new().await;
    let cookie = app.setup_owner().await;
    let s = server(&app, &cookie).await;

    let listing = json_body(app.get(&url(&s, ""), Some(&cookie)).await).await;
    assert_eq!(listing["path"], "");
    assert_eq!(listing["running"], false);
    let names: Vec<&str> = listing["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["name"].as_str().unwrap())
        .collect();
    assert!(
        names.contains(&"eula.txt") && names.contains(&"server.properties"),
        "{names:?}"
    );

    // Create a folder, write a new file in it, edit it with its revision.
    assert_eq!(
        app.post(&url(&s, "/folder"), json!({ "path": "config" }), Some(&cookie))
            .await
            .status(),
        StatusCode::CREATED
    );
    let created = json_body(
        app.put(
            &url(&s, "/text"),
            json!({ "path": "config/a.yml", "content": "a: 1\n" }),
            Some(&cookie),
        )
        .await,
    )
    .await;
    let opened = json_body(app.get(&url(&s, "/text?path=config/a.yml"), Some(&cookie)).await).await;
    assert_eq!(opened["content"], "a: 1\n");
    assert_eq!(opened["revision"], created["revision"]);
    let saved = app
        .put(
            &url(&s, "/text"),
            json!({ "path": "config/a.yml", "content": "a: 2\n", "revision": opened["revision"] }),
            Some(&cookie),
        )
        .await;
    assert_eq!(saved.status(), StatusCode::OK);
    let stale = app
        .put(
            &url(&s, "/text"),
            json!({ "path": "config/a.yml", "content": "a: 3\n", "revision": opened["revision"] }),
            Some(&cookie),
        )
        .await;
    assert_eq!(stale.status(), StatusCode::CONFLICT);
    assert_eq!(json_body(stale).await["error"]["code"], "file_changed");
    assert_eq!(
        std::fs::read_to_string(s.dir.join("config/a.yml")).unwrap(),
        "a: 2\n"
    );
}

#[tokio::test]
async fn copy_rename_delete_and_binary_refusal() {
    let app = TestApp::new().await;
    let cookie = app.setup_owner().await;
    let s = server(&app, &cookie).await;
    app.post(&url(&s, "/folder"), json!({ "path": "config" }), Some(&cookie))
        .await;
    app.put(
        &url(&s, "/text"),
        json!({ "path": "config/a.yml", "content": "a: 2\n" }),
        Some(&cookie),
    )
    .await;

    // Copy, rename, delete (to the trash).
    let ok = |status: StatusCode| status.is_success();
    assert!(ok(app
        .post(
            &url(&s, "/copy"),
            json!({ "from": "config", "to": "config-old" }),
            Some(&cookie)
        )
        .await
        .status()));
    assert!(ok(app
        .post(
            &url(&s, "/rename"),
            json!({ "from": "config-old", "to": "archive" }),
            Some(&cookie)
        )
        .await
        .status()));
    assert!(s.dir.join("archive/a.yml").is_file());
    let exists = app
        .post(
            &url(&s, "/rename"),
            json!({ "from": "archive", "to": "config" }),
            Some(&cookie),
        )
        .await;
    assert_eq!(exists.status(), StatusCode::CONFLICT);
    assert!(ok(app
        .post(
            &url(&s, "/delete"),
            json!({ "paths": ["archive"] }),
            Some(&cookie)
        )
        .await
        .status()));
    assert!(!s.dir.join("archive").exists());
    assert!(
        s.dir
            .parent()
            .unwrap()
            .join("trash")
            .read_dir()
            .unwrap()
            .next()
            .is_some()
    );

    // Binary files are refused by the editor.
    upload(
        &app,
        &s,
        "server.jar.copy",
        b"PK\x03\x04\x00binary",
        false,
        &cookie,
    )
    .await;
    let binary = app
        .get(&url(&s, "/text?path=server.jar.copy"), Some(&cookie))
        .await;
    assert_eq!(json_body(binary).await["error"]["code"], "not_text");
}

#[tokio::test]
async fn uploads_and_downloads() {
    let app = TestApp::new().await;
    let cookie = app.setup_owner().await;
    let s = server(&app, &cookie).await;
    app.post(&url(&s, "/folder"), json!({ "path": "mods" }), Some(&cookie))
        .await;

    let response = upload(&app, &s, "mods/caf%C3%A9.jar", b"jar bytes", false, &cookie).await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let entry = json_body(response).await;
    assert_eq!(
        (entry["name"].as_str(), entry["size"].as_u64()),
        (Some("caf\u{e9}.jar"), Some(9))
    );
    assert_eq!(
        upload(&app, &s, "mods/caf%C3%A9.jar", b"again", false, &cookie)
            .await
            .status(),
        StatusCode::CONFLICT
    );
    assert_eq!(
        upload(&app, &s, "mods/caf%C3%A9.jar", b"again", true, &cookie)
            .await
            .status(),
        StatusCode::CREATED
    );

    let download = app
        .get(&url(&s, "/download?path=mods/caf%C3%A9.jar"), Some(&cookie))
        .await;
    assert_eq!(download.status(), StatusCode::OK);
    let headers = download.headers().clone();
    assert_eq!(
        headers[header::CONTENT_TYPE],
        "application/octet-stream",
        "never rendered inline"
    );
    assert_eq!(headers[header::CONTENT_LENGTH], "5");
    let disposition = headers[header::CONTENT_DISPOSITION].to_str().unwrap();
    assert!(
        disposition.starts_with("attachment;") && disposition.contains("filename*=UTF-8''caf%C3%A9.jar"),
        "{disposition}"
    );
    assert_eq!(bytes(download).await, b"again");

    // A folder downloads as a zip that contains it.
    let zipped = app.get(&url(&s, "/download?path=mods"), Some(&cookie)).await;
    assert!(
        zipped.headers()[header::CONTENT_DISPOSITION]
            .to_str()
            .unwrap()
            .contains("mods.zip")
    );
    let archive = zip::ZipArchive::new(std::io::Cursor::new(bytes(zipped).await)).unwrap();
    let names: Vec<String> = archive.file_names().map(|n| n.unwrap().into_owned()).collect();
    assert!(names.contains(&"mods/caf\u{e9}.jar".to_owned()), "{names:?}");

    // Compress, then extract somewhere else.
    let compressed = app
        .post(
            &url(&s, "/compress"),
            json!({ "paths": ["mods"], "destination": "mods.zip" }),
            Some(&cookie),
        )
        .await;
    assert_eq!(compressed.status(), StatusCode::CREATED);
    let summary = json_body(
        app.post(
            &url(&s, "/extract"),
            json!({ "path": "mods.zip", "destination": "restored" }),
            Some(&cookie),
        )
        .await,
    )
    .await;
    assert_eq!(summary["files"], 1);
    assert!(s.dir.join("restored/mods/caf\u{e9}.jar").is_file());
}

/// Downloads arrive intact and unencoded whatever `Accept-Encoding` the client sends (browsers
/// ask for gzip/br/zstd).
#[tokio::test]
async fn downloads_survive_compression_negotiation() {
    let app = TestApp::new().await;
    let cookie = app.setup_owner().await;
    let s = server(&app, &cookie).await;
    upload(&app, &s, "data.bin", b"0123456789", false, &cookie).await;
    app.post(&url(&s, "/folder"), json!({ "path": "folder" }), Some(&cookie))
        .await;
    upload(&app, &s, "folder/inside.txt", b"inside", false, &cookie).await;
    app.post(
        &url(&s, "/compress"),
        json!({ "paths": ["folder"], "destination": "folder.zip" }),
        Some(&cookie),
    )
    .await;

    for path in ["data.bin", "folder.zip", "folder"] {
        let request = Request::get(format!("{}?path={path}", url(&s, "/download")))
            .header(header::COOKIE, &cookie)
            .header(header::ACCEPT_ENCODING, "gzip, deflate, br, zstd")
            .body(Body::empty())
            .unwrap();
        let response = app.send(request).await;
        assert_eq!(response.status(), StatusCode::OK, "{path}");
        assert!(
            response.headers().get(header::CONTENT_ENCODING).is_none(),
            "{path}: downloads are sent as they are (no transfer compression)"
        );
        assert!(!bytes(response).await.is_empty(), "{path}");
    }
}

#[tokio::test]
async fn paths_and_links_can_not_leave_the_server_folder() {
    let app = TestApp::new().await;
    let cookie = app.setup_owner().await;
    let s = server(&app, &cookie).await;
    let outside = s
        .dir
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("outside.txt");
    std::fs::write(&outside, "secret").unwrap();
    std::os::unix::fs::symlink(&outside, s.dir.join("link")).unwrap();

    for path in ["../instance.toml", "../../../outside.txt", "a/../../x"] {
        let response = app
            .get(&url(&s, &format!("/text?path={path}")), Some(&cookie))
            .await;
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY, "{path}");
        assert_eq!(json_body(response).await["error"]["code"], "invalid_path");
    }
    for route in ["/text?path=link", "/download?path=link"] {
        let response = app.get(&url(&s, route), Some(&cookie)).await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN, "{route}");
    }
    let planted = app
        .put(
            &url(&s, "/text"),
            json!({ "path": "../planted.txt", "content": "x" }),
            Some(&cookie),
        )
        .await;
    assert_eq!(planted.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(std::fs::read_to_string(&outside).unwrap(), "secret");
}

#[tokio::test]
async fn only_users_with_file_permissions_get_in() {
    let app = TestApp::new().await;
    let owner = app.setup_owner().await;
    let s = server(&app, &owner).await;
    let id: InstanceId = s.id.parse().unwrap();

    let viewer_id = app.create_user("viewer", GlobalRole::Member).await;
    app.state
        .auth
        .grants()
        .set(viewer_id, id, Grant::Viewer)
        .await
        .unwrap();
    let viewer = app.login("viewer").await;
    assert_eq!(
        app.get(&url(&s, ""), Some(&viewer)).await.status(),
        StatusCode::FORBIDDEN
    );

    let manager_id = app.create_user("manager", GlobalRole::Member).await;
    app.state
        .auth
        .grants()
        .set(manager_id, id, Grant::Manager)
        .await
        .unwrap();
    let manager = app.login("manager").await;
    assert_eq!(
        app.get(&url(&s, ""), Some(&manager)).await.status(),
        StatusCode::OK
    );
    assert_eq!(
        app.post(
            &url(&s, "/folder"),
            json!({ "path": "by-manager" }),
            Some(&manager)
        )
        .await
        .status(),
        StatusCode::CREATED
    );

    let audit: Vec<Value> =
        sqlx::query_scalar::<_, String>("SELECT details_json FROM audit_log WHERE action = 'files.changed'")
            .fetch_all(&app.pool)
            .await
            .unwrap()
            .into_iter()
            .map(|d| serde_json::from_str(&d).unwrap())
            .collect();
    assert_eq!(
        audit,
        [json!({ "operation": "create_folder", "paths": ["by-manager"] })]
    );
}
