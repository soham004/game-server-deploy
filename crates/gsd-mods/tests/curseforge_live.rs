//! The `CurseForge` client against the real API, which its mocks are only a reading of: search,
//! projects, files, dependencies, and whether our file fingerprint is the one `CurseForge` computes.
//!
//! The key comes from `CURSEFORGE_API_KEY`, or else from the `.env` file at the repository root
//! (which git and Docker ignore: the key is for testing and must never ship). Without a key these
//! tests pass without doing anything, as on forks, whose CI gets no secrets; `CURSEFORGE_REQUIRED`
//! makes that a failure, so CI on this repository can't go quiet if the secret disappears. Nothing
//! here prints the key.
//!
//! They use an older Minecraft version with a settled set of mods, so what they find doesn't change
//! from week to week.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;

use gsd_core::net::http_client;
use gsd_minecraft::{LoaderKind, settings::MinecraftServer};
use gsd_mods::{
    SourceError, Target,
    curseforge::{CurseForge, DEFAULT_API},
    fingerprint::fingerprint,
    model::{Channel, DependencyKind, SearchPage, SearchQuery, SearchSort, SourceKind},
};
use sha1::{Digest, Sha1};

const MC_VERSION: &str = "1.20.1";
/// Fabric API, which most Fabric mods need.
const FABRIC_API: &str = "306612";

/// The value of `name` in `.env`-style text: `export` allowed, surrounding quotes removed, and
/// nothing expanded (a key can contain `$`).
fn value_in(text: &str, name: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let line = line.trim().trim_start_matches('\u{feff}');
        let line = line.strip_prefix("export ").unwrap_or(line);
        let (key, value) = line.split_once('=')?;
        if line.starts_with('#') || key.trim() != name {
            return None;
        }
        let value = value.trim();
        let unquoted = ['"', '\''].iter().find_map(|quote| {
            value
                .strip_prefix(*quote)
                .and_then(|rest| rest.strip_suffix(*quote))
        });
        Some(unquoted.unwrap_or(value).to_owned())
    })
}

/// The key, if there is one. An empty one counts as none.
fn key() -> Option<String> {
    let from_environment = std::env::var("CURSEFORGE_API_KEY")
        .ok()
        .filter(|key| !key.is_empty());
    from_environment.or_else(|| {
        let dot_env = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.env");
        let text = std::fs::read_to_string(dot_env).ok()?;
        value_in(&text, "CURSEFORGE_API_KEY").filter(|key| !key.is_empty())
    })
}

/// A client with the key, or `None` when these tests can't run.
fn client() -> Option<CurseForge> {
    let Some(key) = key() else {
        assert!(
            std::env::var_os("CURSEFORGE_REQUIRED").is_none_or(|required| required.is_empty()),
            "CURSEFORGE_REQUIRED is set, but there is no key"
        );
        eprintln!("no CurseForge key: skipped");
        return None;
    };
    let curseforge = CurseForge::new(http_client().unwrap(), DEFAULT_API);
    curseforge.set_key(Some(&key)).expect("a usable key");
    Some(curseforge)
}

