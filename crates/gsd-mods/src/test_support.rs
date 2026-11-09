//! A Modrinth stand-in for tests (here and in the app's API tests): the recorded responses
//! (`tests/fixtures/record.mjs`), with every version's file replaced by a small generated jar
//! whose descriptor names the project, and the hashes and URLs rewritten to match. Downloads are
//! therefore checked for real.

#![allow(clippy::unwrap_used, clippy::missing_panics_doc)]

use std::{collections::HashMap, io::Write, path::PathBuf, sync::Arc};

use serde_json::{Value, json};
use sha1::Sha1;
use sha2::{Digest, Sha512};
use wiremock::{
    Mock, MockServer, Request, Respond, ResponseTemplate,
    matchers::{header, method, path},
};

/// The recorded projects: slug, and the loader their versions were recorded for.
pub const PROJECTS: [(&str, &str); 5] = [
    ("appleskin", "fabric"),
    ("architectury-api", "fabric"),
    ("fabric-api", "fabric"),
    ("lithium", "fabric"),
    ("chunky", "paper"),
];

pub fn fixture_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

pub fn fixture(name: &str) -> Value {
    serde_json::from_str(&std::fs::read_to_string(fixture_path(name)).unwrap()).unwrap()
}

/// The SHA-1 sources publish for a file with these contents.
pub fn sha1_hex(bytes: &[u8]) -> String {
    hex::encode(Sha1::digest(bytes))
}

/// A jar whose descriptor says it is `slug` at `version` (a plugin descriptor for plugins).
pub fn fake_jar(slug: &str, title: &str, version: &str, plugin: bool) -> Vec<u8> {
    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default();
    if plugin {
        zip.start_file("plugin.yml", options).unwrap();
        write!(zip, "name: {title}\nversion: {version}\nmain: example.Main\n").unwrap();
    } else {
        zip.start_file("fabric.mod.json", options).unwrap();
        let descriptor =
            json!({ "schemaVersion": 1, "id": slug.replace('-', "_"), "name": title, "version": version });
        zip.write_all(descriptor.to_string().as_bytes()).unwrap();
    }
    zip.finish().unwrap().into_inner()
}

/// A mock Modrinth serving the recorded projects, their versions and their (fake) files.
pub async fn mock_modrinth() -> MockServer {
    let server = MockServer::start().await;
    for search in ["search_fabric_26.3.json", "search_paper_26.3.json"] {
        let facet = if search.contains("paper") {
            "project_type:plugin"
        } else {
            "project_type:mod"
        };
        Mock::given(method("GET"))
            .and(path("/search"))
            .and(wiremock::matchers::query_param_contains("facets", facet))
            .respond_with(ResponseTemplate::new(200).set_body_json(fixture(search)))
            .mount(&server)
            .await;
    }
    let mut projects = Vec::new();
    let mut by_sha1 = HashMap::new();
    for (slug, loader) in PROJECTS {
        let project = fixture(&format!("project_{slug}.json"));
        projects.push(project.clone());
        let id = project["id"].as_str().unwrap().to_owned();
        let title = project["title"].as_str().unwrap().to_owned();
        let mut versions = fixture(&format!("versions_{slug}_{loader}_26.3.json"));
        for version in versions.as_array_mut().unwrap() {
            let jar = fake_jar(
                slug,
                &title,
                version["version_number"].as_str().unwrap(),
                loader == "paper",
            );
            serve_file(&server, version, jar).await;
        }
        // Newest first, as Modrinth lists them: an update check answers with the first.
        let newest = versions[0].clone();
        for version in versions.as_array().unwrap() {
            let sha1 = version["files"][0]["hashes"]["sha1"].as_str().unwrap().to_owned();
            by_sha1.insert(sha1, (version.clone(), newest.clone()));
        }
        for key in [&id, &slug.to_owned()] {
            Mock::given(method("GET"))
                .and(path(format!("/project/{key}")))
                .respond_with(ResponseTemplate::new(200).set_body_json(project.clone()))
                .mount(&server)
                .await;
            Mock::given(method("GET"))
                .and(path(format!("/project/{key}/version")))
                .respond_with(ResponseTemplate::new(200).set_body_json(versions.clone()))
                .mount(&server)
                .await;
        }
    }
    Mock::given(method("GET"))
        .and(path("/projects"))
        .respond_with(ResponseTemplate::new(200).set_body_json(Value::Array(projects)))
        .mount(&server)
        .await;
    let hashes = Arc::new(by_sha1);
    Mock::given(method("POST"))
        .and(path("/version_files"))
        .respond_with(ByHash {
            hashes: Arc::clone(&hashes),
            newest: false,
        })
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/version_files/update"))
        .respond_with(ByHash { hashes, newest: true })
        .mount(&server)
        .await;
    server
}

