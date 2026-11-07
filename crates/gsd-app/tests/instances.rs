//! End-to-end tests of the server-management API: catalog, create/install, lifecycle,
//! console, permissions and jobs. A fake Java (shell script) plays the Minecraft server.

#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use axum::http::StatusCode;
use common::{TestApp, json_body};
use gsd_core::{
    auth::{GlobalRole, Grant},
    ids::InstanceId,
};
use serde_json::{Value, json};

fn create_body(loader: &str) -> Value {
    json!({
        "name": "Friends SMP",
        "mc_version": "26.3",
        "loader": loader,
        "max_memory_mb": 2048,
        "eula_accepted": true,
    })
}

/// Creates an instance and waits for its install job; returns the instance id.
async fn create_installed(app: &TestApp, cookie: &str, loader: &str) -> String {
    let response = app
        .post("/api/v1/instances", create_body(loader), Some(cookie))
        .await;
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    let created = json_body(response).await;
    let job = app
        .wait_for_job(created["job_id"].as_str().unwrap(), cookie)
        .await;
    assert_eq!(job["status"], "succeeded", "{job}");
    created["instance"]["id"].as_str().unwrap().to_owned()
}

#[tokio::test]
async fn catalog_endpoints_feed_the_version_picker() {
    let app = TestApp::new().await;
    let cookie = app.setup_owner().await;

    let versions = json_body(app.get("/api/v1/catalog/minecraft/versions", Some(&cookie)).await).await;
    assert_eq!(versions[0]["id"], "26.4-snapshot-7");
    assert!(
        versions
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v["id"] == "26.3" && v["kind"] == "release")
    );

    let loaders = json_body(
        app.get("/api/v1/catalog/minecraft/loaders?mc=26.3", Some(&cookie))
            .await,
    )
    .await;
    let summary: Vec<String> = loaders
        .as_array()
        .unwrap()
        .iter()
        .map(|l| {
            format!(
                "{}:{}:{}",
                l["name"].as_str().unwrap(),
                l["content"].as_str().unwrap(),
                l["available"]
            )
        })
        .collect();
    assert_eq!(
        summary,
        [
            "Vanilla:none:true",
            "Paper:plugins:true",
            "Purpur:plugins:true",
            "Fabric:mods:true",
            "Quilt:mods:true",
            "Forge:mods:true",
            "NeoForge:mods:true",
        ]
    );

    let fabric = json_body(
        app.get(
            "/api/v1/catalog/minecraft/loaders/fabric/versions?mc=26.3",
            Some(&cookie),
        )
        .await,
    )
    .await;
    assert_eq!(fabric[0]["recommended"], true);
    let forge = json_body(
        app.get(
            "/api/v1/catalog/minecraft/loaders/forge/versions?mc=26.3",
            Some(&cookie),
        )
        .await,
    )
    .await;
    let starred: Vec<&Value> = forge
        .as_array()
        .unwrap()
        .iter()
        .filter(|v| v["recommended"] == true)
        .collect();
    assert_eq!(starred.len(), 1);
    assert_eq!(starred[0]["id"], "66.1.0");
    let bad_loader = app
        .get(
            "/api/v1/catalog/minecraft/loaders/spigot/versions?mc=26.3",
            Some(&cookie),
        )
        .await;
    assert!(bad_loader.status().is_client_error());

    let unknown = app
        .get("/api/v1/catalog/minecraft/loaders?mc=9.9", Some(&cookie))
        .await;
    assert_eq!(unknown.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        app.get("/api/v1/catalog/minecraft/versions", None).await.status(),
        StatusCode::UNAUTHORIZED
    );
}

