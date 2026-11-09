#![allow(clippy::unwrap_used)]

use gsd_minecraft::{LoaderKind, settings::MinecraftServer};
use serde_json::json;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{header, method, path, query_param, query_param_is_missing},
};

use super::*;
use crate::{
    fingerprint::fingerprint,
    model::{Channel, DependencyKind, SearchSort, SideSupport, SourceKind},
    test_support::{
        CF_BLOCKED, CF_LIBRARY, CF_MOD, CF_MOD_FILES, CF_PLUGIN, CURSEFORGE_KEY, curseforge_blocked_jar,
        fake_jar, mock_curseforge,
    },
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

fn with_key(server: &MockServer) -> CurseForge {
    let curseforge = CurseForge::new(reqwest::Client::new(), server.uri());
    curseforge.set_key(Some(CURSEFORGE_KEY)).unwrap();
    curseforge
}

#[tokio::test]
async fn without_a_key_nothing_is_sent() {
    // Nothing listens there: a request would be a different error.
    let curseforge = CurseForge::new(reqwest::Client::new(), "http://127.0.0.1:9");
    assert!(!curseforge.has_key());
    let fabric = target(LoaderKind::Fabric);
    assert!(matches!(
        curseforge
            .search(&fabric, &query("", SearchSort::Relevance))
            .await,
        Err(SourceError::NoKey)
    ));
    assert!(matches!(curseforge.project("1").await, Err(SourceError::NoKey)));
    assert!(matches!(
        curseforge.versions("1", &fabric).await,
        Err(SourceError::NoKey)
    ));
    assert!(matches!(curseforge.identify(&[1]).await, Err(SourceError::NoKey)));
}

#[tokio::test]
async fn a_wrong_key_is_reported_as_such_and_can_be_checked_first() {
    let server = mock_curseforge().await;
    let curseforge = CurseForge::new(reqwest::Client::new(), server.uri());
    curseforge.set_key(Some("wrong")).unwrap();
    assert!(matches!(
        curseforge.project("1002").await,
        Err(SourceError::KeyRefused)
    ));

    assert!(matches!(
        curseforge.check_key("wrong").await,
        Err(SourceError::KeyRefused)
    ));
    curseforge.check_key(CURSEFORGE_KEY).await.unwrap();
    assert!(
        matches!(curseforge.project("1002").await, Err(SourceError::KeyRefused)),
        "checking a key doesn't start using it"
    );
    curseforge.set_key(None).unwrap();
    assert!(!curseforge.has_key());
}

#[test]
fn the_key_never_shows_in_debug_output() {
    let curseforge = CurseForge::new(reqwest::Client::new(), "http://127.0.0.1:9");
    curseforge.set_key(Some("very-secret-value")).unwrap();
    let printed = format!("{curseforge:?}");
    assert!(!printed.contains("very-secret-value"), "{printed}");
    assert!(printed.contains("has_key: true"), "{printed}");
    assert!(
        curseforge.set_key(Some("line\nbreak")).is_err(),
        "a key that can't be a header is refused"
    );
    assert!(curseforge.has_key(), "and the old one stays");
}

#[tokio::test]
async fn searches_fit_the_servers_class_version_and_loader() {
    let server = MockServer::start().await;
    let empty = json!({ "data": [], "pagination": { "totalCount": 0 } });
    Mock::given(method("GET"))
        .and(path("/mods/search"))
        .and(header("x-api-key", CURSEFORGE_KEY))
        .and(query_param("gameId", "432"))
        .and(query_param("classId", "6"))
        .and(query_param("gameVersion", "26.3"))
        .and(query_param("modLoaderType", "4"))
        .and(query_param("searchFilter", "map"))
        .and(query_param_is_missing("sortField"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&empty))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/mods/search"))
        .and(query_param("modLoaderTypes", "[5,4]"))
        .and(query_param("sortField", "6"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&empty))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/mods/search"))
        .and(query_param("classId", "5"))
        .and(query_param_is_missing("modLoaderType"))
        .and(query_param("sortField", "2"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&empty))
        .expect(1)
        .mount(&server)
        .await;
    let curseforge = with_key(&server);
    curseforge
        .search(&target(LoaderKind::Fabric), &query("map", SearchSort::Relevance))
        .await
        .unwrap();
    curseforge
        .search(&target(LoaderKind::Quilt), &query("", SearchSort::Downloads))
        .await
        .unwrap();
    curseforge
        .search(&target(LoaderKind::Paper), &query("", SearchSort::Relevance))
        .await
        .unwrap();
}

#[tokio::test]
async fn search_results_map_onto_the_model() {
    let server = mock_curseforge().await;
    let curseforge = with_key(&server);
    let page = curseforge
        .search(&target(LoaderKind::Paper), &query("", SearchSort::Relevance))
        .await
        .unwrap();
    assert_eq!(page.total, 1);
    let hit = &page.hits[0];
    assert_eq!(hit.source, SourceKind::CurseForge);
    assert_eq!(hit.id, CF_PLUGIN.to_string());
    assert_eq!(
        hit.page_url,
        "https://www.curseforge.com/minecraft/bukkit-plugins/cf-plugin"
    );
    assert_eq!((hit.author.as_str(), hit.downloads), ("mock author", 1234));
    assert_eq!(
        hit.icon_url.as_deref(),
        Some("https://media.forgecdn.net/thumb.png")
    );
    assert_eq!(
        (hit.server_side, hit.client_side),
        (SideSupport::Unknown, SideSupport::Unknown)
    );
}

#[tokio::test]
async fn versions_are_the_fitting_files_newest_first_with_sha1_and_dependencies() {
    let server = mock_curseforge().await;
    let curseforge = with_key(&server);
    let versions = curseforge
        .versions(&CF_MOD.to_string(), &target(LoaderKind::Fabric))
        .await
        .unwrap();
    let ids: Vec<String> = versions.iter().map(|version| version.id.clone()).collect();
    assert_eq!(
        ids,
        CF_MOD_FILES.map(|id| id.to_string()),
        "newest first; the Forge file is left out"
    );
    let newest = &versions[0];
    assert_eq!(newest.channel, Channel::Release);
    assert_eq!(newest.project_id, CF_MOD.to_string());
    assert_eq!(
        newest.game_versions,
        ["26.3"],
        "loaders and sides aren't versions"
    );
    assert_eq!(newest.loaders, ["fabric"]);
    assert_eq!(newest.file.sha1.as_ref().map(String::len), Some(40));
    assert_eq!(newest.file.manual_url, None);
    assert!(newest.file.url.ends_with("/cf-mod-2.jar"), "{:?}", newest.file);
    assert_eq!(newest.dependencies[0].project_id, Some(CF_LIBRARY.to_string()));
    assert_eq!(newest.dependencies[0].kind, DependencyKind::Required);
    assert_eq!(newest.dependencies[1].kind, DependencyKind::Optional);

    let quilt = curseforge
        .versions(&CF_MOD.to_string(), &target(LoaderKind::Quilt))
        .await
        .unwrap();
    assert_eq!(quilt.len(), 2, "Quilt runs Fabric mods");
    let plugin = curseforge
        .versions(&CF_PLUGIN.to_string(), &target(LoaderKind::Paper))
        .await
        .unwrap();
    assert_eq!(plugin[0].loaders, ["bukkit"], "a plugin names no loader");
    let wrong_kind = curseforge
        .versions(&CF_MOD.to_string(), &target(LoaderKind::Paper))
        .await
        .unwrap();
    assert_eq!(wrong_kind, [], "a mod's files don't fit a plugin server");
}

#[tokio::test]
async fn release_types_map_and_server_packs_are_left_out() {
    let server = MockServer::start().await;
    let file = |id: u64, release: u8, pack: bool| {
        json!({
            "id": id, "modId": 7, "displayName": format!("v{id}"), "fileName": format!("v{id}.jar"),
            "releaseType": release, "fileDate": format!("2026-10-{id:02}T00:00:00Z"),
            "downloadUrl": "https://example.test/x.jar", "gameVersions": ["26.3", "Fabric"],
            "isServerPack": pack,
        })
    };
    Mock::given(method("GET"))
        .and(path("/mods/7/files"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "data": [
            file(1, 1, false), file(2, 2, false), file(3, 3, false), file(4, 1, true),
        ]})))
        .mount(&server)
        .await;
    let versions = with_key(&server)
        .versions("7", &target(LoaderKind::Fabric))
        .await
        .unwrap();
    let channels: Vec<Channel> = versions.iter().map(|version| version.channel).collect();
    assert_eq!(channels, [Channel::Alpha, Channel::Beta, Channel::Release]);
}

