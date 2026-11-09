#![allow(clippy::unwrap_used)]

use gsd_minecraft::{LoaderKind, settings::MinecraftServer};
use serde_json::json;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{method, path, query_param, query_param_is_missing},
};

use super::*;
use crate::{
    model::{Channel, DependencyKind, SideSupport, SourceKind},
    test_support::mock_hangar,
};

fn target(loader: LoaderKind) -> Target {
    Target::for_server(&MinecraftServer {
        mc_version: "26.3".into(),
        loader,
        loader_version: None,
        eula_accepted_at: chrono::Utc::now(),
    })
    .unwrap()
}

fn query(text: &str, sort: SearchSort) -> SearchQuery {
    SearchQuery {
        text: text.into(),
        sort,
        offset: 0,
        limit: 20,
    }
}

#[tokio::test]
async fn searches_plugins_for_the_servers_minecraft_version() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/projects"))
        .and(query_param("platform", "PAPER"))
        .and(query_param("version", "26.3"))
        .and(query_param("q", "chunk"))
        .and(query_param_is_missing("sort"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(crate::test_support::fixture("hangar/search_paper_26.3.json")),
        )
        .expect(1)
        .mount(&server)
        .await;
    let hangar = Hangar::new(reqwest::Client::new(), server.uri());

    let page = hangar
        .search(
            &target(LoaderKind::Purpur),
            &query("chunk", SearchSort::Relevance),
        )
        .await
        .unwrap();
    let first = &page.hits[0];
    assert_eq!(first.source, SourceKind::Hangar);
    assert!(first.id.chars().all(|c| c.is_ascii_digit()), "{first:?}");
    assert!(page.total >= u32::try_from(page.hits.len()).unwrap());
    assert!(!first.author.is_empty() && first.downloads > 0, "{first:?}");
    assert_eq!(
        (first.server_side, first.client_side),
        (SideSupport::Required, SideSupport::Unsupported)
    );
}

#[tokio::test]
async fn sorting_without_text_puts_the_most_downloaded_first() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/projects"))
        .and(query_param("sort", "-downloads"))
        .and(query_param_is_missing("q"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({ "pagination": { "count": 0 }, "result": [] })),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/projects"))
        .and(query_param("sort", "-updated"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({ "pagination": { "count": 0 }, "result": [] })),
        )
        .expect(1)
        .mount(&server)
        .await;
    let hangar = Hangar::new(reqwest::Client::new(), server.uri());
    let paper = target(LoaderKind::Paper);
    hangar
        .search(&paper, &query("", SearchSort::Relevance))
        .await
        .unwrap();
    hangar
        .search(&paper, &query("x", SearchSort::Updated))
        .await
        .unwrap();
}

#[tokio::test]
async fn mod_servers_get_nothing_without_asking() {
    // No mocks: any request would fail.
    let hangar = Hangar::new(reqwest::Client::new(), "http://127.0.0.1:9");
    let fabric = target(LoaderKind::Fabric);
    assert!(!Hangar::serves(&fabric));
    let page = hangar
        .search(&fabric, &query("", SearchSort::Relevance))
        .await
        .unwrap();
    assert_eq!((page.hits.len(), page.total), (0, 0));
    assert_eq!(hangar.versions("4", &fabric).await.unwrap(), Vec::new());
}

#[tokio::test]
async fn versions_carry_dependencies_channels_and_the_sha256() {
    let server = mock_hangar().await;
    let hangar = Hangar::new(reqwest::Client::new(), server.uri());
    let versions = hangar
        .versions("ViaBackwards", &target(LoaderKind::Paper))
        .await
        .unwrap();
    assert_eq!(versions.len(), 25);
    let newest = &versions[0];
    assert_eq!(newest.project_id, "12", "the number, though asked by name");
    assert_eq!(newest.channel, Channel::Beta, "a snapshot");
    assert!(versions.iter().any(|version| version.channel == Channel::Release));
    assert_eq!(newest.version_number, newest.name);
    assert!(newest.game_versions.contains(&"26.3".to_owned()));
    let deps: Vec<(Option<&str>, DependencyKind)> = newest
        .dependencies
        .iter()
        .map(|dependency| (dependency.project_id.as_deref(), dependency.kind))
        .collect();
    assert!(
        deps.contains(&(Some("31"), DependencyKind::Required)),
        "ViaVersion: {deps:?}"
    );
    assert!(
        deps.contains(&(Some("112"), DependencyKind::Optional)),
        "ViaRewind: {deps:?}"
    );
    let file = &newest.file;
    assert_eq!((file.sha1.as_deref(), file.sha512.as_deref()), (None, None));
    assert_eq!(file.sha256.as_ref().map(String::len), Some(64));
    assert!(
        std::path::Path::new(&file.filename)
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("jar"))
            && file.url.contains("/files/"),
        "{file:?}"
    );
}

#[tokio::test]
async fn versions_hosted_elsewhere_or_for_other_versions_are_left_out() {
    let server = MockServer::start().await;
    let version = |id: u64, url: Option<&str>, game: &str| {
        json!({
            "id": id, "projectId": 7, "name": format!("1.{id}"), "createdAt": "2026-11-01T00:00:00Z",
            "channel": { "name": "Release" },
            "pluginDependencies": { "PAPER": [{ "name": "Elsewhere", "projectId": null, "required": true, "externalUrl": "https://example.test" }] },
            "platformDependencies": { "PAPER": [game] },
            "downloads": { "PAPER": { "fileInfo": { "name": "x.jar", "sizeBytes": 1, "sha256Hash": "ab" }, "externalUrl": url.map(|_| "https://example.test"), "downloadUrl": url.map_or(json!(null), |url| json!(url)) } },
        })
    };
    let body = json!({ "pagination": { "count": 3 }, "result": [
        version(1, Some("https://hangarcdn.test/x.jar"), "26.3"),
        version(2, None, "26.3"),
        version(3, Some("https://hangarcdn.test/y.jar"), "26.2"),
    ]});
    Mock::given(method("GET"))
        .and(path("/projects/7/versions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(&server)
        .await;
    let hangar = Hangar::new(reqwest::Client::new(), server.uri());
    let versions = hangar.versions("7", &target(LoaderKind::Paper)).await.unwrap();
    let ids: Vec<&str> = versions.iter().map(|version| version.id.as_str()).collect();
    assert_eq!(ids, ["1"]);
    assert_eq!(
        versions[0].dependencies[0].project_id, None,
        "a dependency hosted elsewhere has no Hangar project"
    );
}

#[tokio::test]
async fn projects_by_number_or_name_and_errors() {
    let server = mock_hangar().await;
    let hangar = Hangar::new(reqwest::Client::new(), server.uri());
    let by_name = hangar.project("Chunky").await.unwrap();
    let by_number = hangar.project(&by_name.id).await.unwrap();
    assert_eq!(by_name, by_number);
    assert_eq!(by_name.source, SourceKind::Hangar);
    assert_eq!(by_name.page_url, "https://hangar.papermc.io/pop4959/Chunky");
    // The project endpoint sends no page text (the browse list shows the description).
    assert!(
        by_name.icon_url.is_some() && !by_name.description.is_empty(),
        "{by_name:?}"
    );

    assert!(matches!(
        hangar.project("NoSuchPlugin").await,
        Err(SourceError::NotFound)
    ));
    assert!(
        matches!(hangar.project("../admin").await, Err(SourceError::NotFound)),
        "never sent"
    );
}