/// Answers Modrinth's hash lookups for the generated jars: the version a file is, or (for the
/// update check) the newest version of its project. Unknown hashes are left out, as Modrinth does.
struct ByHash {
    hashes: Arc<HashMap<String, (Value, Value)>>,
    newest: bool,
}

impl Respond for ByHash {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let body: Value = serde_json::from_slice(&request.body).unwrap_or_default();
        let found: serde_json::Map<String, Value> = body["hashes"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|hash| {
                let hash = hash.as_str()?;
                let (own, newest) = self.hashes.get(hash)?;
                Some((
                    hash.to_owned(),
                    if self.newest { newest.clone() } else { own.clone() },
                ))
            })
            .collect();
        ResponseTemplate::new(200).set_body_json(Value::Object(found))
    }
}

/// Points the version's file at `jar`, served by `server`, with matching hashes.
async fn serve_file(server: &MockServer, version: &mut Value, jar: Vec<u8>) {
    let id = version["id"].as_str().unwrap().to_owned();
    let file = &mut version["files"][0];
    let route = format!("/files/{id}/{}", file["filename"].as_str().unwrap());
    file["url"] = format!("{}{route}", server.uri()).into();
    file["size"] = jar.len().into();
    file["hashes"] =
        json!({ "sha1": hex::encode(Sha1::digest(&jar)), "sha512": hex::encode(Sha512::digest(&jar)) });
    Mock::given(method("GET"))
        .and(path(route))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(jar))
        .mount(server)
        .await;
}

/// The key the mock `CurseForge` accepts; any other is refused, as `CurseForge` does.
pub const CURSEFORGE_KEY: &str = "test-curseforge-key";
/// Mock `CurseForge` projects: a mod needing a library, a mod whose author blocks other apps
/// from downloading it, and a Bukkit plugin. Made up from the documented schema: there is no key
/// here to record real answers with (`tests/e2e/curseforge.e2e.mjs` checks the real API).
pub const CF_MOD: u64 = 1002;
pub const CF_LIBRARY: u64 = 1001;
pub const CF_BLOCKED: u64 = 1003;
pub const CF_PLUGIN: u64 = 2001;
/// The files of [`CF_MOD`] that fit a Fabric server on 26.3, newest first.
pub const CF_MOD_FILES: [u64; 2] = [5002, 5001];

/// The blocked mod's jar, which the mock doesn't serve: the user brings it.
pub fn curseforge_blocked_jar() -> Vec<u8> {
    fake_jar("cf-blocked", "CF Blocked", "3.0", false)
}

/// A mock `CurseForge`. Every route wants [`CURSEFORGE_KEY`].
pub async fn mock_curseforge() -> MockServer {
    let server = MockServer::start().await;
    let mods = [
        cf_mod(CF_LIBRARY, "CF Library", 6),
        cf_mod(CF_MOD, "CF Mod", 6),
        cf_mod(CF_BLOCKED, "CF Blocked", 6),
        cf_mod(CF_PLUGIN, "CF Plugin", 5),
    ];
    let mut by_fingerprint = HashMap::new();
    let mut by_id = HashMap::new();
    for project in &mods {
        let mut files = Vec::new();
        for file in cf_files()
            .into_iter()
            .filter(|file| project["id"] == file.project)
        {
            files.push(cf_file(&server, file).await);
        }
        let id = &project["id"];
        keyed(&server, "GET", &format!("/mods/{id}"), json!({ "data": project })).await;
        keyed(
            &server,
            "GET",
            &format!("/mods/{id}/files"),
            json!({ "data": files }),
        )
        .await;
        for file in files {
            by_id.insert(file["id"].as_u64().unwrap(), file.clone());
            by_fingerprint.insert(file["fileFingerprint"].as_u64().unwrap(), file);
        }
    }
    for class in [6, 5] {
        let hits: Vec<&Value> = mods
            .iter()
            .filter(|project| project["classId"] == class)
            .collect();
        Mock::given(method("GET"))
            .and(path("/mods/search"))
            .and(header(KEY_HEADER, CURSEFORGE_KEY))
            .and(wiremock::matchers::query_param("classId", class.to_string()))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": hits,
                "pagination": { "index": 0, "pageSize": 20, "resultCount": hits.len(), "totalCount": hits.len() },
            })))
            .mount(&server)
            .await;
    }
    Mock::given(method("POST"))
        .and(path("/fingerprints/432"))
        .and(header(KEY_HEADER, CURSEFORGE_KEY))
        .respond_with(ByFingerprint(by_fingerprint))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/mods"))
        .and(header(KEY_HEADER, CURSEFORGE_KEY))
        .respond_with(ModsById(mods.to_vec()))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/mods/files"))
        .and(header(KEY_HEADER, CURSEFORGE_KEY))
        .respond_with(FilesById(by_id))
        .mount(&server)
        .await;
    keyed(&server, "GET", "/games/432", json!({ "data": { "id": 432 } })).await;
    // Tried last: anything without the right key.
    Mock::given(wiremock::matchers::any())
        .respond_with(ResponseTemplate::new(403))
        .with_priority(u8::MAX)
        .mount(&server)
        .await;
    server
}