/// Polls the console snapshot until `ready` holds; returns the last snapshot.
async fn wait_for_console(app: &TestApp, id: &str, cookie: &str, ready: impl Fn(&Value) -> bool) -> Value {
    let mut snapshot = Value::Null;
    for _ in 0..100 {
        snapshot = json_body(
            app.get(&format!("/api/v1/instances/{id}/console"), Some(cookie))
                .await,
        )
        .await;
        if ready(&snapshot) {
            return snapshot;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    panic!("console never matched: {snapshot}");
}

fn has_line(snapshot: &Value, matches: impl Fn(&Value) -> bool) -> bool {
    snapshot["console"]
        .as_array()
        .is_some_and(|lines| lines.iter().any(matches))
}

async fn start_running(app: &TestApp, id: &str, cookie: &str) {
    let started = app
        .post(&format!("/api/v1/instances/{id}/start"), json!({}), Some(cookie))
        .await;
    assert_eq!(started.status(), StatusCode::ACCEPTED);
    app.wait_for_state(id, cookie, "running").await;
}

#[tokio::test]
async fn console_commands_and_players() {
    let app = TestApp::new().await;
    let cookie = app.setup_owner().await;
    let id = create_installed(&app, &cookie, "vanilla").await;
    start_running(&app, &id, &cookie).await;

    let command = |text: &str| json!({ "command": text });
    let sent = app
        .post(
            &format!("/api/v1/instances/{id}/command"),
            command("/say hello"),
            Some(&cookie),
        )
        .await;
    assert_eq!(sent.status(), StatusCode::NO_CONTENT);
    let snapshot = wait_for_console(&app, &id, &cookie, |s| {
        has_line(s, |l| l["text"] == "[Server] hello")
    })
    .await;
    assert!(has_line(&snapshot, |l| l["source"] == "input"
        && l["text"] == "[owner] > say hello"));

    // Players are tracked from the log.
    app.post(
        &format!("/api/v1/instances/{id}/command"),
        command("join Steve"),
        Some(&cookie),
    )
    .await;
    wait_for_console(&app, &id, &cookie, |s| s["players"] == json!(["Steve"])).await;

    let empty = app
        .post(
            &format!("/api/v1/instances/{id}/command"),
            command("   "),
            Some(&cookie),
        )
        .await;
    assert_eq!(json_body(empty).await["error"]["code"], "invalid_command");
    app.post(&format!("/api/v1/instances/{id}/kill"), json!({}), Some(&cookie))
        .await;
}

#[tokio::test]
async fn full_lifecycle_create_start_stop_delete() {
    let app = TestApp::new().await;
    let cookie = app.setup_owner().await;
    let id = create_installed(&app, &cookie, "vanilla").await;

    let instance = json_body(app.get(&format!("/api/v1/instances/{id}"), Some(&cookie)).await).await;
    assert_eq!(instance["installed"], true);
    assert_eq!(instance["server"]["mc_version"], "26.3");
    assert_eq!(instance["port"], 41000);
    assert_eq!(instance["state"]["status"], "stopped");

    start_running(&app, &id, &cookie).await;
    let again = app
        .post(&format!("/api/v1/instances/{id}/start"), json!({}), Some(&cookie))
        .await;
    assert_eq!(json_body(again).await["error"]["code"], "already_running");
    app.post(
        &format!("/api/v1/instances/{id}/command"),
        json!({ "command": "say hi" }),
        Some(&cookie),
    )
    .await;

    // Can't delete a running server.
    let busy = app
        .delete(&format!("/api/v1/instances/{id}"), Some(&cookie))
        .await;
    assert_eq!(json_body(busy).await["error"]["code"], "instance_running");

    assert_eq!(
        app.post(&format!("/api/v1/instances/{id}/stop"), json!({}), Some(&cookie))
            .await
            .status(),
        StatusCode::ACCEPTED
    );
    app.wait_for_state(&id, &cookie, "stopped").await;

    assert_eq!(
        app.delete(&format!("/api/v1/instances/{id}"), Some(&cookie))
            .await
            .status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        app.get(&format!("/api/v1/instances/{id}"), Some(&cookie))
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        std::fs::read_dir(app.dir.path().join("trash")).unwrap().count(),
        1,
        "moved to trash, not erased"
    );

    let audit = app.state.audit.recent(50).await.unwrap();
    for action in [
        "instance.created",
        "instance.started",
        "console.command",
        "instance.stopped",
        "instance.deleted",
    ] {
        assert!(
            audit.iter().any(|entry| entry.action == action),
            "missing audit entry {action}"
        );
    }
}

#[tokio::test]
async fn creation_is_validated() {
    let app = TestApp::new().await;
    let cookie = app.setup_owner().await;

    let cases = [
        (json!({ "eula_accepted": false }), "eula_required"),
        (json!({ "mc_version": "9.9.9" }), "unknown_version"),
        (json!({ "max_memory_mb": 10 }), "invalid_memory"),
        (json!({ "name": "" }), "invalid_name"),
        (
            json!({ "loader": "fabric", "loader_version": "nope" }),
            "unknown_loader_version",
        ),
        (
            json!({ "mc_version": "1.0", "loader": "paper" }),
            "loader_unavailable",
        ),
    ];
    for (overrides, code) in cases {
        let mut body = create_body("vanilla");
        for (key, value) in overrides.as_object().unwrap() {
            body[key] = value.clone();
        }
        let response = app.post("/api/v1/instances", body.clone(), Some(&cookie)).await;
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY, "{body}");
        assert_eq!(json_body(response).await["error"]["code"], code, "{body}");
    }
    let list = json_body(app.get("/api/v1/instances", Some(&cookie)).await).await;
    assert_eq!(list.as_array().unwrap().len(), 0, "nothing was created");
}

#[tokio::test]
async fn paper_and_fabric_install_too() {
    let app = TestApp::new().await;
    let cookie = app.setup_owner().await;
    let paper = create_installed(&app, &cookie, "paper").await;
    let fabric = create_installed(&app, &cookie, "fabric").await;

    let paper = json_body(
        app.get(&format!("/api/v1/instances/{paper}"), Some(&cookie))
            .await,
    )
    .await;
    assert_eq!(paper["server"]["loader"], "paper");
    assert!(
        paper["server"]["loader_version"]
            .as_str()
            .unwrap()
            .parse::<u32>()
            .is_ok()
    );
    let fabric = json_body(
        app.get(&format!("/api/v1/instances/{fabric}"), Some(&cookie))
            .await,
    )
    .await;
    assert_eq!(fabric["server"]["loader_version"], "0.20.1");
    assert_ne!(paper["port"], fabric["port"], "each instance gets its own port");
}

/// The whole path for an installer-based loader: create → installer job → args-files launch →
/// running server with a console → stop.
#[tokio::test]
async fn forge_servers_install_start_and_stop_through_the_api() {
    let app = TestApp::new().await;
    let cookie = app.setup_owner().await;
    let id = create_installed(&app, &cookie, "forge").await;

    let instance = json_body(app.get(&format!("/api/v1/instances/{id}"), Some(&cookie)).await).await;
    assert_eq!(instance["server"]["loader"], "forge");
    assert_eq!(instance["server"]["loader_version"], "66.1.0");

    start_running(&app, &id, &cookie).await;
    let console = wait_for_console(&app, &id, &cookie, |snapshot| {
        has_line(snapshot, |line| {
            line["text"]
                .as_str()
                .is_some_and(|t| t.starts_with("Launched with:"))
        })
    })
    .await;
    let launched = console["console"]
        .as_array()
        .unwrap()
        .iter()
        .find_map(|line| line["text"].as_str().filter(|t| t.starts_with("Launched with:")))
        .unwrap()
        .to_owned();
    assert!(
        launched.ends_with(
            "@user_jvm_args.txt @libraries/net/minecraftforge/forge/26.3-66.1.0/unix_args.txt nogui"
        ),
        "{launched}"
    );
    assert!(
        launched.contains("-Xmx2048M"),
        "our memory settings come first: {launched}"
    );

    let stopped = app
        .post(&format!("/api/v1/instances/{id}/stop"), json!({}), Some(&cookie))
        .await;
    assert_eq!(stopped.status(), StatusCode::ACCEPTED);
    app.wait_for_state(&id, &cookie, "stopped").await;
}

#[tokio::test]
async fn every_other_loader_installs_through_the_api() {
    let app = TestApp::new().await;
    let cookie = app.setup_owner().await;
    for (loader, version) in [
        ("purpur", "2657"),
        ("quilt", "0.30.2"),
        ("neoforge", "26.3.0.101"),
    ] {
        let id = create_installed(&app, &cookie, loader).await;
        let instance = json_body(app.get(&format!("/api/v1/instances/{id}"), Some(&cookie)).await).await;
        assert_eq!(instance["server"]["loader_version"], version, "{loader}");
    }
}

#[tokio::test]
async fn members_only_see_and_do_what_they_were_granted() {
    let app = TestApp::new().await;
    let owner = app.setup_owner().await;
    let visible = create_installed(&app, &owner, "vanilla").await;
    let hidden = create_installed(&app, &owner, "vanilla").await;

    let member_id = app.create_user("friend", GlobalRole::Member).await;
    app.state
        .auth
        .grants()
        .set(member_id, visible.parse::<InstanceId>().unwrap(), Grant::Viewer)
        .await
        .unwrap();
    let friend = app.login("friend").await;

    let list = json_body(app.get("/api/v1/instances", Some(&friend)).await).await;
    let ids: Vec<&str> = list
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, vec![visible.as_str()]);
    assert!(
        list[0]["permissions"]
            .as_array()
            .unwrap()
            .contains(&json!("console.read"))
    );

    assert_eq!(
        app.get(&format!("/api/v1/instances/{hidden}"), Some(&friend))
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        app.get(&format!("/api/v1/instances/{visible}/console"), Some(&friend))
            .await
            .status(),
        StatusCode::OK
    );
    for action in ["start", "stop", "kill", "install"] {
        let response = app
            .post(
                &format!("/api/v1/instances/{visible}/{action}"),
                json!({}),
                Some(&friend),
            )
            .await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN, "{action}");
    }
    let command = app
        .post(
            &format!("/api/v1/instances/{visible}/command"),
            json!({ "command": "op me" }),
            Some(&friend),
        )
        .await;
    assert_eq!(command.status(), StatusCode::FORBIDDEN);
    assert_eq!(
        app.post("/api/v1/instances", create_body("vanilla"), Some(&friend))
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        app.delete(&format!("/api/v1/instances/{visible}"), Some(&friend))
            .await
            .status(),
        StatusCode::FORBIDDEN
    );

    // Jobs of hidden instances are hidden too.
    let jobs = json_body(app.get("/api/v1/jobs", Some(&friend)).await).await;
    assert!(
        jobs.as_array()
            .unwrap()
            .iter()
            .all(|job| job["instance_id"] == visible.as_str()),
        "{jobs}"
    );
}

