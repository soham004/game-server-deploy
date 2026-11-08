//! Deny by default (`http/access.rs`): without a valid session, every route refuses, pages lead to
//! the sign-in page, and only signing in (plus first-run setup, until it is done) is open. The
//! route table is read from the source, so a route added later is covered without touching this.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use axum::{
    body::Body,
    http::{Method, Request, StatusCode, header},
    response::Response,
};
use common::{TestApp, json_body};
use gsd_app::http::access::{SETUP_FILES, SIGN_IN_FILES};

/// What a visitor without a session may call once the app is set up.
const OPEN_AFTER_SETUP: &[(&str, &str)] = &[
    ("POST", "/api/v1/auth/login"),
    ("POST", "/api/v1/auth/logout"),
    ("GET", "/api/v1/health"),
];

const METHODS: [Method; 5] = [
    Method::GET,
    Method::POST,
    Method::PUT,
    Method::PATCH,
    Method::DELETE,
];

/// Sends `method path` with an optional cookie and a JSON body, without the CSRF header (the
/// session check comes first, so it shouldn't matter).
async fn send(app: &TestApp, method: &Method, path: &str, cookie: Option<&str>) -> Response {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(cookie) = cookie {
        request = request.header(header::COOKIE, cookie);
    }
    app.send(request.body(Body::from("{}")).unwrap()).await
}

async fn assert_refused(response: Response, what: &str) {
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "{what}");
    assert_eq!(
        json_body(response).await["error"]["code"],
        "unauthenticated",
        "{what}"
    );
}

fn location(response: &Response) -> &str {
    response
        .headers()
        .get(header::LOCATION)
        .map_or("", |value| value.to_str().unwrap())
}

