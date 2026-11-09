//! Finding modpacks on a source and fetching them: search, versions, the download job, what it turned
//! out to be, a `CurseForge` server pack, files only downloadable by hand, and the pack being recorded
//! on the server it becomes. Each test adds its pack to the mock sources.

#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use axum::http::StatusCode;
use common::{TestApp, json_body};
use gsd_core::{auth::GlobalRole, ids::InstanceId};
use gsd_mods::test_support::{CF_MOD, CF_MOD_FILES, CURSEFORGE_KEY, fixture, sha1_hex};
use serde_json::{Value, json};
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{method, path, query_param, query_param_contains},
};

const PACK: &str = "packPACK";
const CF_PACK: u64 = 7001;
const CF_FILE: u64 = 7101;
const CF_SERVER_FILE: u64 = 7102;

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

async fn fabric_loader(app: &TestApp, cookie: &str) -> String {
    let loaders = json_body(
        app.get(
            "/api/v1/catalog/minecraft/loaders/fabric/versions?mc=26.3",
            Some(cookie),
        )
        .await,
    )
    .await;
    loaders[0]["id"].as_str().unwrap().to_owned()
}

/// An `.mrpack` with nothing to download, only a file of its own.
async fn mrpack(app: &TestApp, cookie: &str) -> Vec<u8> {
    let index = json!({
        "formatVersion": 1, "game": "minecraft", "versionId": "2.0", "name": "Found Pack",
        "dependencies": { "minecraft": "26.3", "fabric-loader": fabric_loader(app, cookie).await }, "files": [],
    })
    .to_string();
    zip_of(&[
        ("modrinth.index.json", index.as_bytes()),
        ("overrides/config/pack.txt", b"from the pack"),
    ])
}

/// A Modrinth version of [`PACK`] whose file has these `contents`, but the hash of `published`.
async fn serve_modrinth_version(
    server: &MockServer,
    id: &str,
    channel: &str,
    contents: &[u8],
    published: &[u8],
) -> Value {
    let route = format!("/files/{id}/pack.mrpack");
    Mock::given(method("GET"))
        .and(path(route.clone()))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(contents.to_vec()))
        .mount(server)
        .await;
    let mut version = fixture("versions_lithium_fabric_26.3.json")[0].clone();
    version["id"] = id.into();
    version["project_id"] = PACK.into();
    version["version_type"] = channel.into();
    version["dependencies"] = json!([]);
    version["files"] = json!([{
        "url": format!("{}{route}", server.uri()), "filename": "pack.mrpack", "primary": true, "size": contents.len(),
        "hashes": { "sha1": sha1_hex(published) },
    }]);
    version
}

async fn list_modrinth_versions(server: &MockServer, versions: &[Value]) {
    Mock::given(method("GET"))
        .and(path(format!("/project/{PACK}/version")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!(versions)))
        .mount(server)
        .await;
}

/// Fetches and waits for the job: the started response and the finished job.
async fn fetch(app: &TestApp, body: Value, cookie: &str) -> (Value, Value) {
    let response = app.post("/api/v1/modpacks/fetch", body, Some(cookie)).await;
    let status = response.status();
    let started = json_body(response).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{started}");
    let job = app
        .wait_for_job(started["job_id"].as_str().unwrap(), cookie)
        .await;
    (started, job)
}

async fn fetched(app: &TestApp, started: &Value, cookie: &str) -> (StatusCode, Value) {
    let route = format!(
        "/api/v1/modpacks/uploads/{}",
        started["upload_id"].as_str().unwrap()
    );
    let response = app.get(&route, Some(cookie)).await;
    (response.status(), json_body(response).await)
}

/// Creates a server from what is waiting under `upload_id`, and waits: its id and the finished job.
async fn install(app: &TestApp, upload_id: &Value, cookie: &str) -> (InstanceId, Value) {
    let body = json!({ "upload_id": upload_id, "name": "From a pack", "max_memory_mb": 1024, "eula_accepted": true });
    let response = app.post("/api/v1/modpacks/install", body, Some(cookie)).await;
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    let created = json_body(response).await;
    let job = app
        .wait_for_job(created["job_id"].as_str().unwrap(), cookie)
        .await;
    let id = created["instance"]["id"].as_str().unwrap().parse().unwrap();
    (id, job)
}

