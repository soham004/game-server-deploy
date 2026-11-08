//! Mods and plugins API: what fits a server, search, planning with dependencies, installing
//! (files, packwiz index, ownership), replacing, turning off and on, removing, jars added by
//! hand (and identifying them), updating after a backup, and who may do what. Modrinth is `gsd_mods::test_support::mock_modrinth`: recorded
//! responses with generated jars.

#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::path::PathBuf;

use axum::http::StatusCode;
use common::{TestApp, json_body};
use gsd_core::{
    auth::{GlobalRole, Grant},
    ids::InstanceId,
};
use serde_json::{Value, json};

const APPLESKIN: &str = "EsAfCjCV";
const ARCHITECTURY: &str = "lhGA9TYQ";
const LITHIUM: &str = "gvQqBUqZ";
const FABRIC_API: &str = "P7dR8mSH";
const CHUNKY: &str = "fALzjamp";

async fn server(app: &TestApp, loader: &str, cookie: &str) -> String {
    let body = json!({ "name": format!("{loader} server"), "mc_version": "26.3", "loader": loader, "max_memory_mb": 1024, "eula_accepted": true });
    let created = json_body(app.post("/api/v1/instances", body, Some(cookie)).await).await;
    let job = app
        .wait_for_job(created["job_id"].as_str().unwrap(), cookie)
        .await;
    assert_eq!(job["status"], "succeeded", "{job}");
    created["instance"]["id"].as_str().unwrap().to_owned()
}

fn url(id: &str, rest: &str) -> String {
    format!("/api/v1/instances/{id}/content{rest}")
}

async fn content(app: &TestApp, id: &str, cookie: &str) -> Value {
    json_body(app.get(&url(id, ""), Some(cookie)).await).await
}

/// Installs and waits; returns the started response (plan included).
async fn install(app: &TestApp, id: &str, body: Value, cookie: &str) -> Value {
    let response = app.post(&url(id, "/install"), body, Some(cookie)).await;
    let status = response.status();
    let started = json_body(response).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{started}");
    let job = app
        .wait_for_job(started["job_id"].as_str().unwrap(), cookie)
        .await;
    assert_eq!(job["status"], "succeeded", "{job}");
    started
}

async fn server_dir(app: &TestApp, id: &str) -> PathBuf {
    let id: InstanceId = id.parse().unwrap();
    app.state.instances.get(id).await.unwrap().layout.server_dir()
}

fn names(dir: &std::path::Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

fn item<'a>(list: &'a Value, name: &str) -> &'a Value {
    list["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["name"] == name)
        .unwrap_or_else(|| panic!("{name} in {list}"))
}

#[tokio::test]
async fn vanilla_runs_no_content_and_modded_servers_get_mods() {
    let app = TestApp::new().await;
    let cookie = app.setup_owner().await;
    let vanilla = server(&app, "vanilla", &cookie).await;
    let response = app.get(&url(&vanilla, ""), Some(&cookie)).await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(
        json_body(response).await["error"]["code"],
        "no_content_for_loader"
    );

    let fabric = server(&app, "fabric", &cookie).await;
    let list = content(&app, &fabric, &cookie).await;
    assert_eq!(
        (list["kind"].as_str(), list["folder"].as_str()),
        (Some("mods"), Some("mods"))
    );
    assert_eq!(list["target"]["loaders"], json!(["fabric"]));
    assert_eq!(list["items"], json!([]));
}

#[tokio::test]
async fn search_and_versions_fit_the_server() {
    let app = TestApp::new().await;
    let cookie = app.setup_owner().await;
    let id = server(&app, "fabric", &cookie).await;

    let page = json_body(
        app.get(&url(&id, "/search?q=&sort=downloads"), Some(&cookie))
            .await,
    )
    .await;
    assert!(!page["hits"].as_array().unwrap().is_empty(), "{page}");
    assert_eq!(page["hits"][0]["source"], "modrinth");

    let versions = json_body(
        app.get(
            &url(&id, &format!("/projects/{APPLESKIN}/versions")),
            Some(&cookie),
        )
        .await,
    )
    .await;
    let newest = &versions[0];
    assert!(
        newest["game_versions"]
            .as_array()
            .unwrap()
            .contains(&json!("26.3"))
    );
    assert_eq!(newest["dependencies"][0]["kind"], "required");

    let project = json_body(app.get(&url(&id, "/projects/appleskin"), Some(&cookie)).await).await;
    assert_eq!(project["title"], "AppleSkin");
    let missing = app.get(&url(&id, "/projects/nope"), Some(&cookie)).await;
    assert_eq!(json_body(missing).await["error"]["code"], "project_not_found");
}