fn target(loader: LoaderKind) -> Target {
    Target::for_server(&MinecraftServer {
        mc_version: MC_VERSION.into(),
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
        limit: 10,
    }
}

fn assert_found(page: &SearchPage, what: &str) {
    assert!(!page.hits.is_empty() && page.total > 0, "{what}: {page:?}");
    for hit in &page.hits {
        assert_eq!(hit.source, SourceKind::CurseForge);
        assert!(hit.id.parse::<u64>().is_ok(), "{hit:?}");
        assert!(
            hit.page_url.starts_with("https://www.curseforge.com/minecraft/"),
            "{hit:?}"
        );
    }
}

/// Packs are listed under their own class; their files are offered without the server versions, which
/// are reached from the pack's file.
#[tokio::test]
async fn modpacks_are_found_with_their_files_and_server_packs() {
    let Some(curseforge) = client() else { return };
    let page = curseforge
        .search_packs(&query("", SearchSort::Downloads), Some(MC_VERSION))
        .await
        .unwrap();
    assert_found(&page, "modpacks");
    assert!(
        page.hits
            .iter()
            .all(|hit| hit.page_url.contains("/minecraft/modpacks/")),
        "only modpacks: {page:?}"
    );

    let mut server_packs = 0;
    for hit in &page.hits {
        let versions = curseforge.pack_versions(&hit.id).await.unwrap();
        let newest = versions
            .first()
            .unwrap_or_else(|| panic!("{} has files", hit.title));
        let name = newest.file.filename.to_ascii_lowercase();
        assert!(
            std::path::Path::new(&name)
                .extension()
                .is_some_and(|extension| extension == "zip"),
            "{}: {name}",
            hit.title
        );
        if let Some(server_pack) = curseforge.server_pack(&hit.id, &newest.id).await.unwrap() {
            assert_eq!(server_pack.project_id, hit.id);
            assert_ne!(server_pack.id, newest.id);
            server_packs += 1;
        }
    }
    eprintln!("{server_packs} of {} packs have a server pack", page.hits.len());
    assert!(server_packs > 0, "the biggest packs usually have one");
}

#[tokio::test]
async fn the_key_is_accepted_and_a_made_up_one_refused() {
    let Some(curseforge) = client() else { return };
    let made_up = curseforge.check_key("not-a-real-key").await;
    assert!(matches!(made_up, Err(SourceError::KeyRefused)), "{made_up:?}");
    let own = key().expect("client() found one");
    curseforge.check_key(&own).await.unwrap();
}

#[tokio::test]
async fn searches_find_what_fits_each_kind_of_server() {
    let Some(curseforge) = client() else { return };
    let fabric = target(LoaderKind::Fabric);

    let by_text = curseforge
        .search(&fabric, &query("fabric api", SearchSort::Relevance))
        .await
        .unwrap();
    assert_found(&by_text, "by text");
    assert!(by_text.hits.iter().any(|hit| hit.id == FABRIC_API), "{by_text:?}");

    for sort in [
        SearchSort::Relevance,
        SearchSort::Downloads,
        SearchSort::Updated,
        SearchSort::Newest,
    ] {
        let page = curseforge.search(&fabric, &query("", sort)).await.unwrap();
        assert_found(&page, "each sort");
    }
    let most_downloaded = curseforge
        .search(&fabric, &query("", SearchSort::Downloads))
        .await
        .unwrap();
    // CurseForge's order lags behind its live counts, so neighbours can be slightly out of order.
    let counts: Vec<u64> = most_downloaded.hits.iter().map(|hit| hit.downloads).collect();
    assert!(
        counts.first() > counts.last() && counts.iter().all(|count| *count > 1_000_000),
        "sort field 6 is total downloads: {counts:?}"
    );

    // Quilt asks for two loaders at once; Quilt runs Fabric mods, so Fabric API must show.
    let quilt = curseforge
        .search(
            &target(LoaderKind::Quilt),
            &query("fabric api", SearchSort::Relevance),
        )
        .await
        .unwrap();
    assert!(quilt.hits.iter().any(|hit| hit.id == FABRIC_API), "{quilt:?}");

    let plugins = curseforge
        .search(&target(LoaderKind::Paper), &query("", SearchSort::Downloads))
        .await
        .unwrap();
    assert_found(&plugins, "Bukkit plugins");
    assert!(
        plugins
            .hits
            .iter()
            .all(|hit| hit.page_url.contains("/bukkit-plugins/")),
        "{plugins:?}"
    );
}

#[tokio::test]
async fn projects_and_their_files_map_onto_the_model() {
    let Some(curseforge) = client() else { return };
    let project = curseforge.project(FABRIC_API).await.unwrap();
    assert_eq!(project.source, SourceKind::CurseForge);
    assert_eq!(project.id, FABRIC_API);
    assert!(project.title.contains("Fabric"), "{project:?}");
    assert!(project.page_url.ends_with("/fabric-api"), "{project:?}");
    let missing = curseforge.project("999999999").await;
    assert!(matches!(missing, Err(SourceError::NotFound)), "{missing:?}");

    let several = curseforge
        .projects(&[FABRIC_API.to_owned(), "999999999".to_owned()])
        .await
        .unwrap();
    assert_eq!(several.len(), 1, "{several:?}");

    let versions = curseforge
        .versions(FABRIC_API, &target(LoaderKind::Fabric))
        .await
        .unwrap();
    assert!(versions.len() > 1, "{versions:?}");
    assert!(
        versions
            .windows(2)
            .all(|two| two[0].published >= two[1].published),
        "newest first"
    );
    for version in &versions {
        assert_eq!(version.project_id, FABRIC_API);
        assert!(
            version.game_versions.iter().any(|game| game == MC_VERSION),
            "{version:?}"
        );
        assert!(
            version.loaders.iter().any(|loader| loader == "fabric"),
            "{version:?}"
        );
        assert!(version.published.is_some(), "{version:?}");
        let file = &version.file;
        assert_eq!(file.sha1.as_ref().map(String::len), Some(40), "{file:?}");
        let is_jar = Path::new(&file.filename)
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("jar"));
        assert!(is_jar && file.size > 0, "{file:?}");
        assert_ne!(file.url.is_empty(), file.manual_url.is_none(), "{file:?}");
    }
    assert!(
        versions.iter().any(|version| version.channel == Channel::Release),
        "{versions:?}"
    );

    let id: u64 = versions[0].id.parse().unwrap();
    // File ids are 32-bit there: a bigger number is a bad request, not an unknown file.
    let by_id = curseforge.files(&[id, 2_147_483_000]).await.unwrap();
    assert_eq!(by_id.len(), 1, "unknown ids are left out: {by_id:?}");
    assert_eq!(by_id[0].id, versions[0].id);
}

