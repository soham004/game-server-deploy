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
    matchers::{method, path},
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
