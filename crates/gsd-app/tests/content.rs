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
use gsd_mods::test_support::{CF_BLOCKED, CF_LIBRARY, CF_MOD, CF_MOD_FILES, CF_PLUGIN, CURSEFORGE_KEY};
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

#[tokio::test]
async fn plugin_servers_can_search_hangar_too() {
    let app = TestApp::new().await;
    let cookie = app.setup_owner().await;
    let paper = server(&app, "paper", &cookie).await;
    assert_eq!(
        content(&app, &paper, &cookie).await["sources"],
        json!(["modrinth", "hangar"])
    );
    let fabric = server(&app, "fabric", &cookie).await;
    assert_eq!(
        content(&app, &fabric, &cookie).await["sources"],
        json!(["modrinth"])
    );

    let page = json_body(
        app.get(&url(&paper, "/search?source=hangar&q=chunky"), Some(&cookie))
            .await,
    )
    .await;
    let first = &page["hits"][0];
    assert_eq!(first["source"], "hangar", "{page}");
    assert!(
        first["page_url"]
            .as_str()
            .unwrap()
            .starts_with("https://hangar.papermc.io/"),
        "{first}"
    );
    let modrinth = json_body(app.get(&url(&paper, "/search"), Some(&cookie)).await).await;
    assert_eq!(modrinth["hits"][0]["source"], "modrinth", "the default");

    let project = json_body(
        app.get(&url(&paper, "/projects/Chunky?source=hangar"), Some(&cookie))
            .await,
    )
    .await;
    assert_eq!(project["source"], "hangar");
    let versions = json_body(
        app.get(
            &url(&paper, "/projects/Chunky/versions?source=hangar"),
            Some(&cookie),
        )
        .await,
    )
    .await;
    assert!(versions[0]["file"]["sha256"].is_string(), "{versions}");

    // A name on Hangar means nothing on Modrinth, and a mod server gets nothing from Hangar.
    let elsewhere = app.get(&url(&paper, "/projects/ViaVersion"), Some(&cookie)).await;
    assert_eq!(json_body(elsewhere).await["error"]["code"], "project_not_found");
    let nothing = json_body(
        app.get(&url(&fabric, "/search?source=hangar"), Some(&cookie))
            .await,
    )
    .await;
    assert_eq!(nothing["hits"], json!([]));
    let unknown = app
        .get(&url(&paper, "/search?source=spigot"), Some(&cookie))
        .await;
    assert_eq!(unknown.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn hangar_plugins_install_with_dependencies_from_hangar() {
    let app = TestApp::new().await;
    let cookie = app.setup_owner().await;
    let id = server(&app, "paper", &cookie).await;

    let started = install(
        &app,
        &id,
        json!({ "source": "hangar", "project_id": "ViaBackwards" }),
        &cookie,
    )
    .await;
    let steps: Vec<(&str, &str, Option<&str>)> = started["plan"]["steps"]
        .as_array()
        .unwrap()
        .iter()
        .map(|step| {
            (
                step["project"]["title"].as_str().unwrap(),
                step["version"]["channel"].as_str().unwrap(),
                step["required_by"].as_str(),
            )
        })
        .collect();
    assert_eq!(
        steps,
        [
            ("ViaBackwards", "release", None),
            ("ViaVersion", "release", Some("ViaBackwards"))
        ],
        "releases, though newer snapshots exist; the optional ViaRewind stays out"
    );

    let list = content(&app, &id, &cookie).await;
    let via = item(&list, "ViaVersion");
    assert_eq!(
        (
            via["source"]["source"].as_str(),
            via["source"]["project_id"].as_str()
        ),
        (Some("hangar"), Some("31"))
    );
    assert!(
        via["source"]["page_url"]
            .as_str()
            .unwrap()
            .ends_with("/ViaVersion"),
        "{via}"
    );
    assert_eq!(via["mod_id"], "ViaVersion", "plugin.yml");

    let plugins = server_dir(&app, &id).await.join("plugins");
    let index = std::fs::read_to_string(plugins.join(".index/viaversion.pw.toml")).unwrap();
    assert!(
        index.contains("hash-format = \"sha256\"")
            && index.contains("[update.hangar]")
            && index.contains("project-id = \"31\""),
        "{index}"
    );

    // Installed once: asking again finds it, by project, on its source.
    let again = json_body(
        app.post(
            &url(&id, "/plan"),
            json!({ "source": "hangar", "project_id": "ViaBackwards" }),
            Some(&cookie),
        )
        .await,
    )
    .await;
    assert!(again["steps"][0]["replaces"].is_string(), "{again}");
    assert_eq!(again["already_installed"], json!(["ViaVersion"]));
}

#[tokio::test]
async fn hangar_plugins_update_after_a_backup_one_project_at_a_time() {
    let app = TestApp::new().await;
    let cookie = app.setup_owner().await;
    let id = server(&app, "paper", &cookie).await;
    let versions = json_body(
        app.get(
            &url(&id, "/projects/Chunky/versions?source=hangar"),
            Some(&cookie),
        )
        .await,
    )
    .await;
    let (newest, older) = (&versions[0], &versions[1]);
    install(
        &app,
        &id,
        json!({ "source": "hangar", "project_id": "Chunky", "version_id": older["id"] }),
        &cookie,
    )
    .await;

    let check = updates(&app, &id, &cookie).await;
    assert_eq!(check["updates"].as_array().unwrap().len(), 1, "{check}");
    let update_found = &check["updates"][0];
    assert_eq!(update_found["source"], "hangar");
    assert_eq!(update_found["installed_version"], older["version_number"]);
    assert_eq!(update_found["version"]["id"], newest["id"]);

    let started = update(&app, &id, json!({}), &cookie).await;
    assert_eq!(started["updates"].as_array().unwrap().len(), 1);
    let list = content(&app, &id, &cookie).await;
    assert_eq!(list["items"].as_array().unwrap().len(), 1, "{list}");
    assert_eq!(item(&list, "Chunky")["source"]["version_id"], newest["id"]);
    assert_eq!(updates(&app, &id, &cookie).await["updates"], json!([]));
    let backups = json_body(
        app.get(&format!("/api/v1/instances/{id}/backups"), Some(&cookie))
            .await,
    )
    .await;
    assert_eq!(backups["backups"][0]["kind"], "pre_update");
}

#[tokio::test]
async fn a_hangar_project_that_is_gone_does_not_hide_other_updates() {
    let app = TestApp::new().await;
    let cookie = app.setup_owner().await;
    let id = server(&app, "paper", &cookie).await;
    let versions = json_body(
        app.get(
            &url(&id, "/projects/Chunky/versions?source=hangar"),
            Some(&cookie),
        )
        .await,
    )
    .await;
    install(
        &app,
        &id,
        json!({ "source": "hangar", "project_id": "Chunky", "version_id": versions[1]["id"] }),
        &cookie,
    )
    .await;
    let plugins = server_dir(&app, &id).await.join("plugins");
    std::fs::write(
        plugins.join("gone.jar"),
        gsd_mods::test_support::fake_jar("gone", "Gone", "1", true),
    )
    .unwrap();
    std::fs::write(
        plugins.join(".index/gone.pw.toml"),
        "name = \"Gone\"\nfilename = \"gone.jar\"\n[download]\nhash-format = \"sha256\"\nhash = \"a\"\n\
         [update.hangar]\nproject-id = \"999999\"\nversion-id = \"1\"\n",
    )
    .unwrap();

    let check = updates(&app, &id, &cookie).await;
    let names: Vec<&str> = check["updates"]
        .as_array()
        .unwrap_or_else(|| panic!("{check}"))
        .iter()
        .map(|update| update["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["Chunky"]);
}

#[tokio::test]
async fn one_plugin_from_two_sources_stays_one_file() {
    let app = TestApp::new().await;
    let cookie = app.setup_owner().await;
    let id = server(&app, "paper", &cookie).await;
    install(&app, &id, json!({ "project_id": CHUNKY }), &cookie).await;
    let from_modrinth = item(&content(&app, &id, &cookie).await, "Chunky")["file"]
        .as_str()
        .unwrap()
        .to_owned();
    // An entry named differently from the one Hangar will write (as when a slug changes).
    let index = server_dir(&app, &id).await.join("plugins/.index");
    std::fs::rename(index.join("chunky.pw.toml"), index.join("old-name.pw.toml")).unwrap();

    let started = install(
        &app,
        &id,
        json!({ "source": "hangar", "project_id": "Chunky" }),
        &cookie,
    )
    .await;
    assert_eq!(started["plan"]["steps"][0]["replaces"], json!(from_modrinth));

    let list = content(&app, &id, &cookie).await;
    assert_eq!(list["items"].as_array().unwrap().len(), 1, "{list}");
    assert_eq!(item(&list, "Chunky")["source"]["source"], "hangar");
    assert_eq!(
        names(&index),
        ["chunky.pw.toml"],
        "the old entry went with its file"
    );
}

#[tokio::test]
async fn a_plugin_added_by_hand_counts_as_the_dependency() {
    let app = TestApp::new().await;
    let cookie = app.setup_owner().await;
    let id = server(&app, "paper", &cookie).await;
    let plugins = server_dir(&app, &id).await.join("plugins");
    std::fs::create_dir_all(&plugins).unwrap();
    std::fs::write(
        plugins.join("handmade-via.jar"),
        gsd_mods::test_support::fake_jar("via", "ViaVersion", "5.0", true),
    )
    .unwrap();

    let plan = json_body(
        app.post(
            &url(&id, "/plan"),
            json!({ "source": "hangar", "project_id": "ViaBackwards" }),
            Some(&cookie),
        )
        .await,
    )
    .await;
    assert_eq!(plan["steps"].as_array().unwrap().len(), 1, "{plan}");
    assert_eq!(plan["already_installed"], json!(["ViaVersion"]));
}

#[tokio::test]
async fn one_install_can_mix_sources_but_not_take_a_plugin_from_two() {
    let app = TestApp::new().await;
    let cookie = app.setup_owner().await;
    let id = server(&app, "paper", &cookie).await;

    let both = app
        .post(
            &url(&id, "/plan"),
            json!({ "projects": [{ "project_id": CHUNKY }, { "source": "hangar", "project_id": "Chunky" }] }),
            Some(&cookie),
        )
        .await;
    assert_eq!(both.status(), StatusCode::CONFLICT);
    let error = json_body(both).await;
    assert_eq!(error["error"]["code"], "cannot_install");
    assert!(
        error["error"]["message"]
            .as_str()
            .unwrap()
            .contains("from two sources"),
        "{error}"
    );

    let started = install(
        &app,
        &id,
        json!({ "projects": [{ "project_id": CHUNKY }, { "source": "hangar", "project_id": "ViaVersion" }] }),
        &cookie,
    )
    .await;
    assert_eq!(started["plan"]["steps"].as_array().unwrap().len(), 2);
    let list = content(&app, &id, &cookie).await;
    assert_eq!(item(&list, "Chunky")["source"]["source"], "modrinth");
    assert_eq!(item(&list, "ViaVersion")["source"]["source"], "hangar");

    // The single form and the list don't mix, and a top-level source needs a project.
    let mixed = app
        .post(
            &url(&id, "/plan"),
            json!({ "source": "hangar", "projects": [{ "project_id": "Chunky" }] }),
            Some(&cookie),
        )
        .await;
    assert_eq!(mixed.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

/// A zip of these files, as an uploaded pack or server folder is.
fn zip_of(entries: &[(&str, &[u8])]) -> Vec<u8> {
    use std::io::Write as _;
    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    for (name, contents) in entries {
        zip.start_file(*name, zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(contents).unwrap();
    }
    zip.finish().unwrap().into_inner()
}

/// An `.mrpack` for Fabric on 26.3: Lithium, a client-only file with an address that must never be
/// fetched, and overrides (`server-overrides` winning over `overrides`).
async fn fabric_mrpack(app: &TestApp, id: &str, cookie: &str, extra: &[Value]) -> Vec<u8> {
    let version = &versions(app, id, LITHIUM, cookie).await[0];
    let loaders = json_body(
        app.get(
            "/api/v1/catalog/minecraft/loaders/fabric/versions?mc=26.3",
            Some(cookie),
        )
        .await,
    )
    .await;
    let loader = loaders
        .as_array()
        .and_then(|all| all.iter().find(|entry| entry["recommended"] == true))
        .unwrap_or_else(|| panic!("a recommended Fabric loader: {loaders}"));
    let mut files = vec![
        json!({
            "path": "mods/lithium.jar",
            "hashes": { "sha1": version["file"]["sha1"], "sha512": version["file"]["sha512"] },
            "env": { "client": "required", "server": "required" },
            "downloads": [version["file"]["url"]], "fileSize": version["file"]["size"],
        }),
        json!({
            "path": "mods/client-only.jar", "hashes": { "sha512": "00" },
            "env": { "client": "required", "server": "unsupported" },
            "downloads": ["https://example.invalid/never.jar"], "fileSize": 1,
        }),
    ];
    files.extend_from_slice(extra);
    let index = json!({
        "formatVersion": 1, "game": "minecraft", "versionId": "1.0", "name": "Test Pack",
        "dependencies": { "minecraft": "26.3", "fabric-loader": loader["id"] }, "files": files,
    })
    .to_string();
    zip_of(&[
        ("modrinth.index.json", index.as_bytes()),
        ("overrides/config/pack.txt", b"from overrides"),
        ("overrides/config/both.txt", b"from overrides"),
        ("server-overrides/config/both.txt", b"from server-overrides"),
        ("client-overrides/config/client.txt", b"client only"),
        ("overrides/../../escaped.txt", b"never written"),
    ])
}

/// Uploads a pack and returns what the app made of it.
async fn upload_pack(app: &TestApp, name: &str, pack: Vec<u8>, cookie: &str) -> Value {
    let response = app
        .put_bytes(
            &format!("/api/v1/modpacks/upload?file={name}"),
            pack,
            Some(cookie),
        )
        .await;
    assert_eq!(response.status(), StatusCode::CREATED);
    json_body(response).await
}

/// Installs an uploaded pack and waits for the job; returns the finished job and the server's id.
async fn install_pack(app: &TestApp, upload_id: &Value, cookie: &str) -> (Value, String) {
    let body = json!({ "upload_id": upload_id, "name": "From a pack", "max_memory_mb": 1024, "eula_accepted": true });
    let response = app.post("/api/v1/modpacks/install", body, Some(cookie)).await;
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    let started = json_body(response).await;
    let job = app
        .wait_for_job(started["job_id"].as_str().unwrap(), cookie)
        .await;
    (job, started["instance"]["id"].as_str().unwrap().to_owned())
}

#[tokio::test]
async fn an_mrpack_becomes_a_server_with_its_files_and_overrides() {
    let app = TestApp::new().await;
    let cookie = app.setup_owner().await;
    let other = server(&app, "fabric", &cookie).await;
    let pack = fabric_mrpack(&app, &other, &cookie, &[]).await;

    let uploaded = upload_pack(&app, "pack.mrpack", pack, &cookie).await;
    assert_eq!(uploaded["kind"], "pack", "{uploaded}");
    assert_eq!(uploaded["pack"]["name"], "Test Pack");
    assert_eq!(uploaded["pack"]["loader"], "fabric");
    assert_eq!(uploaded["pack"]["files"], 1, "not the client-only one");

    let (job, id) = install_pack(&app, &uploaded["upload_id"], &cookie).await;
    assert_eq!(job["status"], "succeeded", "{job}");

    let dir = server_dir(&app, &id).await;
    assert!(dir.join("mods/lithium.jar").is_file());
    assert!(!dir.join("mods/client-only.jar").exists());
    assert_eq!(
        std::fs::read_to_string(dir.join("config/pack.txt")).unwrap(),
        "from overrides"
    );
    assert_eq!(
        std::fs::read_to_string(dir.join("config/both.txt")).unwrap(),
        "from server-overrides",
        "the server's own folder wins"
    );
    assert!(!dir.join("config/client.txt").exists());
    let escaped = dir.ancestors().any(|folder| folder.join("escaped.txt").exists());
    assert!(!escaped, "an entry can't leave the server");

    // Installed, and the file Modrinth knows is tracked under its own name.
    let list = content(&app, &id, &cookie).await;
    let lithium = item(&list, "Lithium");
    assert_eq!(lithium["file"], "lithium.jar");
    assert_eq!(lithium["source"]["project_id"], LITHIUM);

    // The upload is used up.
    let again = app
        .post(
            "/api/v1/modpacks/install",
            json!({ "upload_id": uploaded["upload_id"], "name": "Again", "max_memory_mb": 1024, "eula_accepted": true }),
            Some(&cookie),
        )
        .await;
    assert_eq!(json_body(again).await["error"]["code"], "upload_not_found");
}

#[tokio::test]
async fn a_pack_cannot_aim_downloads_anywhere_or_write_outside_the_server() {
    let app = TestApp::new().await;
    let cookie = app.setup_owner().await;
    let other = server(&app, "fabric", &cookie).await;

    let elsewhere = json!({
        "path": "mods/evil.jar", "hashes": { "sha512": "00" }, "env": { "server": "required" },
        "downloads": ["http://169.254.169.254/latest/meta-data"], "fileSize": 1,
    });
    let pack = fabric_mrpack(&app, &other, &cookie, &[elsewhere]).await;
    let uploaded = upload_pack(&app, "evil.mrpack", pack, &cookie).await;
    let (job, _) = install_pack(&app, &uploaded["upload_id"], &cookie).await;
    assert_eq!(job["status"], "failed", "{job}");
    assert!(
        job["error"].as_str().unwrap().contains("hosted somewhere"),
        "{job}"
    );

    let version = &versions(&app, &other, LITHIUM, &cookie).await[0];
    let outside = json!({
        "path": "../../../escaped.jar",
        "hashes": { "sha512": version["file"]["sha512"] }, "env": { "server": "required" },
        "downloads": [version["file"]["url"]], "fileSize": version["file"]["size"],
    });
    let pack = fabric_mrpack(&app, &other, &cookie, &[outside]).await;
    let uploaded = upload_pack(&app, "evil.mrpack", pack, &cookie).await;
    let (job, id) = install_pack(&app, &uploaded["upload_id"], &cookie).await;
    assert_eq!(job["status"], "failed", "{job}");
    let dir = server_dir(&app, &id).await;
    assert!(
        !dir.ancestors().any(|folder| folder.join("escaped.jar").exists()),
        "nothing outside the server"
    );
}

#[tokio::test]
async fn a_changed_download_fails_the_pack() {
    let app = TestApp::new().await;
    let cookie = app.setup_owner().await;
    let other = server(&app, "fabric", &cookie).await;
    let version = &versions(&app, &other, LITHIUM, &cookie).await[0];
    let tampered = json!({
        "path": "mods/tampered.jar", "hashes": { "sha512": "0".repeat(128) }, "env": { "server": "required" },
        "downloads": [version["file"]["url"]], "fileSize": 1,
    });
    let pack = fabric_mrpack(&app, &other, &cookie, &[tampered]).await;
    let uploaded = upload_pack(&app, "pack.mrpack", pack, &cookie).await;
    let (job, id) = install_pack(&app, &uploaded["upload_id"], &cookie).await;
    assert_eq!(job["status"], "failed", "{job}");
    assert!(!server_dir(&app, &id).await.join("mods/tampered.jar").exists());
}

#[tokio::test]
async fn broken_archives_and_rights_are_refused() {
    let app = TestApp::new().await;
    let owner = app.setup_owner().await;
    let not_a_zip = app
        .put_bytes(
            "/api/v1/modpacks/upload?file=x.zip",
            b"nope".to_vec(),
            Some(&owner),
        )
        .await;
    assert_eq!(not_a_zip.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(json_body(not_a_zip).await["error"]["code"], "invalid_modpack");
    let unsupported = zip_of(&[(
        "modrinth.index.json",
        json!({ "game": "minecraft", "name": "x", "files": [], "dependencies": { "minecraft": "26.3", "rift": "1" } })
            .to_string()
            .as_bytes(),
    )]);
    let response = app
        .put_bytes("/api/v1/modpacks/upload?file=x.mrpack", unsupported, Some(&owner))
        .await;
    assert!(
        json_body(response).await["error"]["message"]
            .as_str()
            .unwrap()
            .contains("rift")
    );
    for upload_id in ["../../gsd", "", "nothing-here"] {
        let response = app
            .post(
                "/api/v1/modpacks/install",
                json!({ "upload_id": upload_id, "name": "x", "max_memory_mb": 1024, "eula_accepted": true }),
                Some(&owner),
            )
            .await;
        assert_eq!(json_body(response).await["error"]["code"], "upload_not_found");
    }

    app.create_user("member", GlobalRole::Member).await;
    let member = app.login("member").await;
    let forbidden = app
        .put_bytes("/api/v1/modpacks/upload?file=x.zip", b"x".to_vec(), Some(&member))
        .await;
    assert_eq!(forbidden.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn a_curseforge_pack_needs_the_key_and_brings_its_files() {
    let app = TestApp::new().await;
    let cookie = app.setup_owner().await;
    let loaders = json_body(
        app.get(
            "/api/v1/catalog/minecraft/loaders/fabric/versions?mc=26.3",
            Some(&cookie),
        )
        .await,
    )
    .await;
    let loader = loaders[0]["id"].as_str().unwrap();
    let manifest = json!({
        "minecraft": { "version": "26.3", "modLoaders": [{ "id": format!("fabric-{loader}"), "primary": true }] },
        "manifestType": "minecraftModpack", "name": "CF Pack", "version": "1", "overrides": "overrides",
        "files": [
            { "projectID": CF_MOD, "fileID": CF_MOD_FILES[0], "required": true },
            { "projectID": CF_BLOCKED, "fileID": 5201, "required": true },
            { "projectID": CF_LIBRARY, "fileID": 5101, "required": false },
        ],
    })
    .to_string();
    let pack = zip_of(&[
        ("manifest.json", manifest.as_bytes()),
        ("overrides/config/cf.txt", b"from the pack"),
    ]);
    let uploaded = upload_pack(&app, "cf.zip", pack, &cookie).await;
    assert_eq!(uploaded["pack"]["format"], "curseforge", "{uploaded}");

    let body = json!({ "upload_id": uploaded["upload_id"], "name": "CF", "max_memory_mb": 1024, "eula_accepted": true });
    let without_key = app.post("/api/v1/modpacks/install", body, Some(&cookie)).await;
    assert_eq!(
        json_body(without_key).await["error"]["code"],
        "source_key_missing"
    );

    set_curseforge_key(&app, &cookie).await;
    let (job, id) = install_pack(&app, &uploaded["upload_id"], &cookie).await;
    assert_eq!(job["status"], "succeeded", "{job}");
    let dir = server_dir(&app, &id).await;
    assert!(dir.join("mods/cf-mod-2.jar").is_file());
    assert!(!dir.join("mods/cf-library.jar").exists(), "not required");
    assert!(!dir.join("mods/cf-blocked.jar").exists(), "only by hand");
    assert!(
        job["log"].to_string().contains("cf-blocked.jar"),
        "the job says what is missing: {job}"
    );
    assert!(dir.join("config/cf.txt").is_file());
}

#[tokio::test]
async fn a_zipped_server_goes_to_the_imports_folder_without_its_top_folder() {
    let app = TestApp::new().await;
    let cookie = app.setup_owner().await;
    let zip = zip_of(&[
        ("my-pack/server.jar", b"a server"),
        ("my-pack/mods/handmade.jar", b"a mod"),
        ("my-pack/server.properties", b"motd=hi\n"),
    ]);
    let uploaded = upload_pack(&app, "My%20Server.zip", zip.clone(), &cookie).await;
    assert_eq!(uploaded["kind"], "server_folder", "{uploaded}");
    assert_eq!(uploaded["folder"], "My-Server");

    let imports = app.state.config.imports_dir();
    assert!(imports.join("My-Server/server.jar").is_file());
    assert!(imports.join("My-Server/mods/handmade.jar").is_file());
    let view = json_body(app.get("/api/v1/imports", Some(&cookie)).await).await;
    assert_eq!(view["candidates"][0]["folder"], "My-Server", "{view}");

    let second = upload_pack(&app, "My%20Server.zip", zip, &cookie).await;
    assert_eq!(second["folder"], "My-Server-2", "the first one stays");
}

#[tokio::test]
async fn an_imported_server_gets_its_jars_looked_up() {
    let app = TestApp::new().await;
    let cookie = app.setup_owner().await;
    let other = server(&app, "fabric", &cookie).await;
    install(&app, &other, json!({ "project_id": LITHIUM }), &cookie).await;
    let list = content(&app, &other, &cookie).await;
    let file = item(&list, "Lithium")["file"].as_str().unwrap().to_owned();
    let jar = std::fs::read(server_dir(&app, &other).await.join("mods").join(&file)).unwrap();

    let zip = zip_of(&[("server.jar", b"a server"), ("mods/by-hand.jar", &jar)]);
    let uploaded = upload_pack(&app, "imported.zip", zip, &cookie).await;
    assert_eq!(uploaded["folder"], "imported");

    let body = json!({ "folder": "imported", "name": "Imported", "mc_version": "26.3", "loader": "fabric", "jar": "server.jar", "max_memory_mb": 1024, "eula_accepted": true });
    let started = json_body(app.post("/api/v1/imports", body, Some(&cookie)).await).await;
    let job = app
        .wait_for_job(started["job_id"].as_str().unwrap(), &cookie)
        .await;
    assert_eq!(job["status"], "succeeded", "{job}");
    let id = started["instance"]["id"].as_str().unwrap();

    // The lookup follows the job, so give it a moment.
    let mut source = Value::Null;
    for _ in 0..100 {
        let list = content(&app, id, &cookie).await;
        source = list["items"][0]["source"]["project_id"].clone();
        if !source.is_null() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert_eq!(source, LITHIUM, "tracked after the import");
}

/// Saves the mock's key the way an owner does.
async fn set_curseforge_key(app: &TestApp, cookie: &str) {
    let response = app
        .put(
            "/api/v1/settings/curseforge-key",
            json!({ "key": CURSEFORGE_KEY }),
            Some(cookie),
        )
        .await;
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn the_curseforge_key_is_write_only_checked_and_for_owners() {
    let app = TestApp::new().await;
    let owner = app.setup_owner().await;
    let id = server(&app, "fabric", &owner).await;
    assert_eq!(
        content(&app, &id, &owner).await["sources"],
        json!(["modrinth"]),
        "no key, no CurseForge"
    );
    let before = app
        .get(&url(&id, "/search?source=curseforge"), Some(&owner))
        .await;
    assert_eq!(before.status(), StatusCode::CONFLICT);
    assert_eq!(json_body(before).await["error"]["code"], "source_key_missing");

    let wrong = app
        .put(
            "/api/v1/settings/curseforge-key",
            json!({ "key": "wrong-key" }),
            Some(&owner),
        )
        .await;
    assert_eq!(wrong.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(json_body(wrong).await["error"]["code"], "curseforge_key_refused");
    let shown = json_body(app.get("/api/v1/settings", Some(&owner)).await).await;
    assert_eq!(shown, json!({ "curseforge_key_set": false }), "nothing was saved");

    set_curseforge_key(&app, &owner).await;
    let shown = app.get("/api/v1/settings", Some(&owner)).await;
    let text = String::from_utf8(
        http_body_util::BodyExt::collect(shown.into_body())
            .await
            .unwrap()
            .to_bytes()
            .to_vec(),
    )
    .unwrap();
    assert_eq!(text, r#"{"curseforge_key_set":true}"#, "set, and never shown");
    assert_eq!(
        content(&app, &id, &owner).await["sources"],
        json!(["modrinth", "curseforge"])
    );
    let audit: Vec<String> = sqlx::query_scalar("SELECT details_json FROM audit_log")
        .fetch_all(&app.pool)
        .await
        .unwrap();
    assert!(
        audit.iter().all(|details| !details.contains(CURSEFORGE_KEY)),
        "{audit:?}"
    );

    // Not for anyone else: a member can't see or change it.
    app.create_user("member", GlobalRole::Member).await;
    let member = app.login("member").await;
    assert_eq!(
        app.get("/api/v1/settings", Some(&member)).await.status(),
        StatusCode::FORBIDDEN
    );
    let forbidden = app
        .put(
            "/api/v1/settings/curseforge-key",
            json!({ "key": "x" }),
            Some(&member),
        )
        .await;
    assert_eq!(forbidden.status(), StatusCode::FORBIDDEN);
    assert_eq!(
        app.delete("/api/v1/settings/curseforge-key", Some(&member))
            .await
            .status(),
        StatusCode::FORBIDDEN
    );

    let removed = app.delete("/api/v1/settings/curseforge-key", Some(&owner)).await;
    assert_eq!(removed.status(), StatusCode::NO_CONTENT);
    assert_eq!(content(&app, &id, &owner).await["sources"], json!(["modrinth"]));
}

#[tokio::test]
async fn the_saved_curseforge_key_is_used_after_a_restart() {
    let app = TestApp::new().await;
    let owner = app.setup_owner().await;
    set_curseforge_key(&app, &owner).await;
    // As a restart does: a new app on the same database.
    let dir = tempfile::tempdir().unwrap();
    let options = gsd_app::state::BuildOptions {
        sandbox: Some(gsd_core::supervisor::HostSandbox::new(
            gsd_core::supervisor::SandboxSupport::none(),
            None,
        )),
        probe_ports: Some(false),
        ..Default::default()
    };
    let restarted = gsd_app::state::AppState::build_with(
        gsd_app::config::Config::with_data_dir(dir.path().to_owned()),
        app.pool.clone(),
        options,
    )
    .await
    .unwrap();
    assert!(restarted.content.has_curseforge_key());
}

#[tokio::test]
async fn curseforge_mods_install_with_dependencies_and_update_after_a_backup() {
    let app = TestApp::new().await;
    let cookie = app.setup_owner().await;
    set_curseforge_key(&app, &cookie).await;
    let id = server(&app, "fabric", &cookie).await;
    let page = json_body(
        app.get(&url(&id, "/search?source=curseforge"), Some(&cookie))
            .await,
    )
    .await;
    assert_eq!(page["hits"][0]["source"], "curseforge", "{page}");

    let [newest, older] = CF_MOD_FILES;
    let started = install(
        &app,
        &id,
        json!({ "source": "curseforge", "project_id": CF_MOD.to_string(), "version_id": older.to_string() }),
        &cookie,
    )
    .await;
    assert_eq!(
        started["plan"]["steps"][1]["project"]["id"],
        CF_LIBRARY.to_string(),
        "the required library, from CurseForge too"
    );
    let list = content(&app, &id, &cookie).await;
    let found = item(&list, "CF Mod");
    assert_eq!(found["source"]["source"], "curseforge");
    assert_eq!(found["source"]["version_id"], older.to_string());
    assert!(found["source"]["page_url"].as_str().unwrap().ends_with("/cf-mod"));
    let index =
        std::fs::read_to_string(server_dir(&app, &id).await.join("mods/.index/cf-mod.pw.toml")).unwrap();
    assert!(
        index.contains("hash-format = \"sha1\"") && index.contains("project-id = 1002"),
        "{index}"
    );

    let check = updates(&app, &id, &cookie).await;
    let available = check["updates"].as_array().unwrap();
    assert_eq!(available.len(), 1, "{check}");
    assert_eq!(available[0]["source"], "curseforge");
    assert_eq!(available[0]["version"]["id"], newest.to_string());
    update(&app, &id, json!({}), &cookie).await;
    let list = content(&app, &id, &cookie).await;
    assert_eq!(list["items"].as_array().unwrap().len(), 2, "{list}");
    assert_eq!(item(&list, "CF Mod")["file"], "cf-mod-2.jar");
    assert_eq!(updates(&app, &id, &cookie).await["updates"], json!([]));
}

#[tokio::test]
async fn bukkit_plugins_come_from_curseforge_too() {
    let app = TestApp::new().await;
    let cookie = app.setup_owner().await;
    set_curseforge_key(&app, &cookie).await;
    let id = server(&app, "paper", &cookie).await;
    assert_eq!(
        content(&app, &id, &cookie).await["sources"],
        json!(["modrinth", "hangar", "curseforge"])
    );
    let page = json_body(
        app.get(&url(&id, "/search?source=curseforge"), Some(&cookie))
            .await,
    )
    .await;
    assert_eq!(page["hits"][0]["title"], "CF Plugin", "{page}");
    install(
        &app,
        &id,
        json!({ "source": "curseforge", "project_id": CF_PLUGIN.to_string() }),
        &cookie,
    )
    .await;
    assert!(
        server_dir(&app, &id)
            .await
            .join("plugins/cf-plugin.jar")
            .is_file()
    );
}

#[tokio::test]
async fn a_file_its_author_blocks_is_left_for_the_user_and_recognised_when_uploaded() {
    let app = TestApp::new().await;
    let cookie = app.setup_owner().await;
    set_curseforge_key(&app, &cookie).await;
    let id = server(&app, "fabric", &cookie).await;

    let started = install(
        &app,
        &id,
        json!({ "source": "curseforge", "project_id": CF_BLOCKED.to_string() }),
        &cookie,
    )
    .await;
    let file = &started["plan"]["steps"][0]["version"]["file"];
    assert!(
        file["manual_url"]
            .as_str()
            .unwrap()
            .ends_with("/cf-blocked/files/5201"),
        "{file}"
    );
    assert_eq!(
        content(&app, &id, &cookie).await["items"],
        json!([]),
        "not downloaded, and the job still finishes"
    );

    let upload_url = url(&id, "/upload?file=cf-blocked.jar");
    let jar = gsd_mods::test_support::curseforge_blocked_jar();
    let response = app.put_bytes(&upload_url, jar.clone(), Some(&cookie)).await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let identified = json_body(response).await;
    assert_eq!(identified["identified"], json!(["CF Blocked"]), "{identified}");
    let list = content(&app, &id, &cookie).await;
    assert_eq!(item(&list, "CF Blocked")["source"]["source"], "curseforge");

    let again = app.put_bytes(&upload_url, jar.clone(), Some(&cookie)).await;
    assert_eq!(again.status(), StatusCode::CONFLICT, "an existing file stays");
    let not_a_jar = app
        .put_bytes(&url(&id, "/upload?file=notes.txt"), jar.clone(), Some(&cookie))
        .await;
    assert_eq!(json_body(not_a_jar).await["error"]["code"], "not_a_jar");
    let outside = app
        .put_bytes(
            &url(&id, "/upload?file=..%2Fevil.jar"),
            jar.clone(),
            Some(&cookie),
        )
        .await;
    assert!(outside.status().is_client_error(), "{}", outside.status());
    assert!(!server_dir(&app, &id).await.join("evil.jar").exists());

    // Needs `content.manage`.
    let viewer = app.create_user("viewer", GlobalRole::Member).await;
    app.state
        .auth
        .grants()
        .set(viewer, id.parse().unwrap(), Grant::Viewer)
        .await
        .unwrap();
    let viewer = app.login("viewer").await;
    let forbidden = app
        .put_bytes(&url(&id, "/upload?file=other.jar"), jar, Some(&viewer))
        .await;
    assert_eq!(forbidden.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn hand_added_jars_are_identified_on_curseforge_when_modrinth_does_not_know_them() {
    let app = TestApp::new().await;
    let cookie = app.setup_owner().await;
    let id = server(&app, "fabric", &cookie).await;
    let mods = server_dir(&app, &id).await.join("mods");
    std::fs::create_dir_all(&mods).unwrap();
    std::fs::write(
        mods.join("from-curseforge.jar"),
        gsd_mods::test_support::curseforge_blocked_jar(),
    )
    .unwrap();
    std::fs::write(
        mods.join("unknown.jar"),
        gsd_mods::test_support::fake_jar("unknown", "Unknown", "1", false),
    )
    .unwrap();
    let identify = async || json_body(app.post(&url(&id, "/identify"), json!({}), Some(&cookie)).await).await;

    let without_key = identify().await;
    assert_eq!(without_key["identified"], json!([]), "{without_key}");

    set_curseforge_key(&app, &cookie).await;
    let found = identify().await;
    assert_eq!(found["identified"], json!(["CF Blocked"]), "{found}");
    assert_eq!(found["still_unknown"], json!(["unknown.jar"]));
    assert_eq!(
        item(&content(&app, &id, &cookie).await, "CF Blocked")["file"],
        "from-curseforge.jar",
        "the file keeps its name"
    );
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

#[tokio::test]
async fn several_projects_install_in_one_job_with_each_dependency_once() {
    let app = TestApp::new().await;
    let cookie = app.setup_owner().await;
    let id = server(&app, "fabric", &cookie).await;
    // Both need Fabric API; Lithium needs nothing.
    let body = json!({ "projects": [
        { "project_id": APPLESKIN },
        { "project_id": ARCHITECTURY },
        { "project_id": LITHIUM, "version_id": null },
    ]});

    let plan = json_body(app.post(&url(&id, "/plan"), body.clone(), Some(&cookie)).await).await;
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
    assert_eq!(
        steps,
        [
            ("AppleSkin", None),
            ("Architectury API", None),
            ("Lithium", None),
            ("Fabric API", Some("AppleSkin")),
        ]
    );

    let started = install(&app, &id, body, &cookie).await;
    assert_eq!(started["plan"], plan);
    let list = content(&app, &id, &cookie).await;
    let mut titles: Vec<&str> = list["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["name"].as_str().unwrap())
        .collect();
    titles.sort_unstable();
    assert_eq!(titles, ["AppleSkin", "Architectury API", "Fabric API", "Lithium"]);
    let mods = server_dir(&app, &id).await.join("mods");
    assert_eq!(names(&mods.join(".index")).len(), 4);
}

#[tokio::test]
async fn an_install_asks_for_one_project_or_a_list() {
    let app = TestApp::new().await;
    let cookie = app.setup_owner().await;
    let id = server(&app, "fabric", &cookie).await;
    let many: Vec<Value> = (0..33).map(|_| json!({ "project_id": LITHIUM })).collect();
    for body in [
        json!({}),
        json!({ "projects": [] }),
        json!({ "projects": many }),
        json!({ "project_id": LITHIUM, "projects": [{ "project_id": APPLESKIN }] }),
        json!({ "version_id": "x", "projects": [{ "project_id": APPLESKIN }] }),
    ] {
        for path in ["/plan", "/install"] {
            let response = app.post(&url(&id, path), body.clone(), Some(&cookie)).await;
            assert_eq!(
                response.status(),
                StatusCode::UNPROCESSABLE_ENTITY,
                "{path} {body}"
            );
            assert_eq!(
                json_body(response).await["error"]["code"],
                "invalid_install_request"
            );
        }
    }
    let unknown_field = app
        .post(
            &url(&id, "/plan"),
            json!({ "projects": [{ "project_id": LITHIUM, "colour": "red" }] }),
            Some(&cookie),
        )
        .await;
    assert!(
        unknown_field.status().is_client_error(),
        "{}",
        unknown_field.status()
    );
    assert_eq!(
        content(&app, &id, &cookie).await["items"],
        json!([]),
        "nothing was installed"
    );
}