/// A `CurseForge` pack's `manifest.json` for Fabric on 26.3, naming these (project, file) pairs.
async fn cf_manifest(app: &TestApp, cookie: &str, files: &[(u64, u64)]) -> String {
    let files: Vec<Value> = files
        .iter()
        .map(|(project, file)| json!({ "projectID": project, "fileID": file, "required": true }))
        .collect();
    json!({
        "minecraft": { "version": "26.3", "modLoaders": [{ "id": format!("fabric-{}", fabric_loader(app, cookie).await), "primary": true }] },
        "manifestType": "minecraftModpack", "name": "CF Pack", "version": "1", "overrides": "overrides", "files": files,
    })
    .to_string()
}

async fn set_curseforge_key(app: &TestApp, cookie: &str) {
    let body = json!({ "key": CURSEFORGE_KEY });
    let response = app
        .put("/api/v1/settings/curseforge-key", body, Some(cookie))
        .await;
    assert_eq!(response.status(), StatusCode::OK);
}

/// A file of [`CF_PACK`], as `CurseForge` describes it. `contents: None` means only by hand.
fn cf_file(
    server: &MockServer,
    id: u64,
    name: &str,
    contents: Option<&[u8]>,
    server_pack: Option<u64>,
) -> Value {
    json!({
        "id": id, "gameId": 432, "modId": CF_PACK, "isAvailable": true, "displayName": name, "fileName": name,
        "releaseType": 1, "fileStatus": 4, "fileDate": "2026-11-01T00:00:00Z", "fileLength": contents.map_or(0, <[u8]>::len),
        "hashes": [{ "value": sha1_hex(contents.unwrap_or_default()), "algo": 1 }],
        "downloadUrl": contents.map(|_| format!("{}/files/{id}/{name}", server.uri())),
        "gameVersions": ["26.3", "Fabric"], "dependencies": [], "fileFingerprint": id,
        "isServerPack": id == CF_SERVER_FILE,
        "serverPackFileId": server_pack,
    })
}

/// Adds [`CF_PACK`] with the file listed by `main`, and the server version `server_file` if any.
async fn serve_curseforge_pack(server: &MockServer, main: Value, server_file: Option<Value>) {
    let mut listed = vec![main.clone()];
    let mut all = vec![main];
    if let Some(server_file) = server_file {
        all.push(server_file.clone());
        // CurseForge lists server packs along with the rest.
        listed.push(server_file);
    }
    let body = |value: Value| ResponseTemplate::new(200).set_body_json(json!({ "data": value }));
    Mock::given(method("GET"))
        .and(path(format!("/mods/{CF_PACK}/files")))
        .respond_with(body(json!(listed)))
        .mount(server)
        .await;
    for file in all {
        Mock::given(method("GET"))
            .and(path(format!("/mods/{CF_PACK}/files/{}", file["id"])))
            .respond_with(body(file))
            .mount(server)
            .await;
    }
    Mock::given(method("GET"))
        .and(path(format!("/mods/{CF_PACK}")))
        .respond_with(body(json!({
            "id": CF_PACK, "name": "CF Pack", "slug": "cf-pack", "classId": 4471,
            "links": { "websiteUrl": "https://www.curseforge.com/minecraft/modpacks/cf-pack" },
        })))
        .mount(server)
        .await;
}

async fn serve_bytes(server: &MockServer, id: u64, name: &str, contents: &[u8]) {
    Mock::given(method("GET"))
        .and(path(format!("/files/{id}/{name}")))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(contents.to_vec()))
        .mount(server)
        .await;
}