#[tokio::test]
async fn required_dependencies_are_read() {
    let Some(curseforge) = client() else { return };
    let fabric = target(LoaderKind::Fabric);
    let page = curseforge
        .search(&fabric, &query("", SearchSort::Downloads))
        .await
        .unwrap();
    let mut found = false;
    for hit in page.hits.iter().filter(|hit| hit.id != FABRIC_API) {
        let versions = curseforge.versions(&hit.id, &fabric).await.unwrap();
        found = versions
            .iter()
            .flat_map(|version| &version.dependencies)
            .any(|dependency| dependency.kind == DependencyKind::Required && dependency.project_id.is_some());
        if found {
            break;
        }
    }
    assert!(
        found,
        "no required dependency among the most downloaded Fabric mods"
    );
}

/// The one thing a mock can't prove: that our fingerprint of a real file is the one `CurseForge` has.
#[tokio::test]
async fn a_downloaded_file_is_recognised_by_its_fingerprint() {
    let Some(curseforge) = client() else { return };
    let versions = curseforge
        .versions(FABRIC_API, &target(LoaderKind::Fabric))
        .await
        .unwrap();
    let version = versions
        .iter()
        .find(|version| version.file.manual_url.is_none())
        .expect("a file that can be downloaded");
    let response = http_client()
        .unwrap()
        .get(&version.file.url)
        .send()
        .await
        .unwrap();
    assert!(response.status().is_success(), "{}", response.status());
    let bytes = response.bytes().await.unwrap();

    assert_eq!(
        hex::encode(Sha1::digest(&bytes)),
        version.file.sha1.clone().unwrap(),
        "the SHA-1 CurseForge gives is the file's"
    );
    let ours = fingerprint(&bytes);
    let found = curseforge.identify(&[ours]).await.unwrap();
    let recognised = found.get(&ours).expect("CurseForge knows this fingerprint");
    assert_eq!(recognised.project_id, FABRIC_API);
    assert_eq!(recognised.id, version.id);
    let unknown = curseforge.identify(&[1]).await.unwrap();
    assert!(unknown.is_empty(), "{unknown:?}");
}

#[test]
fn dot_env_values_are_read_as_written() {
    let text = "# CURSEFORGE_API_KEY=commented\r\nOTHER=x\r\nexport CURSEFORGE_API_KEY='$2a$10$ab=c'\r\n";
    assert_eq!(
        value_in(text, "CURSEFORGE_API_KEY").as_deref(),
        Some("$2a$10$ab=c"),
        "CRLF, export, quotes, and no expansion"
    );
    assert_eq!(value_in("KEY=\"quoted\"", "KEY").as_deref(), Some("quoted"));
    assert_eq!(value_in("KEY=plain", "KEY").as_deref(), Some("plain"));
    assert_eq!(value_in("KEY=", "KEY").as_deref(), Some(""));
    assert_eq!(value_in("OTHER=x", "KEY"), None);
}
