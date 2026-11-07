//! Shared helpers for the API integration tests: an app on a temporary data directory, a mock
//! of the upstream version APIs, a fake Java that behaves like a Minecraft server, and HTTP
//! helpers.

#![allow(clippy::unwrap_used, clippy::expect_used, dead_code)]

use std::{path::Path, time::Duration};

use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode, header},
    response::Response,
};
use gsd_app::{
    config::Config,
    http,
    state::{AppState, BuildOptions},
};
use gsd_core::{
    auth::{
        GlobalRole, Username, password,
        users::{NewUser, UserRepo},
    },
    store::{self, SqlitePool},
    supervisor::SandboxSupport,
};
use gsd_minecraft::Endpoints;
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{method, path, path_regex},
};

pub const PASSWORD: &str = "correct horse battery";
pub const FAKE_JAR: &[u8] = b"PK\x03\x04 fake server jar";

pub struct TestApp {
    pub state: AppState,
    pub pool: SqlitePool,
    pub router: Router,
    pub upstream: MockServer,
    pub dir: tempfile::TempDir,
}

impl TestApp {
    pub async fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let upstream = mock_upstream().await;
        let pool = store::open_in_memory().await.unwrap();
        let mut config = Config::with_data_dir(dir.path().to_owned());
        config.game_ports = "41000-41009".parse().unwrap();
        install_fake_java(&config.runtimes_dir());

        let options = BuildOptions {
            endpoints: Some(Endpoints {
                mojang_manifest: format!("{}/mojang/manifest.json", upstream.uri()),
                paper: upstream.uri(),
                fabric: upstream.uri(),
            }),
            adoptium_api: Some(upstream.uri()),
            sandbox: Some(SandboxSupport::none()),
            probe_ports: Some(false),
        };
        let state = AppState::build_with(config, pool.clone(), options).await.unwrap();
        let router = http::router(state.clone());
        Self {
            state,
            pool,
            router,
            upstream,
            dir,
        }
    }

    pub fn with_router(mut self, router: Router) -> Self {
        self.router = router;
        self
    }

    pub async fn send(&self, request: Request<Body>) -> Response {
        self.router.clone().oneshot(request).await.unwrap()
    }

    pub async fn get(&self, path: &str, cookie: Option<&str>) -> Response {
        let mut builder = Request::get(path);
        if let Some(cookie) = cookie {
            builder = builder.header(header::COOKIE, cookie);
        }
        self.send(builder.body(Body::empty()).unwrap()).await
    }

    pub async fn post(&self, path: &str, body: Value, cookie: Option<&str>) -> Response {
        self.write("POST", path, body, cookie).await
    }

    pub async fn delete(&self, path: &str, cookie: Option<&str>) -> Response {
        self.write("DELETE", path, json!({}), cookie).await
    }

    async fn write(&self, verb: &str, path: &str, body: Value, cookie: Option<&str>) -> Response {
        let mut builder = Request::builder()
            .method(verb)
            .uri(path)
            .header(header::CONTENT_TYPE, "application/json")
            .header("x-csrf", "1");
        if let Some(cookie) = cookie {
            builder = builder.header(header::COOKIE, cookie);
        }
        self.send(builder.body(Body::from(body.to_string())).unwrap())
            .await
    }

    /// Runs first-run setup and returns the owner's session cookie (`name=value`).
    pub async fn setup_owner(&self) -> String {
        let response = self
            .post(
                "/api/v1/setup",
                json!({ "username": "owner", "password": PASSWORD }),
                None,
            )
            .await;
        assert_eq!(response.status(), StatusCode::CREATED);
        session_cookie(&response)
    }

    pub async fn create_user(&self, username: &str, role: GlobalRole) -> gsd_core::ids::UserId {
        UserRepo::new(self.pool.clone())
            .insert(&NewUser {
                username: Username::parse(username).unwrap(),
                password_hash: password::hash(PASSWORD).unwrap(),
                role,
                must_change_password: false,
            })
            .await
            .unwrap()
            .id
    }

    pub async fn login(&self, username: &str) -> String {
        let response = self
            .post(
                "/api/v1/auth/login",
                json!({ "username": username, "password": PASSWORD }),
                None,
            )
            .await;
        assert_eq!(response.status(), StatusCode::OK);
        session_cookie(&response)
    }

    /// Polls a job until it finishes; returns its final JSON.
    pub async fn wait_for_job(&self, job_id: &str, cookie: &str) -> Value {
        for _ in 0..600 {
            let job = json_body(self.get(&format!("/api/v1/jobs/{job_id}"), Some(cookie)).await).await;
            if job["status"] != "running" {
                return job;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("job {job_id} did not finish");
    }

    /// Polls an instance until its state matches.
    pub async fn wait_for_state(&self, instance_id: &str, cookie: &str, status: &str) -> Value {
        let mut last = Value::Null;
        for _ in 0..600 {
            last = json_body(
                self.get(&format!("/api/v1/instances/{instance_id}"), Some(cookie))
                    .await,
            )
            .await;
            if last["state"]["status"] == status {
                return last;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("instance never reached {status}: {last}");
    }
}

pub fn session_cookie(response: &Response) -> String {
    let set_cookie = response
        .headers()
        .get(header::SET_COOKIE)
        .expect("Set-Cookie")
        .to_str()
        .unwrap();
    set_cookie.split(';').next().unwrap().to_owned()
}

pub async fn json_body(response: Response) -> Value {
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap_or(Value::Null)
}

pub async fn text_body(response: Response) -> String {
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    String::from_utf8(bytes.to_vec()).unwrap()
}

/// A "Java 25" whose `java` is a shell script behaving like a Minecraft server.
fn install_fake_java(runtimes_dir: &Path) {
    let home = runtimes_dir.join("temurin-25-x64");
    std::fs::create_dir_all(home.join("bin")).unwrap();
    std::fs::write(
        home.join("release"),
        "JAVA_VERSION=\"25.0.1\"\nIMPLEMENTOR=\"Test\"\n",
    )
    .unwrap();
    let script = r#"#!/bin/sh
echo "[12:00:00] [Server thread/INFO]: Starting minecraft server version 26.3"
echo '[12:00:01] [Server thread/INFO]: Done (0.42s)! For help, type "help"'
while IFS= read -r line; do
  case "$line" in
    stop) echo "[12:00:09] [Server thread/INFO]: Stopping the server"; exit 0 ;;
    say\ *) echo "[12:00:05] [Server thread/INFO]: [Server] ${line#say }" ;;
    join\ *) echo "[12:00:05] [Server thread/INFO]: ${line#join } joined the game" ;;
    *) echo "[12:00:05] [Server thread/INFO]: Unknown or incomplete command" ;;
  esac
