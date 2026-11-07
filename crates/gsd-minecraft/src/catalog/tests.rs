//! Catalog behaviour against the mock upstream (recorded API responses).

use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{method, path},
};

use super::*;
use crate::{
    loader::{ContentKind, LoaderKind},
    test_support::{catalog_for, fixture, mock_upstream},
};

async fn catalog() -> (Catalog, MockServer, tempfile::TempDir) {
    let server = mock_upstream().await;
    let dir = tempfile::tempdir().unwrap();
    (catalog_for(&server, dir.path()), server, dir)
}

#[tokio::test]
async fn lists_versions_newest_first() {
    let (catalog, _server, _dir) = catalog().await;
    let versions = catalog.game_versions().await.unwrap();
    assert_eq!(versions[0].id, "26.4-snapshot-7");
    assert!(versions.windows(2).all(|w| w[0].released >= w[1].released));
    assert!(
        versions
            .iter()
            .any(|v| v.id == "26.3" && v.kind == VersionKind::Release)
    );
    assert!(versions.iter().any(|v| v.kind == VersionKind::OldAlpha));
}

#[tokio::test]
async fn every_loader_is_available_for_26_3() {
    let (catalog, _server, _dir) = catalog().await;
    let loaders = catalog.loaders_for("26.3").await.unwrap();
    let names: Vec<LoaderKind> = loaders.iter().map(|l| l.loader).collect();
    assert_eq!(names, LoaderKind::ALL, "every loader, in picker order");
    assert!(loaders.iter().all(|l| l.available), "{loaders:?}");
    let content = |kind| loaders.iter().find(|l| l.loader == kind).unwrap().content;
    assert_eq!(content(LoaderKind::Paper), ContentKind::Plugins);
    assert_eq!(content(LoaderKind::NeoForge), ContentKind::Mods);
    assert_eq!(content(LoaderKind::Vanilla), ContentKind::None);
}

#[tokio::test]
async fn old_versions_offer_only_what_supports_them() {
    let (catalog, _server, _dir) = catalog().await;
    let old = catalog.loaders_for("1.0").await.unwrap();
    let available: Vec<LoaderKind> = old.iter().filter(|l| l.available).map(|l| l.loader).collect();
    assert_eq!(available, [LoaderKind::Vanilla]);
    assert!(matches!(
        catalog.loaders_for("9.9.9").await,
        Err(CatalogError::UnknownVersion(_))
    ));
}

