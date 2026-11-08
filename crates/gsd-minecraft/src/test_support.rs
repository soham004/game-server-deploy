//! Test support shared with other crates (feature `test-support`): a mock of every upstream API
//! serving responses recorded from the live services (`tests/fixtures`, 2026-11-07), and a fake
//! `java` that plays both the loader installers and a Minecraft server.

#![allow(clippy::unwrap_used, clippy::missing_panics_doc)]

use std::path::Path;

use sha2::Digest;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{method, path, path_regex},
};

use crate::catalog::{Catalog, Endpoints};

/// Stand-in for every jar (server or installer) served by the mock.
pub const FAKE_JAR: &[u8] = b"PK\x03\x04 not really a jar";

pub fn fixture(name: &str) -> serde_json::Value {
    serde_json::from_str(&fixture_text(name)).unwrap()
}

pub fn fixture_text(name: &str) -> String {
    let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(path).unwrap()
}

pub fn hash_hex<D: Digest>(bytes: &[u8]) -> String {
    hex::encode(D::digest(bytes))
}

/// A mock upstream for all sources (see [`Endpoints::all_at`]). Download URLs and checksums in
/// the recorded data are rewritten to point at the mock and match [`FAKE_JAR`].
pub async fn mock_upstream() -> MockServer {
    let server = MockServer::start().await;
    for (route, body) in json_routes(&server.uri()) {
        Mock::given(method("GET"))
            .and(path(route))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .mount(&server)
            .await;
    }
    Mock::given(method("GET"))
        .and(path("/org/quiltmc/quilt-installer/maven-metadata.xml"))
        .respond_with(
            ResponseTemplate::new(200).set_body_string(fixture_text("quilt_installer_metadata.xml")),
        )
        .mount(&server)
        .await;
    // Maven side files, then every jar.
    Mock::given(method("GET"))
        .and(path_regex(r"\.jar\.sha1$"))
        .respond_with(ResponseTemplate::new(200).set_body_string(hash_hex::<sha1::Sha1>(FAKE_JAR)))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path_regex(r"(\.jar|/download|/server/jar)$"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(FAKE_JAR))
        .mount(&server)
        .await;
    server
}

fn json_routes(base: &str) -> Vec<(String, serde_json::Value)> {
    let mut manifest = fixture("mojang_manifest.json");
    for version in manifest["versions"].as_array_mut().unwrap() {
        let id = version["id"].as_str().unwrap().to_owned();
        version["url"] = format!("{base}/mojang/versions/{id}.json").into();
    }
    let mut detail = fixture("mojang_version_26.3.json");
    detail["downloads"]["server"]["url"] = format!("{base}/downloads/vanilla-26.3.jar").into();
    detail["downloads"]["server"]["sha1"] = hash_hex::<sha1::Sha1>(FAKE_JAR).into();
    let mut paper_builds = fixture("paper_builds_26.3.json");
    for build in paper_builds.as_array_mut().unwrap() {
        let id = build["id"].clone();
        let download = &mut build["downloads"]["server:default"];
        download["url"] = format!("{base}/downloads/paper-{id}.jar").into();
        download["checksums"]["sha256"] = hash_hex::<sha2::Sha256>(FAKE_JAR).into();
    }
    let mut purpur_build = fixture("purpur_build_26.3_2657.json");
    purpur_build["md5"] = hash_hex::<md5::Md5>(FAKE_JAR).into();

    vec![
        ("/mojang/manifest.json".into(), manifest),
        ("/mojang/versions/26.3.json".into(), detail),
        (
            "/mojang/versions/1.0.json".into(),
            fixture("mojang_version_1.0.json"),
        ),
        ("/v3/projects/paper".into(), fixture("paper_project.json")),
        ("/v3/projects/paper/versions/26.3/builds".into(), paper_builds),
        ("/v2/purpur".into(), fixture("purpur_project.json")),
        ("/v2/purpur/26.3".into(), fixture("purpur_builds_26.3.json")),
        ("/v2/purpur/26.3/2657".into(), purpur_build),
        ("/v2/versions/game".into(), fixture("fabric_game.json")),
        (
            "/v2/versions/loader/26.3".into(),
            fixture("fabric_loaders_26.3.json"),
        ),
        ("/v2/versions/installer".into(), fixture("fabric_installers.json")),
        ("/v3/versions/game".into(), fixture("quilt_game.json")),
        (
            "/v3/versions/loader/26.3".into(),
            fixture("quilt_loaders_26.3.json"),
        ),
        (
            "/net/minecraftforge/forge/maven-metadata.json".into(),
            fixture("forge_maven_metadata.json"),
        ),
        (
            "/net/minecraftforge/forge/promotions_slim.json".into(),
            fixture("forge_promotions.json"),
        ),
        (
            "/api/maven/versions/releases/net/neoforged/neoforge".into(),
            fixture("neoforge_versions.json"),
        ),
    ]
}