const KEY_HEADER: &str = "x-api-key";

/// A route that answers `body` to whoever has the key.
async fn keyed(server: &MockServer, verb: &str, route: &str, body: Value) {
    Mock::given(method(verb))
        .and(path(route))
        .and(header(KEY_HEADER, CURSEFORGE_KEY))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(server)
        .await;
}

fn cf_mod(id: u64, name: &str, class: u64) -> Value {
    let slug = name.to_lowercase().replace(' ', "-");
    let section = if class == 5 { "bukkit-plugins" } else { "mc-mods" };
    json!({
        "id": id, "gameId": 432, "name": name, "slug": slug, "classId": class,
        "links": { "websiteUrl": format!("https://www.curseforge.com/minecraft/{section}/{slug}") },
        "summary": format!("{name} from the mock"), "status": 4, "downloadCount": 1234.0,
        "categories": [{ "id": 1, "name": "Utility" }],
        "authors": [{ "id": 1, "name": "mock author", "url": "https://example.test" }],
        "logo": { "id": 1, "modId": id, "thumbnailUrl": "https://media.forgecdn.net/thumb.png", "url": "https://media.forgecdn.net/full.png" },
        "mainFileId": 1, "latestFiles": [], "latestFilesIndexes": [],
        "dateModified": "2026-11-01T00:00:00Z", "allowModDistribution": true, "isAvailable": true,
    })
}

/// One file of the mock.
struct CfFile {
    id: u64,
    project: u64,
    name: &'static str,
    date: &'static str,
    game_versions: &'static [&'static str],
    needs_library: bool,
    /// What it contains (empty: no bytes, and a made-up fingerprint).
    contents: Vec<u8>,
    /// The author only allows downloading by hand.
    blocked: bool,
}

/// Every file of the mock. Oldest first, on purpose: the client orders them itself.
fn cf_files() -> Vec<CfFile> {
    const FABRIC: &[&str] = &["26.3", "Fabric", "Server"];
    let file = |id, project, name, date, game_versions, contents| CfFile {
        id,
        project,
        name,
        date,
        game_versions,
        needs_library: false,
        contents,
        blocked: false,
    };
    let cf_mod_file = |id, name, date, version| CfFile {
        needs_library: true,
        ..file(
            id,
            CF_MOD,
            name,
            date,
            FABRIC,
            fake_jar("cf-mod", "CF Mod", version, false),
        )
    };
    vec![
        cf_mod_file(5001, "cf-mod-1.jar", "2026-10-01", "1.0"),
        file(
            5009,
            CF_MOD,
            "cf-mod-forge.jar",
            "2026-10-02",
            &["26.3", "Forge"],
            Vec::new(),
        ),
        cf_mod_file(5002, "cf-mod-2.jar", "2026-10-03", "2.0"),
        file(
            5101,
            CF_LIBRARY,
            "cf-library.jar",
            "2026-09-01",
            FABRIC,
            fake_jar("cf-library", "CF Library", "1.0", false),
        ),
        CfFile {
            blocked: true,
            ..file(
                5201,
                CF_BLOCKED,
                "cf-blocked.jar",
                "2026-09-02",
                FABRIC,
                curseforge_blocked_jar(),
            )
        },
        file(
            5301,
            CF_PLUGIN,
            "cf-plugin.jar",
            "2026-09-03",
            &["26.3"],
            fake_jar("cf-plugin", "CF Plugin", "1.0", true),
        ),
    ]
}