#[tokio::test]
async fn a_loader_api_outage_marks_only_that_loader_unavailable() {
    let server = MockServer::start().await;
    // Only Mojang answers.
    Mock::given(method("GET"))
        .and(path("/mojang/manifest.json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(fixture("mojang_manifest.json")))
        .mount(&server)
        .await;
    let dir = tempfile::tempdir().unwrap();
    let loaders = catalog_for(&server, dir.path())
        .loaders_for("26.3")
        .await
        .unwrap();
    let available: Vec<LoaderKind> = loaders.iter().filter(|l| l.available).map(|l| l.loader).collect();
    assert_eq!(available, [LoaderKind::Vanilla]);
}

#[tokio::test]
async fn every_loader_lists_versions_with_exactly_one_recommended() {
    let (catalog, _server, _dir) = catalog().await;
    for loader in LoaderKind::ALL.into_iter().filter(|l| l.has_versions()) {
        let versions = catalog.loader_versions(loader, "26.3").await.unwrap();
        assert!(!versions.is_empty(), "{loader}");
        assert_eq!(
            versions.iter().filter(|v| v.recommended).count(),
            1,
            "{loader}: {versions:?}"
        );
    }
    let vanilla = catalog
        .loader_versions(LoaderKind::Vanilla, "26.3")
        .await
        .unwrap();
    assert_eq!(vanilla, Vec::<LoaderVersion>::new());
}

#[tokio::test]
async fn artifacts_carry_the_right_checksum_kind() {
    use gsd_core::net::ExpectedHash;
    let (catalog, server, _dir) = catalog().await;
    let cases = [
        (LoaderKind::Vanilla, None),
        (LoaderKind::Paper, Some("244")),
        (LoaderKind::Purpur, Some("2657")),
        (LoaderKind::Fabric, Some("0.20.1")),
        (LoaderKind::Quilt, Some("0.30.2")),
        (LoaderKind::Forge, Some("66.1.0")),
        (LoaderKind::NeoForge, Some("26.3.0.101")),
    ];
    for (loader, version) in cases {
        let artifact = catalog.artifact(loader, "26.3", version).await.unwrap();
        assert!(
            artifact.url.starts_with(&server.uri()),
            "{loader}: {}",
            artifact.url
        );
        let kind_ok = match loader {
            LoaderKind::Vanilla | LoaderKind::Quilt | LoaderKind::Forge | LoaderKind::NeoForge => {
                matches!(artifact.hash, ExpectedHash::Sha1(ref h) if h.len() == 40)
            }
            LoaderKind::Paper => matches!(artifact.hash, ExpectedHash::Sha256(_)),
            LoaderKind::Purpur => matches!(artifact.hash, ExpectedHash::Md5(_)),
            LoaderKind::Fabric => artifact.hash == ExpectedHash::None,
        };
        assert!(kind_ok, "{loader}: {:?}", artifact.hash);
    }
    let forge = catalog
        .artifact(LoaderKind::Forge, "26.3", Some("66.1.0"))
        .await
        .unwrap();
    assert!(
        forge
            .url
            .ends_with("/net/minecraftforge/forge/26.3-66.1.0/forge-26.3-66.1.0-installer.jar")
    );
    let neoforge = catalog
        .artifact(LoaderKind::NeoForge, "26.3", Some("26.3.0.101"))
        .await
        .unwrap();
    assert!(
        neoforge
            .url
            .ends_with("/releases/net/neoforged/neoforge/26.3.0.101/neoforge-26.3.0.101-installer.jar")
    );
    let quilt = catalog
        .artifact(LoaderKind::Quilt, "26.3", Some("0.30.2"))
        .await
        .unwrap();
    assert!(
        quilt
            .url
            .ends_with("/org/quiltmc/quilt-installer/0.15.1/quilt-installer-0.15.1.jar")
    );
}

#[tokio::test]
async fn unknown_loader_versions_are_rejected() {
    let (catalog, _server, _dir) = catalog().await;
    for (loader, version) in [
        (LoaderKind::Paper, "999999"),
        (LoaderKind::Purpur, "1"),
        (LoaderKind::Fabric, "0.0.1-nope"),
        (LoaderKind::Forge, "1.2.3"),
        (LoaderKind::NeoForge, "21.1.77"), // exists, but for 1.21.1
    ] {
        let result = catalog.artifact(loader, "26.3", Some(version)).await;
        assert!(
            matches!(result, Err(CatalogError::UnknownLoaderVersion { .. })),
            "{loader} {version}: {result:?}"
        );
    }
    assert!(matches!(
        catalog.artifact(LoaderKind::Forge, "26.3", None).await,
        Err(CatalogError::UnknownLoaderVersion { .. })
    ));
}

#[tokio::test]
async fn version_details_give_java_and_server_download() {
    let (catalog, _server, _dir) = catalog().await;
    let detail = catalog.version_detail("26.3").await.unwrap();
    assert_eq!(detail.java_major(), 25);
    assert_eq!(detail.server_download().unwrap().sha1.len(), 40);

    let ancient = catalog.version_detail("1.0").await.unwrap();
    assert!(matches!(
        ancient.server_download(),
        Err(CatalogError::NoServerDownload(_))
    ));
    assert_eq!(
        ancient.java_major(),
        8,
        "versions without javaVersion default to 8"
    );
}

#[tokio::test]
async fn falls_back_to_the_disk_cache_when_offline() {
    let server = mock_upstream().await;
    let dir = tempfile::tempdir().unwrap();
    let fresh = catalog_for(&server, dir.path()).game_versions().await.unwrap();
    let forge = catalog_for(&server, dir.path())
        .loader_versions(LoaderKind::Forge, "26.3")
        .await
        .unwrap();

    server.reset().await; // upstream now answers 404 to everything
    let offline = catalog_for(&server, dir.path()); // new catalog: empty memory cache
    assert_eq!(offline.game_versions().await.unwrap(), fresh);
    assert_eq!(
        offline.loader_versions(LoaderKind::Forge, "26.3").await.unwrap(),
        forge
    );

    let empty_dir = tempfile::tempdir().unwrap();
    assert!(
        catalog_for(&server, empty_dir.path())
            .game_versions()
            .await
            .is_err()
    );
}
