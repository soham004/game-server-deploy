//! Shared helpers for the API integration tests: an app on a temporary data directory, a mock
//! of the upstream version APIs, a fake Java that behaves like a Minecraft server, and HTTP
//! helpers.

#![allow(clippy::unwrap_used, clippy::expect_used, dead_code)]

use std::time::Duration;

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
    supervisor::{HostSandbox, SandboxSupport},
};
use gsd_minecraft::{Endpoints, test_support};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{method, path},
};

pub const PASSWORD: &str = "correct horse battery";

pub struct TestApp {
    pub state: AppState,
    pub pool: SqlitePool,
    pub router: Router,
    pub upstream: MockServer,
    /// Modrinth, from recorded responses (`gsd_mods::test_support`).
    pub modrinth: MockServer,
    pub dir: tempfile::TempDir,
}

impl TestApp {
    pub async fn new() -> Self {
        Self::build(None).await
    }

    /// An app whose startup check found that the sandbox can't work in its data folder.
    pub async fn with_sandbox_problem(problem: &str) -> Self {
        Self::build(Some(problem.to_owned())).await
    }

    async fn build(sandbox_problem: Option<String>) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let upstream = mock_upstream().await;
        let modrinth = gsd_mods::test_support::mock_modrinth().await;
        let pool = store::open_in_memory().await.unwrap();
        let mut config = Config::with_data_dir(dir.path().to_owned());
        config.game_ports = "41000-41009".parse().unwrap();
        test_support::install_fake_java(&config.runtimes_dir().join("temurin-25-x64"));

        let options = BuildOptions {
            endpoints: Some(Endpoints::all_at(&upstream.uri())),
            adoptium_api: Some(upstream.uri()),
            sandbox: Some(HostSandbox::new(SandboxSupport::none(), sandbox_problem)),
            probe_ports: Some(false),
            backup_save_timeout: Some(Duration::from_secs(2)),
            modrinth_api: Some(modrinth.uri()),
        };
        let state = AppState::build_with(config, pool.clone(), options).await.unwrap();
        // As `serve` does; it stops with the app's shutdown token.
        state.scheduler.start(state.shutdown.clone());
        let router = http::router(state.clone());
        Self {
            state,
            pool,
            router,
            upstream,
            modrinth,
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

    pub async fn put(&self, path: &str, body: Value, cookie: Option<&str>) -> Response {
        self.write("PUT", path, body, cookie).await
    }

    pub async fn patch(&self, path: &str, body: Value, cookie: Option<&str>) -> Response {
        self.write("PATCH", path, body, cookie).await
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

    /// Creates a vanilla 26.3 server, waits for its install and returns its id.
    pub async fn installed_instance(&self, name: &str, cookie: &str) -> String {
        let body = json!({ "name": name, "mc_version": "26.3", "loader": "vanilla", "max_memory_mb": 1024, "eula_accepted": true });
        let created = json_body(self.post("/api/v1/instances", body, Some(cookie)).await).await;
        let job = self
            .wait_for_job(created["job_id"].as_str().expect("job id"), cookie)
            .await;
        assert_eq!(job["status"], "succeeded", "{job}");
        created["instance"]["id"]
            .as_str()
            .expect("instance id")
            .to_owned()
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

/// The recorded upstream APIs of every loader (shared with gsd-minecraft's own tests), plus an
/// Adoptium with nothing to offer (Java 25 is pre-installed by the test).
async fn mock_upstream() -> MockServer {
    let server = test_support::mock_upstream().await;
    Mock::given(method("GET"))
        .and(path("/v3/assets/latest/25/hotspot"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .mount(&server)
        .await;
    server
}