#[tokio::test]
async fn searching_modrinth_asks_for_server_modpacks_of_that_minecraft_version() {
    let app = TestApp::new().await;
    let cookie = app.setup_owner().await;
    let answer = fixture("search_fabric_26.3.json");
    let total = answer["total_hits"].clone();
    // Above the mock's own search, whose "project_type:mod" also matches "project_type:modpack".
    Mock::given(method("GET"))
        .and(path("/search"))
        .and(query_param_contains("facets", "project_type:modpack"))
        .and(query_param_contains("facets", "server_side:required"))
        .and(query_param_contains("facets", "versions:26.3"))
        .and(query_param("query", "create"))
        .respond_with(ResponseTemplate::new(200).set_body_json(answer))
        .with_priority(1)
        .mount(&app.modrinth)
        .await;

    let page = json_body(
        app.get("/api/v1/modpacks/search?q=create&mc=26.3", Some(&cookie))
            .await,
    )
    .await;
    assert_eq!(page["total"], total, "{page}");
    assert_eq!(page["hits"][0]["source"], "modrinth", "{page}");

    let sources = json_body(app.get("/api/v1/modpacks/sources", Some(&cookie)).await).await;
    assert_eq!(
        sources["sources"],
        json!(["modrinth"]),
        "CurseForge needs its key"
    );
    set_curseforge_key(&app, &cookie).await;
    let sources = json_body(app.get("/api/v1/modpacks/sources", Some(&cookie)).await).await;
    assert_eq!(sources["sources"], json!(["modrinth", "curseforge"]));
}

#[tokio::test]
async fn a_pack_found_on_modrinth_becomes_a_server_that_remembers_it() {
    let app = TestApp::new().await;
    let cookie = app.setup_owner().await;
    let pack = mrpack(&app, &cookie).await;
    let beta = serve_modrinth_version(
        &app.modrinth,
        "newbeta",
        "beta",
        b"never fetched",
        b"never fetched",
    )
    .await;
    let release = serve_modrinth_version(&app.modrinth, "release", "release", &pack, &pack).await;
    list_modrinth_versions(&app.modrinth, &[beta, release]).await;

    let versions = json_body(
        app.get(
            &format!("/api/v1/modpacks/projects/{PACK}/versions?source=modrinth"),
            Some(&cookie),
        )
        .await,
    )
    .await;
    assert_eq!(versions.as_array().unwrap().len(), 2, "{versions}");

    let (started, job) = fetch(&app, json!({ "source": "modrinth", "project_id": PACK }), &cookie).await;
    assert_eq!(
        job["status"], "succeeded",
        "the release, not the newer beta: {job}"
    );
    let (status, uploaded) = fetched(&app, &started, &cookie).await;
    assert_eq!(status, StatusCode::OK, "{uploaded}");
    assert_eq!(uploaded["kind"], "pack");
    assert_eq!(uploaded["pack"]["name"], "Found Pack");
    assert_eq!(uploaded["upload_id"], started["upload_id"]);

    let body = json!({ "upload_id": started["upload_id"], "name": "Found", "max_memory_mb": 1024, "eula_accepted": true });
    let response = app.post("/api/v1/modpacks/install", body, Some(&cookie)).await;
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    let created = json_body(response).await;
    let job = app
        .wait_for_job(created["job_id"].as_str().unwrap(), &cookie)
        .await;
    assert_eq!(job["status"], "succeeded", "{job}");

    let id: InstanceId = created["instance"]["id"].as_str().unwrap().parse().unwrap();
    let instance = app.state.instances.registry().get(id).await.unwrap();
    assert!(instance.layout.server_dir().join("config/pack.txt").is_file());
    let record = instance
        .definition
        .modpack
        .as_ref()
        .expect("the pack is recorded");
    assert_eq!(record.source, "modrinth");
    assert_eq!(record.project_id.as_deref(), Some(PACK));
    assert_eq!(record.version_id.as_deref(), Some("release"));
    assert_eq!(record.name, "Found Pack");
    assert_eq!(record.version.as_deref(), Some("2.0"));

    let reloaded = gsd_core::instances::InstanceDefinition::load(&instance.layout.definition_path()).unwrap();
    assert_eq!(reloaded.modpack.as_ref(), Some(record), "it is in instance.toml");
    let shown = json_body(app.get(&format!("/api/v1/instances/{id}"), Some(&cookie)).await).await;
    assert_eq!(shown["modpack"]["name"], "Found Pack", "{shown}");

    let uploads = app.state.config.data_dir.join("cache/modpacks");
    assert_eq!(
        std::fs::read_dir(uploads).unwrap().count(),
        0,
        "nothing is left waiting"
    );
}