#[tokio::test]
async fn uninstalled_instances_cannot_start_and_failed_installs_can_be_retried() {
    let app = TestApp::new().await;
    let cookie = app.setup_owner().await;
    // Remove the fake Java so the install job fails (Adoptium mock has nothing).
    std::fs::remove_dir_all(app.state.config.runtimes_dir().join("temurin-25-x64")).unwrap();
    app.state.java.list(true).await;

    let created = json_body(
        app.post("/api/v1/instances", create_body("vanilla"), Some(&cookie))
            .await,
    )
    .await;
    let job = app
        .wait_for_job(created["job_id"].as_str().unwrap(), &cookie)
        .await;
    assert_eq!(job["status"], "failed");
    assert!(
        job["log"]
            .as_array()
            .unwrap()
            .iter()
            .any(|line| line.as_str().unwrap().contains("Java 25"))
    );

    let id = created["instance"]["id"].as_str().unwrap();
    let start = app
        .post(&format!("/api/v1/instances/{id}/start"), json!({}), Some(&cookie))
        .await;
    assert_eq!(json_body(start).await["error"]["code"], "not_installed");

    let retry = app
        .post(
            &format!("/api/v1/instances/{id}/install"),
            json!({}),
            Some(&cookie),
        )
        .await;
    assert_eq!(retry.status(), StatusCode::ACCEPTED);
}

