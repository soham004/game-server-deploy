#![allow(clippy::unwrap_used)]

use chrono::Utc;
use gsd_minecraft::{LoaderKind, settings::MinecraftServer};
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{body_partial_json, method, path, query_param},
};

use super::*;
use crate::{
    model::{Channel, DependencyKind, SideSupport},
    test_support::fixture,
};

fn target(loader: LoaderKind) -> Target {
    Target::for_server(&MinecraftServer {
        mc_version: "26.3".into(),
        loader,
        loader_version: None,
        eula_accepted_at: Utc::now(),
    })
    .unwrap()
}

async fn setup() -> (MockServer, Modrinth) {
    let server = MockServer::start().await;
    let modrinth = Modrinth::new(gsd_core::net::http_client().unwrap(), server.uri());
    (server, modrinth)
}

fn query(text: &str) -> SearchQuery {
    SearchQuery {
        text: text.into(),
        sort: SearchSort::Downloads,
        offset: 0,
        limit: 5,
    }
}

#[tokio::test]
async fn search_asks_for_server_content_that_fits_the_loader_and_version() {
    let (server, modrinth) = setup().await;
    let facets = r#"[["project_type:mod"],["categories:quilt","categories:fabric"],["versions:26.3"],["server_side:required","server_side:optional"]]"#;
    Mock::given(method("GET"))
        .and(path("/search"))
        .and(query_param("facets", facets))
        .and(query_param("index", "downloads"))
        .and(query_param("query", "lith"))
        .respond_with(ResponseTemplate::new(200).set_body_json(fixture("search_fabric_26.3.json")))
        .mount(&server)
        .await;

    let page = modrinth
        .search(&target(LoaderKind::Quilt), &query("lith"))
        .await
        .unwrap();

    assert_eq!(page.hits.len(), 5);
    assert!(page.total > 1000, "{}", page.total);
    let first = &page.hits[0];
    assert_eq!(
        (first.id.as_str(), first.slug.as_str()),
        ("P7dR8mSH", "fabric-api")
    );
    assert_eq!(first.server_side, SideSupport::Optional);
    assert!(first.icon_url.is_some() && first.updated.is_some());
}

#[tokio::test]
async fn plugin_servers_search_plugins_for_every_plugin_loader() {
    let (server, modrinth) = setup().await;
    let facets = r#"[["project_type:plugin"],["categories:paper","categories:spigot","categories:bukkit"],["versions:26.3"],["server_side:required","server_side:optional"]]"#;
    Mock::given(method("GET"))
        .and(path("/search"))
        .and(query_param("facets", facets))
        .respond_with(ResponseTemplate::new(200).set_body_json(fixture("search_paper_26.3.json")))
        .mount(&server)
        .await;

    let page = modrinth
        .search(&target(LoaderKind::Paper), &query(""))
        .await
        .unwrap();
    assert_eq!(page.hits.len(), 5);
}

#[tokio::test]
async fn versions_keep_the_primary_file_dependencies_and_only_what_fits() {
    let (server, modrinth) = setup().await;
    Mock::given(method("GET"))
        .and(path("/project/EsAfCjCV/version"))
        .and(query_param("loaders", r#"["fabric"]"#))
        .and(query_param("game_versions", r#"["26.3"]"#))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(fixture("versions_appleskin_fabric_26.3.json")),
        )
        .mount(&server)
        .await;

    let versions = modrinth
        .versions("EsAfCjCV", &target(LoaderKind::Fabric))
        .await
        .unwrap();

    let newest = &versions[0];
    assert_eq!(newest.channel, Channel::Release);
    assert!(newest.file.filename.to_ascii_lowercase().ends_with(".jar"));
    assert_eq!(newest.file.sha1.as_ref().map(String::len), Some(40));
    assert_eq!(newest.file.sha512.as_ref().map(String::len), Some(128));
    let fabric_api = &newest.dependencies[0];
    assert_eq!(
        (fabric_api.project_id.as_deref(), fabric_api.kind),
        (Some("P7dR8mSH"), DependencyKind::Required)
    );
}

#[tokio::test]
async fn files_are_identified_by_hash_and_unknown_ones_left_out() {
    let (server, modrinth) = setup().await;
    let recorded = fixture("version_files.json");
    let known: Vec<String> = recorded.as_object().unwrap().keys().cloned().collect();
    Mock::given(method("POST"))
        .and(path("/version_files"))
        .and(body_partial_json(serde_json::json!({ "algorithm": "sha1" })))
        .respond_with(ResponseTemplate::new(200).set_body_json(recorded))
        .mount(&server)
        .await;

    let mut hashes = known.clone();
    hashes.push("0".repeat(40));
    let identified = modrinth.identify(&hashes).await.unwrap();

    assert_eq!(identified.len(), known.len());
    assert!(!identified.contains_key(&"0".repeat(40)));
    assert!(
        identified
            .values()
            .any(|version| version.project_id == "gvQqBUqZ"),
        "lithium"
    );
    assert!(
        modrinth.identify(&[]).await.unwrap().is_empty(),
        "no request for nothing"
    );
}

#[tokio::test]
async fn the_update_check_sends_the_servers_loaders_and_version() {
    let (server, modrinth) = setup().await;
    let recorded = fixture("version_files_update.json");
    let sha1 = recorded["request_sha1"].as_str().unwrap().to_owned();
    Mock::given(method("POST"))
        .and(path("/version_files/update"))
        .and(body_partial_json(
            serde_json::json!({ "loaders": ["fabric"], "game_versions": ["26.3"] }),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(recorded["response"].clone()))
        .mount(&server)
        .await;

    let latest = modrinth
        .latest(std::slice::from_ref(&sha1), &target(LoaderKind::Fabric))
        .await
        .unwrap();
    assert!(latest[&sha1].game_versions.contains(&"26.3".to_owned()));
}

#[tokio::test]
async fn a_rate_limit_is_waited_out_once_and_errors_are_explained() {
    let (server, modrinth) = setup().await;
    Mock::given(method("GET"))
        .and(path("/project/lithium"))
        .respond_with(ResponseTemplate::new(429).insert_header("x-ratelimit-reset", "0"))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/project/lithium"))
        .respond_with(ResponseTemplate::new(200).set_body_json(fixture("project_lithium.json")))
        .mount(&server)
        .await;
    let lithium = modrinth.project("lithium").await.unwrap();
    assert_eq!(lithium.page_url, "https://modrinth.com/mod/lithium");

    Mock::given(method("GET"))
        .and(path("/project/retired"))
        .respond_with(ResponseTemplate::new(410))
        .mount(&server)
        .await;
    assert!(matches!(
        modrinth.project("retired").await,
        Err(SourceError::ApiRetired)
    ));
    assert!(matches!(
        modrinth.project("missing").await,
        Err(SourceError::NotFound)
    ));
    assert!(
        matches!(modrinth.project("../../etc").await, Err(SourceError::NotFound)),
        "not a path"
    );
}