#[tokio::test]
async fn an_uploaded_pack_is_recorded_as_a_file() {
    let app = TestApp::new().await;
    let cookie = app.setup_owner().await;
    let pack = mrpack(&app, &cookie).await;
    let response = app
        .put_bytes("/api/v1/modpacks/upload?file=p.mrpack", pack, Some(&cookie))
        .await;
    let uploaded = json_body(response).await;
    let body = json!({ "upload_id": uploaded["upload_id"], "name": "Uploaded", "max_memory_mb": 1024, "eula_accepted": true });
    let created = json_body(app.post("/api/v1/modpacks/install", body, Some(&cookie)).await).await;
    let job = app
        .wait_for_job(created["job_id"].as_str().unwrap(), &cookie)
        .await;
    assert_eq!(job["status"], "succeeded", "{job}");
    let id = created["instance"]["id"].as_str().unwrap();
    let shown = json_body(app.get(&format!("/api/v1/instances/{id}"), Some(&cookie)).await).await;
    assert_eq!(
        shown["modpack"],
        json!({ "source": "file", "name": "Found Pack", "version": "2.0" })
    );
}

#[tokio::test]
async fn a_changed_download_fails_the_fetch_and_leaves_nothing() {
    let app = TestApp::new().await;
    let cookie = app.setup_owner().await;
    let version = serve_modrinth_version(
        &app.modrinth,
        "tampered",
        "release",
        b"swapped",
        b"what was published",
    )
    .await;
    list_modrinth_versions(&app.modrinth, &[version]).await;

    let (started, job) = fetch(
        &app,
        json!({ "source": "modrinth", "project_id": PACK, "version_id": "tampered" }),
        &cookie,
    )
    .await;
    assert_eq!(job["status"], "failed", "{job}");
    let (status, body) = fetched(&app, &started, &cookie).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(body["error"]["code"], "upload_not_found");
}