/// Every API route, from `.route("…")` in the route modules, with `{parameters}` filled in.
fn api_routes() -> Vec<String> {
    let http = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/http");
    let mut sources: Vec<std::path::PathBuf> = std::fs::read_dir(http.join("routes"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    sources.push(http.join("ws.rs"));
    let mut routes = Vec::new();
    for source in sources {
        let text = std::fs::read_to_string(&source).unwrap();
        for (index, _) in text.match_indices(".route(") {
            let rest = text[index + ".route(".len()..].trim_start();
            let path = rest.strip_prefix('"').and_then(|rest| rest.split('"').next());
            let path = path.unwrap_or_else(|| panic!("a .route( without a path in {}", source.display()));
            routes.push(format!("/api/v1{}", fill_parameters(path)));
        }
    }
    routes.sort();
    routes.dedup();
    routes
}

/// `/instances/{instance_id}/files` → `/instances/01JAAAAAAAAAAAAAAAAAAAAAAA/files` (a valid id,
/// so a route that parses its parameters first would still answer something other than 401).
fn fill_parameters(path: &str) -> String {
    path.split('/')
        .map(|segment| {
            if segment.starts_with('{') {
                "01JAAAAAAAAAAAAAAAAAAAAAAA"
            } else {
                segment
            }
        })
        .collect::<Vec<_>>()
        .join("/")
}

#[tokio::test]
async fn every_route_refuses_a_visitor_without_a_session() {
    let app = TestApp::new().await;
    app.setup_owner().await;
    let routes = api_routes();
    assert!(routes.len() > 50, "the route table was read: {routes:?}");

    for path in &routes {
        for method in &METHODS {
            let open = OPEN_AFTER_SETUP.contains(&(method.as_str(), path.as_str()));
            let response = send(&app, method, path, None).await;
            if open {
                assert_ne!(response.status(), StatusCode::UNAUTHORIZED, "{method} {path}");
            } else {
                assert_refused(response, &format!("{method} {path}")).await;
            }
        }
    }
    // Paths that don't exist answer exactly like ones that do, so the API can't be mapped.
    for path in [
        "/api/v1/nope",
        "/api/v1/instances/x/nope",
        "/api/v1/",
        "/api/v1/../../etc/passwd",
    ] {
        assert_refused(send(&app, &Method::GET, path, None).await, path).await;
    }
}

#[tokio::test]
async fn an_invalid_or_ended_session_is_no_session() {
    let app = TestApp::new().await;
    let cookie = app.setup_owner().await;
    let name = cookie.split('=').next().unwrap();
    for bad in [
        format!("{name}=garbage"),
        format!("{name}="),
        "other=1".to_owned(),
    ] {
        assert_refused(
            send(&app, &Method::GET, "/api/v1/instances", Some(&bad)).await,
            &bad,
        )
        .await;
        let page = send(&app, &Method::GET, "/", Some(&bad)).await;
        assert_eq!(
            (page.status(), location(&page)),
            (StatusCode::SEE_OTHER, "/login.html")
        );
    }

    assert_eq!(
        send(&app, &Method::GET, "/api/v1/instances", Some(&cookie))
            .await
            .status(),
        StatusCode::OK
    );
    let mut logout = Request::post("/api/v1/auth/logout")
        .header("X-CSRF", "1")
        .header(header::COOKIE, &cookie);
    logout = logout.header(header::CONTENT_TYPE, "application/json");
    app.send(logout.body(Body::empty()).unwrap()).await;
    assert_refused(
        send(&app, &Method::GET, "/api/v1/instances", Some(&cookie)).await,
        "after signing out",
    )
    .await;
}

#[tokio::test]
async fn pages_lead_to_sign_in_and_the_app_files_need_a_session() {
    let app = TestApp::new().await;
    let cookie = app.setup_owner().await;

    for file in SIGN_IN_FILES {
        let response = app.get(&format!("/{file}"), None).await;
        assert_eq!(response.status(), StatusCode::OK, "{file}");
    }
    for (page, to) in [
        ("/", "/login.html"),
        ("/setup.html", "/login.html"),
        ("/users", "/login.html?next=%2Fusers"),
        (
            "/servers/01ABC?tab=files",
            "/login.html?next=%2Fservers%2F01ABC%3Ftab%3Dfiles",
        ),
    ] {
        let response = app.get(page, None).await;
        assert_eq!(
            (response.status(), location(&response)),
            (StatusCode::SEE_OTHER, to),
            "{page}"
        );
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    }
    for file in [
        "/js/main.js",
        "/js/views/users.js",
        "/js/pages/setup.js",
        "/css/layout.css",
        "/missing.js",
    ] {
        assert_refused(app.get(file, None).await, file).await;
    }

    // Signed in, all of it is there.
    for path in ["/", "/users", "/js/main.js", "/js/views/users.js", "/api/v1/meta"] {
        assert_eq!(
            app.get(path, Some(&cookie)).await.status(),
            StatusCode::OK,
            "{path}"
        );
    }
}

#[tokio::test]
async fn before_the_first_account_only_setup_is_open() {
    let app = TestApp::new().await;
    for file in SETUP_FILES {
        assert_eq!(
            app.get(&format!("/{file}"), None).await.status(),
            StatusCode::OK,
            "{file}"
        );
    }
    let status = json_body(app.get("/api/v1/setup/status", None).await).await;
    assert_eq!(status["setup_required"], true);
    for page in ["/", "/login.html", "/servers/x"] {
        let response = app.get(page, None).await;
        assert_eq!(
            (response.status(), location(&response)),
            (StatusCode::SEE_OTHER, "/setup.html"),
            "{page}"
        );
    }
    assert_refused(app.get("/js/pages/login.js", None).await, "the sign-in script").await;
    assert_refused(app.get("/api/v1/meta", None).await, "meta").await;

    // Once the owner exists, setup is closed like everything else.
    app.setup_owner().await;
    assert_refused(app.get("/api/v1/setup/status", None).await, "setup status").await;
    assert_refused(
        app.post("/api/v1/setup", serde_json::json!({}), None).await,
        "setup again",
    )
    .await;
}

#[tokio::test]
async fn signed_in_mistakes_get_json_errors() {
    let app = TestApp::new().await;
    let cookie = app.setup_owner().await;
    let put = Request::put("/api/v1/instances")
        .header(header::COOKIE, &cookie)
        .header("X-CSRF", "1")
        .body(Body::empty())
        .unwrap();
    let wrong_method = app.send(put).await;
    assert_eq!(wrong_method.status(), StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(
        json_body(wrong_method).await["error"]["code"],
        "method_not_allowed"
    );
    let unknown = app.get("/api/v1/nope", Some(&cookie)).await;
    assert_eq!(unknown.status(), StatusCode::NOT_FOUND);
    assert_eq!(json_body(unknown).await["error"]["code"], "not_found");
}