#[tokio::test]
async fn installing_brings_required_dependencies_and_writes_the_packwiz_index() {
    let app = TestApp::new().await;
    let cookie = app.setup_owner().await;
    let id = server(&app, "fabric", &cookie).await;

    let plan = json_body(
        app.post(
            &url(&id, "/plan"),
            json!({ "project_id": APPLESKIN }),
            Some(&cookie),
        )
        .await,
    )
    .await;
    let steps: Vec<(&str, Option<&str>)> = plan["steps"]
        .as_array()
        .unwrap()
        .iter()
        .map(|step| {
            (
                step["project"]["title"].as_str().unwrap(),
                step["required_by"].as_str(),
            )
        })
        .collect();
    assert_eq!(steps, [("AppleSkin", None), ("Fabric API", Some("AppleSkin"))]);

    install(&app, &id, json!({ "project_id": APPLESKIN }), &cookie).await;

    let list = content(&app, &id, &cookie).await;
    assert_eq!(list["items"].as_array().unwrap().len(), 2);
    let appleskin = item(&list, "AppleSkin");
    assert_eq!(appleskin["enabled"], true);
    assert_eq!(appleskin["source"]["project_id"], APPLESKIN);
    assert_eq!(appleskin["mod_id"], "appleskin", "read from the jar");
    assert!(
        appleskin["version"].as_str().unwrap().contains("26.3"),
        "{appleskin}"
    );
    assert_eq!(appleskin["sha1"].as_str().unwrap().len(), 40);

    let mods = server_dir(&app, &id).await.join("mods");
    let jars: Vec<String> = names(&mods)
        .into_iter()
        .filter(|name| {
            std::path::Path::new(name)
                .extension()
                .is_some_and(|ext| ext == "jar")
        })
        .collect();
    assert_eq!(jars.len(), 2, "{jars:?}");
    assert_eq!(
        names(&mods.join(".index")),
        ["appleskin.pw.toml", "fabric-api.pw.toml"]
    );
    let index = std::fs::read_to_string(mods.join(".index/appleskin.pw.toml")).unwrap();
    assert!(
        index.contains("name = \"AppleSkin\"") && index.contains("hash-format = \"sha512\""),
        "{index}"
    );
    assert!(index.contains(&format!("mod-id = \"{APPLESKIN}\"")), "{index}");

    // Another mod that needs Fabric API: it is already there, so only the mod comes.
    let started = install(&app, &id, json!({ "project_id": ARCHITECTURY }), &cookie).await;
    assert_eq!(started["plan"]["steps"].as_array().unwrap().len(), 1);
    assert_eq!(started["plan"]["already_installed"], json!(["Fabric API"]));
    assert_eq!(
        content(&app, &id, &cookie).await["items"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
}

#[tokio::test]
async fn installing_another_version_replaces_the_old_file() {
    let app = TestApp::new().await;
    let cookie = app.setup_owner().await;
    let id = server(&app, "fabric", &cookie).await;
    let versions = json_body(
        app.get(&url(&id, &format!("/projects/{LITHIUM}/versions")), Some(&cookie))
            .await,
    )
    .await;
    let (newest, older) = (
        versions[0]["id"].as_str().unwrap(),
        versions[1]["id"].as_str().unwrap(),
    );

    install(
        &app,
        &id,
        json!({ "project_id": LITHIUM, "version_id": older }),
        &cookie,
    )
    .await;
    assert_eq!(
        item(&content(&app, &id, &cookie).await, "Lithium")["source"]["version_id"],
        older
    );
    let started = install(
        &app,
        &id,
        json!({ "project_id": LITHIUM, "version_id": newest }),
        &cookie,
    )
    .await;
    assert!(started["plan"]["steps"][0]["replaces"].is_string());

    let list = content(&app, &id, &cookie).await;
    assert_eq!(list["items"].as_array().unwrap().len(), 1, "{list}");
    assert_eq!(item(&list, "Lithium")["source"]["version_id"], newest);
}

#[tokio::test]
async fn files_turn_off_and_on_and_removal_takes_the_index_entry_too() {
    let app = TestApp::new().await;
    let cookie = app.setup_owner().await;
    let id = server(&app, "fabric", &cookie).await;
    install(&app, &id, json!({ "project_id": LITHIUM }), &cookie).await;
    let file = item(&content(&app, &id, &cookie).await, "Lithium")["file"]
        .as_str()
        .unwrap()
        .to_owned();
    let mods = server_dir(&app, &id).await.join("mods");
    let encoded = file.replace('+', "%2B");

    let response = app
        .post(
            &url(&id, &format!("/files/{encoded}/disable")),
            json!({}),
            Some(&cookie),
        )
        .await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert!(mods.join(format!("{file}.disabled")).is_file());
    assert_eq!(
        item(&content(&app, &id, &cookie).await, "Lithium")["enabled"],
        false
    );
    app.post(
        &url(&id, &format!("/files/{encoded}/enable")),
        json!({}),
        Some(&cookie),
    )
    .await;
    assert!(mods.join(&file).is_file());

    let response = app
        .delete(&url(&id, &format!("/files/{encoded}")), Some(&cookie))
        .await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert_eq!(content(&app, &id, &cookie).await["items"], json!([]));
    assert!(!mods.join(&file).exists());
    assert_eq!(
        names(&mods.join(".index")),
        Vec::<String>::new(),
        "the index entry went too"
    );
    let missing = app.delete(&url(&id, "/files/nothing.jar"), Some(&cookie)).await;
    assert_eq!(json_body(missing).await["error"]["code"], "content_not_installed");
}

#[tokio::test]
async fn jars_added_by_hand_are_listed_from_their_own_metadata() {
    let app = TestApp::new().await;
    let cookie = app.setup_owner().await;
    let id = server(&app, "fabric", &cookie).await;
    let mods = server_dir(&app, &id).await.join("mods");
    std::fs::create_dir_all(&mods).unwrap();
    std::fs::write(
        mods.join("handmade.jar"),
        gsd_mods::test_support::fake_jar("handmade", "Handmade Mod", "1.2.3", false),
    )
    .unwrap();
    std::fs::write(mods.join("broken.jar"), "not a zip").unwrap();
    std::fs::write(mods.join("notes.txt"), "not a mod").unwrap();

    let list = content(&app, &id, &cookie).await;
    let items = list["items"].as_array().unwrap();
    assert_eq!(items.len(), 2, "{list}");
    let handmade = item(&list, "Handmade Mod");
    assert_eq!(
        (handmade["version"].as_str(), handmade["source"].clone()),
        (Some("1.2.3"), Value::Null)
    );
    assert_eq!(
        item(&list, "broken.jar")["version"],
        Value::Null,
        "unreadable jars are still listed"
    );
}

#[tokio::test]
async fn plugin_servers_get_plugins_in_the_plugins_folder() {
    let app = TestApp::new().await;
    let cookie = app.setup_owner().await;
    let id = server(&app, "paper", &cookie).await;
    let list = content(&app, &id, &cookie).await;
    assert_eq!(
        (list["kind"].as_str(), list["folder"].as_str()),
        (Some("plugins"), Some("plugins"))
    );

    install(&app, &id, json!({ "project_id": CHUNKY }), &cookie).await;

    let chunky = item(&content(&app, &id, &cookie).await, "Chunky").clone();
    assert_eq!(chunky["mod_id"], "Chunky", "plugin.yml");
    let plugins = server_dir(&app, &id).await.join("plugins");
    assert!(
        names(&plugins)
            .iter()
            .any(|name| name == chunky["file"].as_str().unwrap())
    );
    assert_eq!(names(&plugins.join(".index")), ["chunky.pw.toml"]);
}

#[tokio::test]
async fn viewers_browse_but_only_content_managers_change_anything() {
    let app = TestApp::new().await;
    let owner = app.setup_owner().await;
    let id = server(&app, "fabric", &owner).await;
    install(&app, &id, json!({ "project_id": LITHIUM }), &owner).await;
    let file = item(&content(&app, &id, &owner).await, "Lithium")["file"]
        .as_str()
        .unwrap()
        .replace('+', "%2B");
    let viewer = app.create_user("viewer", GlobalRole::Member).await;
    app.state
        .auth
        .grants()
        .set(viewer, id.parse().unwrap(), Grant::Viewer)
        .await
        .unwrap();
    let viewer = app.login("viewer").await;

    assert_eq!(
        app.get(&url(&id, ""), Some(&viewer)).await.status(),
        StatusCode::OK
    );
    assert_eq!(
        app.get(&url(&id, "/search"), Some(&viewer)).await.status(),
        StatusCode::OK
    );
    let forbidden = [
        app.post(
            &url(&id, "/plan"),
            json!({ "project_id": APPLESKIN }),
            Some(&viewer),
        )
        .await,
        app.post(
            &url(&id, "/install"),
            json!({ "project_id": APPLESKIN }),
            Some(&viewer),
        )
        .await,
        app.post(
            &url(&id, &format!("/files/{file}/disable")),
            json!({}),
            Some(&viewer),
        )
        .await,
        app.delete(&url(&id, &format!("/files/{file}")), Some(&viewer))
            .await,
    ];
    for response in forbidden {
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }
}

/// The versions of `project` that fit the server, newest first.
async fn versions(app: &TestApp, id: &str, project: &str, cookie: &str) -> Vec<Value> {
    let versions = app
        .get(&url(id, &format!("/projects/{project}/versions")), Some(cookie))
        .await;
    json_body(versions).await.as_array().unwrap().clone()
}

async fn updates(app: &TestApp, id: &str, cookie: &str) -> Value {
    json_body(app.get(&url(id, "/updates"), Some(cookie)).await).await
}

/// Starts an update and waits; returns the started response.
async fn update(app: &TestApp, id: &str, body: Value, cookie: &str) -> Value {
    let response = app.post(&url(id, "/update"), body, Some(cookie)).await;
    let status = response.status();
    let started = json_body(response).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{started}");
    let job = app
        .wait_for_job(started["job_id"].as_str().unwrap(), cookie)
        .await;
    assert_eq!(job["status"], "succeeded", "{job}");
    started
}

/// The listed item for `name` (a file name).
fn file<'a>(list: &'a Value, name: &str) -> &'a Value {
    list["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["file"] == name)
        .unwrap_or_else(|| panic!("{name} in {list}"))
}

/// `+` in a file name, as a path segment.
fn segment(file: &str) -> String {
    file.replace('+', "%2B")
}

#[tokio::test]
async fn updates_replace_old_versions_after_a_full_backup() {
    let app = TestApp::new().await;
    let cookie = app.setup_owner().await;
    let id = server(&app, "fabric", &cookie).await;
    let lithium = versions(&app, &id, LITHIUM, &cookie).await;
    let (newest, older) = (&lithium[0], &lithium[2]);
    install(
        &app,
        &id,
        json!({ "project_id": LITHIUM, "version_id": older["id"] }),
        &cookie,
    )
    .await;
    let old_file = older["file"]["filename"].as_str().unwrap();

    let check = updates(&app, &id, &cookie).await;
    let available = check["updates"].as_array().unwrap();
    assert_eq!(available.len(), 1, "{check}");
    assert_eq!(
        (
            available[0]["file"].as_str(),
            available[0]["name"].as_str(),
            available[0]["installed_version"].as_str(),
            available[0]["version"]["id"].as_str(),
        ),
        (
            Some(old_file),
            Some("Lithium"),
            older["version_number"].as_str(),
            newest["id"].as_str()
        )
    );
    assert_eq!(check["unknown"], json!([]));

    let started = update(&app, &id, json!({}), &cookie).await;
    assert_eq!(started["updates"].as_array().unwrap().len(), 1);

    let list = content(&app, &id, &cookie).await;
    assert_eq!(list["items"].as_array().unwrap().len(), 1, "{list}");
    let lithium = item(&list, "Lithium");
    assert_eq!(lithium["source"]["version_id"], newest["id"]);
    assert_eq!(lithium["file"], newest["file"]["filename"]);
    let mods = server_dir(&app, &id).await.join("mods");
    assert!(!mods.join(old_file).exists(), "the old file is gone");

    let backups = json_body(
        app.get(&format!("/api/v1/instances/{id}/backups"), Some(&cookie))
            .await,
    )
    .await;
    let kinds: Vec<(&str, &str)> = backups["backups"]
        .as_array()
        .unwrap()
        .iter()
        .map(|backup| {
            (
                backup["kind"].as_str().unwrap(),
                backup["scope"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(kinds, [("pre_update", "full")], "a full backup came first");

    assert_eq!(updates(&app, &id, &cookie).await["updates"], json!([]));
    let again = app.post(&url(&id, "/update"), json!({}), Some(&cookie)).await;
    assert_eq!(again.status(), StatusCode::CONFLICT);
    assert_eq!(json_body(again).await["error"]["code"], "up_to_date");
}

#[tokio::test]
async fn only_chosen_and_turned_on_files_are_updated() {
    let app = TestApp::new().await;
    let cookie = app.setup_owner().await;
    let id = server(&app, "fabric", &cookie).await;
    let lithium = versions(&app, &id, LITHIUM, &cookie).await;
    let fabric_api = versions(&app, &id, FABRIC_API, &cookie).await;
    for (project, version) in [(LITHIUM, &lithium[1]), (FABRIC_API, &fabric_api[1])] {
        install(
            &app,
            &id,
            json!({ "project_id": project, "version_id": version["id"] }),
            &cookie,
        )
        .await;
    }
    let check = updates(&app, &id, &cookie).await;
    assert_eq!(check["updates"].as_array().unwrap().len(), 2, "{check}");

    // Turned off: not offered (updating would turn it back on).
    let fabric_api_file = fabric_api[1]["file"]["filename"].as_str().unwrap();
    let toggle = |action: &'static str| url(&id, &format!("/files/{}/{action}", segment(fabric_api_file)));
    app.post(&toggle("disable"), json!({}), Some(&cookie)).await;
    let check = updates(&app, &id, &cookie).await;
    let offered: Vec<&str> = check["updates"]
        .as_array()
        .unwrap()
        .iter()
        .map(|update| update["name"].as_str().unwrap())
        .collect();
    assert_eq!(offered, ["Lithium"]);
    app.post(&toggle("enable"), json!({}), Some(&cookie)).await;

    // Only the chosen file.
    let started = update(&app, &id, json!({ "files": [fabric_api_file] }), &cookie).await;
    assert_eq!(started["updates"].as_array().unwrap().len(), 1);
    let list = content(&app, &id, &cookie).await;
    assert_eq!(
        item(&list, "Fabric API")["source"]["version_id"],
        fabric_api[0]["id"]
    );
    assert_eq!(item(&list, "Lithium")["source"]["version_id"], lithium[1]["id"]);

    let unknown = app
        .post(
            &url(&id, "/update"),
            json!({ "files": ["nothing.jar"] }),
            Some(&cookie),
        )
        .await;
    assert_eq!(json_body(unknown).await["error"]["code"], "up_to_date");
}

#[tokio::test]
async fn an_update_brings_back_a_missing_required_dependency() {
    let app = TestApp::new().await;
    let cookie = app.setup_owner().await;
    let id = server(&app, "fabric", &cookie).await;
    let architectury = versions(&app, &id, ARCHITECTURY, &cookie).await;
    install(
        &app,
        &id,
        json!({ "project_id": ARCHITECTURY, "version_id": architectury[2]["id"] }),
        &cookie,
    )
    .await;
    let fabric_api = segment(
        item(&content(&app, &id, &cookie).await, "Fabric API")["file"]
            .as_str()
            .unwrap(),
    );
    app.delete(&url(&id, &format!("/files/{fabric_api}")), Some(&cookie))
        .await;
    let list = content(&app, &id, &cookie).await;
    assert_eq!(list["items"].as_array().unwrap().len(), 1, "{list}");

    update(&app, &id, json!({}), &cookie).await;

    let list = content(&app, &id, &cookie).await;
    assert_eq!(
        item(&list, "Architectury API")["source"]["version_id"],
        architectury[0]["id"]
    );
    assert_eq!(item(&list, "Fabric API")["enabled"], true, "{list}");
}

#[tokio::test]
async fn hand_added_jars_with_known_hashes_are_identified_then_updated() {
    let app = TestApp::new().await;
    let cookie = app.setup_owner().await;
    let id = server(&app, "fabric", &cookie).await;
    let lithium = versions(&app, &id, LITHIUM, &cookie).await;
    let mods = server_dir(&app, &id).await.join("mods");
    std::fs::create_dir_all(&mods).unwrap();
    for (file, version) in [
        ("lithium-from-a-friend.jar", &lithium[1]),
        ("old-lithium.jar", &lithium[2]),
    ] {
        let jar = reqwest::get(version["file"]["url"].as_str().unwrap())
            .await
            .unwrap()
            .bytes()
            .await
            .unwrap();
        std::fs::write(mods.join(file), &jar).unwrap();
    }
    std::fs::write(
        mods.join("handmade.jar"),
        gsd_mods::test_support::fake_jar("handmade", "Handmade Mod", "1.2.3", false),
    )
    .unwrap();

    let check = updates(&app, &id, &cookie).await;
    assert_eq!(check["updates"], json!([]), "unknown files have no source");
    let mut unknown: Vec<&str> = check["unknown"]
        .as_array()
        .unwrap()
        .iter()
        .map(|file| file.as_str().unwrap())
        .collect();
    unknown.sort_unstable();
    assert_eq!(
        unknown,
        ["handmade.jar", "lithium-from-a-friend.jar", "old-lithium.jar"]
    );

    let identified = json_body(app.post(&url(&id, "/identify"), json!({}), Some(&cookie)).await).await;
    assert_eq!(identified["identified"], json!(["Lithium"]));
    assert_eq!(identified["still_unknown"], json!(["handmade.jar"]));
    assert_eq!(
        identified["duplicates"],
        json!(["old-lithium.jar"]),
        "a second copy of a project is left alone"
    );

    let list = content(&app, &id, &cookie).await;
    let found = file(&list, "lithium-from-a-friend.jar");
    assert_eq!(
        found["file"], "lithium-from-a-friend.jar",
        "the file keeps its name"
    );
    assert_eq!(found["source"]["version_id"], lithium[1]["id"]);
    let index = std::fs::read_to_string(mods.join(".index/lithium.pw.toml")).unwrap();
    assert!(
        index.contains("filename = \"lithium-from-a-friend.jar\""),
        "{index}"
    );

    // Tracked now, so it can be updated.
    update(&app, &id, json!({}), &cookie).await;
    assert!(!mods.join("lithium-from-a-friend.jar").exists());
    let list = content(&app, &id, &cookie).await;
    assert_eq!(
        file(&list, lithium[0]["file"]["filename"].as_str().unwrap())["source"]["version_id"],
        lithium[0]["id"]
    );
    assert_eq!(file(&list, "old-lithium.jar")["source"], Value::Null);
    assert_eq!(item(&list, "Handmade Mod")["source"], Value::Null);
}

#[tokio::test]
async fn viewers_see_updates_but_cannot_identify_or_update() {
    let app = TestApp::new().await;
    let owner = app.setup_owner().await;
    let id = server(&app, "fabric", &owner).await;
    let lithium = versions(&app, &id, LITHIUM, &owner).await;
    install(
        &app,
        &id,
        json!({ "project_id": LITHIUM, "version_id": lithium[1]["id"] }),
        &owner,
    )
    .await;
    let viewer = app.create_user("viewer", GlobalRole::Member).await;
    app.state
        .auth
        .grants()
        .set(viewer, id.parse().unwrap(), Grant::Viewer)
        .await
        .unwrap();
    let viewer = app.login("viewer").await;

    let check = updates(&app, &id, &viewer).await;
    assert_eq!(check["updates"].as_array().unwrap().len(), 1, "{check}");
    for path in ["/identify", "/update"] {
        let response = app.post(&url(&id, path), json!({}), Some(&viewer)).await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN, "{path}");
    }
}