#[tokio::test]
async fn an_unknown_version_is_refused_before_anything_downloads() {
    let app = TestApp::new().await;
    let cookie = app.setup_owner().await;
    list_modrinth_versions(&app.modrinth, &[]).await;
    for body in [
        json!({ "source": "modrinth", "project_id": PACK }),
        json!({ "source": "modrinth", "project_id": PACK, "version_id": "nope" }),
    ] {
        let response = app.post("/api/v1/modpacks/fetch", body, Some(&cookie)).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
}

/// Real server packs hold `mods/`, `config/` and a `server.properties`, and no loader: the pack's
/// own file says which to install, and the server version replaces its files.
#[tokio::test]
async fn a_curseforge_server_pack_is_installed_on_the_loader_the_pack_names() {
    let app = TestApp::new().await;
    let cookie = app.setup_owner().await;
    set_curseforge_key(&app, &cookie).await;
    let manifest = cf_manifest(&app, &cookie, &[(CF_MOD, CF_MOD_FILES[0])]).await;
    let pack = zip_of(&[
        ("manifest.json", manifest.as_bytes()),
        ("overrides/config/for-clients.txt", b"from the pack"),
    ]);
    let server_pack = zip_of(&[
        ("Pack-Server/mods/picked-by-the-author.jar", b"a mod"),
        ("Pack-Server/config/for-servers.txt", b"from the server pack"),
        (
            "Pack-Server/server.properties",
            b"server-port=25565\nmotd=From the author\n",
        ),
        ("Pack-Server/eula.txt", b"eula=false\n"),
    ]);
    let server = &app.curseforge;
    let main = cf_file(server, CF_FILE, "pack.zip", Some(&pack), Some(CF_SERVER_FILE));
    let server_file = cf_file(
        server,
        CF_SERVER_FILE,
        "pack-server.zip",
        Some(&server_pack),
        None,
    );
    serve_curseforge_pack(server, main, Some(server_file)).await;
    serve_bytes(server, CF_FILE, "pack.zip", &pack).await;
    serve_bytes(server, CF_SERVER_FILE, "pack-server.zip", &server_pack).await;

    let versions = json_body(
        app.get(
            &format!("/api/v1/modpacks/projects/{CF_PACK}/versions?source=curseforge"),
            Some(&cookie),
        )
        .await,
    )
    .await;
    assert_eq!(
        versions.as_array().unwrap().len(),
        1,
        "the server pack isn't offered as a version of its own: {versions}"
    );

    let (started, job) = fetch(
        &app,
        json!({ "source": "curseforge", "project_id": CF_PACK.to_string() }),
        &cookie,
    )
    .await;
    assert_eq!(job["status"], "succeeded", "{job}");
    let (_, uploaded) = fetched(&app, &started, &cookie).await;
    assert_eq!(uploaded["kind"], "pack", "{uploaded}");
    assert_eq!(uploaded["server_pack"], true);
    assert_eq!(uploaded["pack"]["loader"], "fabric");

    let (id, job) = install(&app, &started["upload_id"], &cookie).await;
    assert_eq!(job["status"], "succeeded", "{job}");
    let instance = app.state.instances.registry().get(id).await.unwrap();
    let dir = instance.layout.server_dir();
    assert!(
        dir.join("mods/picked-by-the-author.jar").is_file(),
        "without its top folder"
    );
    assert!(dir.join("config/for-servers.txt").is_file());
    assert!(
        !dir.join("mods/cf-mod-1.jar").exists(),
        "the pack's list isn't used"
    );
    assert!(!dir.join("config/for-clients.txt").exists(), "nor its overrides");
    assert_settings_kept(&dir, instance.definition.ports.game, "From the author");
    let record = instance.definition.modpack.as_ref().expect("recorded");
    assert_eq!(record.source, "curseforge");
    assert_eq!(record.project_id.as_deref(), Some("7001"));
    assert_eq!(record.version_id.as_deref(), Some("7101"));

    let uploads = app.state.config.data_dir.join("cache/modpacks");
    assert_eq!(
        std::fs::read_dir(uploads).unwrap().count(),
        0,
        "nothing is left waiting"
    );
}

/// Packs often bring these. The sandbox only lets a server bind its own port, so the default one
/// in a pack's file would stop it starting.
#[tokio::test]
async fn a_pack_cannot_change_the_servers_port_or_its_eula() {
    let app = TestApp::new().await;
    let cookie = app.setup_owner().await;
    let index = json!({
        "formatVersion": 1, "game": "minecraft", "versionId": "1", "name": "Settings Pack",
        "dependencies": { "minecraft": "26.3", "fabric-loader": fabric_loader(&app, &cookie).await }, "files": [],
    })
    .to_string();
    let pack = zip_of(&[
        ("modrinth.index.json", index.as_bytes()),
        (
            "overrides/server.properties",
            b"server-port=25565\nquery.port=25565\nmotd=From the pack\n",
        ),
        ("overrides/eula.txt", b"eula=false\n"),
    ]);
    let response = app
        .put_bytes("/api/v1/modpacks/upload?file=s.mrpack", pack, Some(&cookie))
        .await;
    let uploaded = json_body(response).await;
    let (id, job) = install(&app, &uploaded["upload_id"], &cookie).await;
    assert_eq!(job["status"], "succeeded", "{job}");
    let instance = app.state.instances.registry().get(id).await.unwrap();
    assert_settings_kept(
        &instance.layout.server_dir(),
        instance.definition.ports.game,
        "From the pack",
    );
}

/// The pack's other settings arrived, but not its port or its EULA.
fn assert_settings_kept(dir: &std::path::Path, port: u16, motd: &str) {
    let properties = std::fs::read_to_string(dir.join("server.properties")).unwrap();
    assert!(
        properties.contains(&format!("server-port={port}\n")),
        "{properties}"
    );
    assert!(properties.contains(&format!("motd={motd}\n")), "{properties}");
    let eula = std::fs::read_to_string(dir.join("eula.txt")).unwrap();
    assert!(eula.contains("eula=true"), "{eula}");
}

#[tokio::test]
async fn a_server_pack_only_downloadable_by_hand_falls_back_to_the_pack() {
    let app = TestApp::new().await;
    let cookie = app.setup_owner().await;
    set_curseforge_key(&app, &cookie).await;
    let manifest = cf_manifest(&app, &cookie, &[]).await;
    let zip = zip_of(&[("manifest.json", manifest.as_bytes())]);
    let server = &app.curseforge;
    let main = cf_file(server, CF_FILE, "pack.zip", Some(&zip), Some(CF_SERVER_FILE));
    let locked = cf_file(server, CF_SERVER_FILE, "pack-server.zip", None, None);
    serve_curseforge_pack(server, main, Some(locked)).await;
    serve_bytes(server, CF_FILE, "pack.zip", &zip).await;

    let (started, job) = fetch(
        &app,
        json!({ "source": "curseforge", "project_id": CF_PACK.to_string() }),
        &cookie,
    )
    .await;
    assert_eq!(job["status"], "succeeded", "{job}");
    let (_, uploaded) = fetched(&app, &started, &cookie).await;
    assert_eq!(uploaded["kind"], "pack", "{uploaded}");
    assert_eq!(uploaded["server_pack"], false);
    assert_eq!(uploaded["pack"]["format"], "curseforge");
}

#[tokio::test]
async fn a_pack_only_downloadable_by_hand_says_where_to_get_it() {
    let app = TestApp::new().await;
    let cookie = app.setup_owner().await;
    set_curseforge_key(&app, &cookie).await;
    let main = cf_file(&app.curseforge, CF_FILE, "pack.zip", None, None);
    serve_curseforge_pack(&app.curseforge, main, None).await;

    let body = json!({ "source": "curseforge", "project_id": CF_PACK.to_string() });
    let response = app.post("/api/v1/modpacks/fetch", body, Some(&cookie)).await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let error = json_body(response).await;
    assert_eq!(error["error"]["code"], "modpack_by_hand");
    let message = error["error"]["message"].as_str().unwrap();
    assert!(
        message.contains("https://www.curseforge.com/minecraft/modpacks/cf-pack/files/7101"),
        "{message}"
    );
}

#[tokio::test]
async fn sources_without_packs_or_a_key_and_bad_ids_are_refused() {
    let app = TestApp::new().await;
    let cookie = app.setup_owner().await;

    let hangar = app
        .get("/api/v1/modpacks/search?source=hangar", Some(&cookie))
        .await;
    assert_eq!(hangar.status(), StatusCode::UNPROCESSABLE_ENTITY);

    let no_key = app
        .get("/api/v1/modpacks/search?source=curseforge", Some(&cookie))
        .await;
    assert_eq!(json_body(no_key).await["error"]["code"], "source_key_missing");

    for id in ["..%2F..%2Fetc", "nothing-here", "unknown0"] {
        let response = app
            .get(&format!("/api/v1/modpacks/uploads/{id}"), Some(&cookie))
            .await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{id}");
    }
}

#[tokio::test]
async fn only_those_who_may_create_servers_can_browse_and_fetch() {
    let app = TestApp::new().await;
    app.setup_owner().await;
    app.create_user("member", GlobalRole::Member).await;
    let member = app.login("member").await;
    for route in [
        "/api/v1/modpacks/sources",
        "/api/v1/modpacks/search",
        "/api/v1/modpacks/projects/x/versions",
        "/api/v1/modpacks/uploads/x",
    ] {
        assert_eq!(
            app.get(route, Some(&member)).await.status(),
            StatusCode::FORBIDDEN,
            "{route}"
        );
    }
    let body = json!({ "source": "modrinth", "project_id": PACK });
    let response = app.post("/api/v1/modpacks/fetch", body, Some(&member)).await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert_eq!(
        app.get("/api/v1/modpacks/sources", None).await.status(),
        StatusCode::UNAUTHORIZED
    );
}