#[tokio::test]
async fn jobs_can_be_listed_and_inspected() {
    let app = TestApp::new().await;
    let cookie = app.setup_owner().await;
    create_installed(&app, &cookie, "vanilla").await;

    let jobs = json_body(app.get("/api/v1/jobs", Some(&cookie)).await).await;
    let job = &jobs[0];
    assert_eq!(job["kind"], "instance.install");
    assert_eq!(job["status"], "succeeded");
    let detail = json_body(
        app.get(
            &format!("/api/v1/jobs/{}", job["id"].as_str().unwrap()),
            Some(&cookie),
        )
        .await,
    )
    .await;
    assert!(
        detail["log"]
            .as_array()
            .unwrap()
            .iter()
            .any(|l| l.as_str().unwrap().contains("Installation complete"))
    );

    let finished = app
        .post(
            &format!("/api/v1/jobs/{}/cancel", job["id"].as_str().unwrap()),
            json!({}),
            Some(&cookie),
        )
        .await;
    assert_eq!(json_body(finished).await["error"]["code"], "job_finished");
}

#[tokio::test]
async fn system_reports_isolation_support() {
    let app = TestApp::new().await;
    let cookie = app.setup_owner().await;
    let system = json_body(app.get("/api/v1/system", Some(&cookie)).await).await;
    assert_eq!(
        system["isolation_level"], "none",
        "tests run with the sandbox disabled"
    );
    assert!(system["sandbox_problem"].is_null());
    assert_eq!(system["game_ports"], "41000-41009");
}

/// Regression: with the data folder on WSL's Windows drive (9p), Landlock blocked the server
/// from its own jar and it crash-looped with only "Unable to access jarfile". The startup check
/// now refuses starts with an explanation instead.
#[tokio::test]
async fn servers_do_not_start_when_the_sandbox_cannot_work_in_the_data_folder() {
    const PROBLEM: &str = "Servers can't start: move the data folder to a Linux filesystem.";
    let app = TestApp::with_sandbox_problem(PROBLEM).await;
    let cookie = app.setup_owner().await;
    // Installing doesn't run sandboxed code, so it still works.
    let id = create_installed(&app, &cookie, "vanilla").await;

    for action in ["start", "restart"] {
        let response = app
            .post(
                &format!("/api/v1/instances/{id}/{action}"),
                json!({}),
                Some(&cookie),
            )
            .await;
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE, "{action}");
        let error = &json_body(response).await["error"];
        assert_eq!(error["code"], "sandbox_unusable");
        assert_eq!(error["message"], PROBLEM);
    }

    let instance = json_body(app.get(&format!("/api/v1/instances/{id}"), Some(&cookie)).await).await;
    assert_eq!(instance["state"]["status"], "stopped", "{instance}");
    let system = json_body(app.get("/api/v1/system", Some(&cookie)).await).await;
    assert_eq!(system["sandbox_problem"], PROBLEM);
}