/// The file as `CurseForge` lists it, served by `server` unless its author blocks that.
async fn cf_file(server: &MockServer, file: CfFile) -> Value {
    let route = format!("/files/{}/{}", file.id, file.name);
    let download_url = if file.blocked {
        Value::Null
    } else {
        Mock::given(method("GET"))
            .and(path(route.clone()))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(file.contents.clone()))
            .mount(server)
            .await;
        format!("{}{route}", server.uri()).into()
    };
    let dependencies = if file.needs_library {
        json!([{ "modId": CF_LIBRARY, "relationType": 3 }, { "modId": 9999, "relationType": 2 }])
    } else {
        json!([])
    };
    let fingerprint = if file.contents.is_empty() {
        u64::from(u32::MAX) - file.id
    } else {
        u64::from(crate::fingerprint::fingerprint(&file.contents))
    };
    json!({
        "id": file.id, "gameId": 432, "modId": file.project, "isAvailable": true,
        "displayName": file.name.trim_end_matches(".jar"), "fileName": file.name, "releaseType": 1,
        "fileStatus": 4,
        "hashes": [{ "value": hex::encode(Sha1::digest(&file.contents)), "algo": 1 }, { "value": "00", "algo": 2 }],
        "fileDate": format!("{}T00:00:00Z", file.date), "fileLength": file.contents.len(),
        "downloadUrl": download_url, "gameVersions": file.game_versions,
        "dependencies": dependencies, "isServerPack": false, "fileFingerprint": fingerprint,
    })
}

/// `POST /fingerprints/432`: unknown fingerprints are left out.
struct ByFingerprint(HashMap<u64, Value>);

impl Respond for ByFingerprint {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let body: Value = serde_json::from_slice(&request.body).unwrap_or_default();
        let matches: Vec<Value> = body["fingerprints"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|fingerprint| self.0.get(&fingerprint.as_u64()?))
            .map(|file| json!({ "id": file["modId"], "file": file, "latestFiles": [] }))
            .collect();
        ResponseTemplate::new(200)
            .set_body_json(json!({ "data": { "isCacheBuilt": true, "exactMatches": matches } }))
    }
}

/// `POST /mods/files`: unknown ids are left out.
struct FilesById(HashMap<u64, Value>);

impl Respond for FilesById {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let body: Value = serde_json::from_slice(&request.body).unwrap_or_default();
        let found: Vec<&Value> = body["fileIds"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|id| self.0.get(&id.as_u64()?))
            .collect();
        ResponseTemplate::new(200).set_body_json(json!({ "data": found }))
    }
}

/// `POST /mods`: unknown ids are left out.
struct ModsById(Vec<Value>);

impl Respond for ModsById {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let body: Value = serde_json::from_slice(&request.body).unwrap_or_default();
        let wanted = body["modIds"].as_array().cloned().unwrap_or_default();
        let found: Vec<&Value> = self.0.iter().filter(|m| wanted.contains(&m["id"])).collect();
        ResponseTemplate::new(200).set_body_json(json!({ "data": found }))
    }
}

/// The recorded Hangar projects (by name): `ViaBackwards` requires `ViaVersion`; `Chunky` needs nothing.
pub const HANGAR_PROJECTS: [&str; 3] = ["Chunky", "ViaBackwards", "ViaVersion"];

/// A mock Hangar serving the recorded search, projects and versions, by number and by name, with
/// every version's Paper file replaced by a generated plugin jar (and its SHA-256).
pub async fn mock_hangar() -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/projects"))
        .respond_with(ResponseTemplate::new(200).set_body_json(fixture("hangar/search_paper_26.3.json")))
        .mount(&server)
        .await;
    for name in HANGAR_PROJECTS {
        let project = fixture(&format!("hangar/project_{name}.json"));
        let mut versions = fixture(&format!("hangar/versions_{name}_26.3.json"));
        for version in versions["result"].as_array_mut().unwrap() {
            let jar = fake_jar(
                &name.to_lowercase(),
                name,
                version["name"].as_str().unwrap(),
                true,
            );
            serve_hangar_file(&server, version, jar).await;
        }
        let id = project["id"].to_string();
        for key in [id.as_str(), name] {
            Mock::given(method("GET"))
                .and(path(format!("/projects/{key}")))
                .respond_with(ResponseTemplate::new(200).set_body_json(project.clone()))
                .mount(&server)
                .await;
            Mock::given(method("GET"))
                .and(path(format!("/projects/{key}/versions")))
                .respond_with(ResponseTemplate::new(200).set_body_json(versions.clone()))
                .mount(&server)
                .await;
        }
    }
    server
}

/// Points the version's Paper download at `jar`, served by `server`, with its real SHA-256.
async fn serve_hangar_file(server: &MockServer, version: &mut Value, jar: Vec<u8>) {
    let file_name = version["downloads"]["PAPER"]["fileInfo"]["name"]
        .as_str()
        .unwrap()
        .to_owned();
    let route = format!("/files/{}/{file_name}", version["id"]);
    let download = &mut version["downloads"]["PAPER"];
    download["downloadUrl"] = format!("{}{route}", server.uri()).into();
    download["fileInfo"]["sizeBytes"] = jar.len().into();
    download["fileInfo"]["sha256Hash"] = hex::encode(sha2::Sha256::digest(&jar)).into();
    Mock::given(method("GET"))
        .and(path(route))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(jar))
        .mount(server)
        .await;
}