#[tokio::test]
async fn a_file_its_author_blocks_says_where_to_get_it_by_hand() {
    let server = mock_curseforge().await;
    let curseforge = with_key(&server);
    let versions = curseforge
        .versions(&CF_BLOCKED.to_string(), &target(LoaderKind::Fabric))
        .await
        .unwrap();
    let file = &versions[0].file;
    assert_eq!(file.url, "");
    assert_eq!(
        file.manual_url.as_deref(),
        Some("https://www.curseforge.com/minecraft/mc-mods/cf-blocked/files/5201")
    );
}

#[tokio::test]
async fn files_are_recognised_by_fingerprint_and_unknown_ones_left_out() {
    let server = mock_curseforge().await;
    let curseforge = with_key(&server);
    let known = fingerprint(&curseforge_blocked_jar());
    let unknown = fingerprint(&fake_jar("other", "Other", "1", false));
    let found = curseforge.identify(&[known, unknown]).await.unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[&known].project_id, CF_BLOCKED.to_string());
    assert_eq!(curseforge.identify(&[]).await.unwrap().len(), 0);
}

#[tokio::test]
async fn projects_by_id_and_errors() {
    let server = mock_curseforge().await;
    let curseforge = with_key(&server);
    let project = curseforge.project(&CF_MOD.to_string()).await.unwrap();
    assert_eq!(project.source, SourceKind::CurseForge);
    assert_eq!(project.title, "CF Mod");
    assert!(
        matches!(curseforge.project("cf-mod").await, Err(SourceError::NotFound)),
        "ids are numbers"
    );

    let projects = curseforge
        .projects(&[CF_MOD.to_string(), "424242".into(), "not-a-number".into()])
        .await
        .unwrap();
    assert_eq!(projects.len(), 1, "unknown ones are left out");
}