/// A catalog reading from `server`, caching in `cache`.
pub fn catalog_for(server: &MockServer, cache: &Path) -> Catalog {
    Catalog::new(
        gsd_core::net::http_client().unwrap(),
        Endpoints::all_at(&server.uri()),
        cache.to_owned(),
    )
}

/// Installs a fake Java 25 at `java_home`. Its `bin/java` script:
/// - run as `java … -jar <name>-installer.jar …`: refuses unless that jar was downloaded into
///   the working directory, then copies `installer-output/<jar>/` there, like the real installer
///   would create those files (built from the real installers' `run.sh`). A `fail-installer`
///   file in `java_home` makes it fail with exit code 3.
/// - otherwise: plays a Minecraft server on stdin/stdout (`stop`, `say`, `join <name>`).
///
/// The script needs `sh`, so only Unix tests run it. On Windows it is still found as Java 25
/// (detection reads the `release` file without running the binary), which is enough for
/// installs that only download.
pub fn install_fake_java(java_home: &Path) {
    std::fs::create_dir_all(java_home.join("bin")).unwrap();
    std::fs::write(
        java_home.join("release"),
        "JAVA_VERSION=\"25.0.1\"\nIMPLEMENTOR=\"Test\"\n",
    )
    .unwrap();
    let java = java_home
        .join("bin")
        .join(if cfg!(windows) { "java.exe" } else { "java" });
    std::fs::write(&java, FAKE_JAVA).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&java, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    for (jar, files) in installer_outputs() {
        for (file, contents) in files {
            let path = java_home.join("installer-output").join(jar).join(file);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, contents).unwrap();
        }
    }
}

const FAKE_JAVA: &str = r#"#!/bin/sh
home="$(cd "$(dirname "$0")/.." && pwd)"
jar=""; previous=""
for arg in "$@"; do
  [ "$previous" = "-jar" ] && jar="$arg"
  previous="$arg"
done
case "$jar" in
  *-installer.jar)
    echo "fake installer: $*"
    [ -f "$jar" ] || { echo "$jar was not downloaded into $(pwd)" >&2; exit 4; }
    if [ -e "$home/fail-installer" ]; then echo "java.lang.IllegalStateException: boom" >&2; exit 3; fi
    cp -R "$home/installer-output/$jar/." .
    echo "The server installed successfully"
    exit 0 ;;
esac
echo "[12:00:00] [Server thread/INFO]: Starting minecraft server version 26.3"
echo "[12:00:00] [Server thread/INFO]: Launched with: $*"
echo '[12:00:01] [Server thread/INFO]: Done (0.42s)! For help, type "help"'
while IFS= read -r line; do
  case "$line" in
    stop) echo "[12:00:09] [Server thread/INFO]: Stopping the server"; exit 0 ;;
    say\ *) echo "[12:00:05] [Server thread/INFO]: [Server] ${line#say }" ;;
    join\ *) echo "[12:00:05] [Server thread/INFO]: ${line#join } joined the game" ;;
    *) echo "[12:00:05] [Server thread/INFO]: Unknown or incomplete command" ;;
  esac
done
"#;

type InstallerOutput = (&'static str, Vec<(&'static str, String)>);

/// What each fake installer leaves in the server directory.
fn installer_outputs() -> [InstallerOutput; 3] {
    [
        (
            "forge-installer.jar",
            vec![
                ("run.sh", fixture_text("forge_26.3_run.sh")),
                ("run.bat", "@echo off\n".into()),
                ("user_jvm_args.txt", fixture_text("forge_26.3_user_jvm_args.txt")),
                (
                    "libraries/net/minecraftforge/forge/26.3-66.1.0/unix_args.txt",
                    "-jar forge-26.3-66.1.0-shim.jar\n".into(),
                ),
                ("forge-installer.jar.log", "installer log\n".into()),
            ],
        ),
        (
            "neoforge-installer.jar",
            vec![
                ("run.sh", fixture_text("neoforge_26.3_run.sh")),
                (
                    "user_jvm_args.txt",
                    fixture_text("neoforge_26.3_user_jvm_args.txt"),
                ),
                (
                    "libraries/net/neoforged/neoforge/26.3.0.101/unix_args.txt",
                    "-DlibraryDirectory=libraries\n".into(),
                ),
            ],
        ),
        (
            "quilt-installer.jar",
            vec![
                ("quilt-server-launch.jar", "launcher".into()),
                ("server.jar", "vanilla".into()),
                ("libraries/org/quiltmc/quilt-loader.jar", "loader".into()),
            ],
        ),
    ]
}