done
"#;
    let java = home.join("bin/java");
    std::fs::write(&java, script).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&java, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
}

fn fixture(name: &str) -> Value {
    let path = format!(
        "{}/../gsd-minecraft/tests/fixtures/{name}",
        env!("CARGO_MANIFEST_DIR")
    );
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn hex_sha1(bytes: &[u8]) -> String {
    use sha1::Digest;
    hex::encode(sha1::Sha1::digest(bytes))
}

/// The recorded upstream API responses, with download URLs pointing back at the mock.
pub async fn mock_upstream() -> MockServer {
    let server = MockServer::start().await;
    let mut manifest = fixture("mojang_manifest.json");
    for version in manifest["versions"].as_array_mut().unwrap() {
        let id = version["id"].as_str().unwrap().to_owned();
        version["url"] = format!("{}/mojang/versions/{id}.json", server.uri()).into();
    }
    let mut detail = fixture("mojang_version_26.3.json");
    detail["downloads"]["server"]["url"] = format!("{}/downloads/vanilla.jar", server.uri()).into();
    detail["downloads"]["server"]["sha1"] = hex_sha1(FAKE_JAR).into();

    let routes = [
        ("/mojang/manifest.json", manifest),
        ("/mojang/versions/26.3.json", detail),
        ("/mojang/versions/1.0.json", fixture("mojang_version_1.0.json")),
        ("/v3/projects/paper", fixture("paper_project.json")),
        (
            "/v3/projects/paper/versions/26.3/builds",
            fixture("paper_builds_26.3.json"),
        ),
        ("/v2/versions/game", fixture("fabric_game.json")),
        ("/v2/versions/loader/26.3", fixture("fabric_loaders_26.3.json")),
        ("/v2/versions/installer", fixture("fabric_installers.json")),
        // Adoptium: nothing available (Java 25 is pre-installed by the test).
        ("/v3/assets/latest/25/hotspot", json!([])),
    ];
    for (route, body) in routes {
        Mock::given(method("GET"))
            .and(path(route))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .mount(&server)
            .await;
    }
    Mock::given(method("GET"))
        .and(path_regex(
            r"^/(downloads/.*\.jar|v2/versions/loader/26\.3/[^/]+/[^/]+/server/jar)$",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(FAKE_JAR))
        .mount(&server)
        .await;
    server
}
