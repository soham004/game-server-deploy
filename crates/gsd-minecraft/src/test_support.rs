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

/// Stand-in for every jar (server or installer) served by the mock. A real zip, so code that opens a
/// jar can: a bundler (`META-INF/versions.list`) with one data pack file, which the data generator's
/// path reads ids from (`commands/datagen.rs`).
pub fn fake_jar() -> &'static [u8] {
    static JAR: std::sync::LazyLock<Vec<u8>> = std::sync::LazyLock::new(|| {
        use std::io::Write;
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored)
            .last_modified_time(zip::DateTime::default());
        for (name, contents) in [
            ("META-INF/versions.list", ""),
            ("data/minecraft/enchantment/sharpness.json", "{}"),
        ] {
            writer.start_file(name, options).unwrap();
            writer.write_all(contents.as_bytes()).unwrap();
        }
        writer.finish().unwrap().into_inner()
    });
    &JAR
}

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

/// A skin `height` pixels high (64, or 32 as before 1.8): a red face, and a hat layer that is clear except
/// for one blue pixel over the face's top left corner.
pub fn test_skin(height: u32) -> Vec<u8> {
    let mut pixels = vec![0_u8; 64 * height as usize * 4];
    let mut set = |x: usize, y: usize, color: [u8; 4]| {
        let at = (y * 64 + x) * 4;
        pixels[at..at + 4].copy_from_slice(&color);
    };
    for y in 8..16 {
        for x in 8..16 {
            set(x, y, [200, 0, 0, 255]);
        }
    }
    set(40, 8, [0, 0, 255, 255]);
    let mut out = Vec::new();
    let mut encoder = png::Encoder::new(&mut out, 64, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().unwrap();
    writer.write_image_data(&pixels).unwrap();
    writer.finish().unwrap();
    out
}

/// A mock upstream for all sources (see [`Endpoints::all_at`]). Download URLs and checksums in
/// the recorded data are rewritten to point at the mock and match [`fake_jar()`].
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
        .respond_with(ResponseTemplate::new(200).set_body_string(hash_hex::<sha1::Sha1>(fake_jar())))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path_regex(r"(\.jar|/download|/server/jar)$"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(fake_jar()))
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
    detail["downloads"]["server"]["sha1"] = hash_hex::<sha1::Sha1>(fake_jar()).into();
    // 26.2 is served as a copy of 26.3's detail (only the id differs), for version changes.
    let mut older = detail.clone();
    older["id"] = "26.2".into();
    older["downloads"]["server"]["url"] = format!("{base}/downloads/vanilla-26.2.jar").into();
    let mut paper_builds = fixture("paper_builds_26.3.json");
    for build in paper_builds.as_array_mut().unwrap() {
        let id = build["id"].clone();
        let download = &mut build["downloads"]["server:default"];
        download["url"] = format!("{base}/downloads/paper-{id}.jar").into();
        download["checksums"]["sha256"] = hash_hex::<sha2::Sha256>(fake_jar()).into();
    }
    let mut purpur_build = fixture("purpur_build_26.3_2657.json");
    purpur_build["md5"] = hash_hex::<md5::Md5>(fake_jar()).into();

    vec![
        ("/mojang/manifest.json".into(), manifest),
        ("/mojang/versions/26.3.json".into(), detail),
        ("/mojang/versions/26.2.json".into(), older),
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
/// - run with `--reports --output <dir>` (the data generator): refuses unless `server.jar` is in
///   the working directory, then copies `datagen-output/` (the real 26.3 `commands.json` and
///   `registries.json`) to `<dir>`. A `fail-datagen` file makes it fail with exit code 5.
/// - otherwise: plays a Minecraft server on stdin/stdout (`stop`, `say`, `join <name>`,
///   `leave <name>`, `kick <name>`, `op <name>` (writes `ops.json` with just them), `save-off`,
///   `save-all flush`, `save-on`; `write <text>` puts `<text>` in `world/level.dat`; `help`
///   answers with `help.txt`, one usage a line, a mod's `chunky` among vanilla ones). A `no-save`
///   file in `java_home` makes it never confirm a save.
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
    let reports = java_home.join("datagen-output").join("reports");
    std::fs::create_dir_all(&reports).unwrap();
    for (fixture, name) in [
        ("commands_26.3.json.gz", "commands.json"),
        ("registries_26.3.json.gz", "registries.json"),
    ] {
        std::fs::write(reports.join(name), gunzip_fixture(fixture)).unwrap();
    }
    std::fs::write(
        java_home.join("help.txt"),
        "/give <targets> <item> [<count>]\n/chunky [start|pause|cancel]\n/xp -> experience\n",
    )
    .unwrap();
}

fn gunzip_fixture(name: &str) -> Vec<u8> {
    use std::io::Read;
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name);
    let mut bytes = Vec::new();
    flate2::read::GzDecoder::new(std::fs::File::open(path).unwrap())
        .read_to_end(&mut bytes)
        .unwrap();
    bytes
}

const FAKE_JAVA: &str = r#"#!/bin/sh
home="$(cd "$(dirname "$0")/.." && pwd)"
jar=""; previous=""; output=""; reports=""
for arg in "$@"; do
  [ "$previous" = "-jar" ] && jar="$arg"
  [ "$previous" = "--output" ] && output="$arg"
  [ "$arg" = "--reports" ] && reports=1
  previous="$arg"
done
if [ -n "$reports" ]; then
  echo "fake data generator: $*"
  [ -f server.jar ] || { echo "server.jar was not copied into $(pwd)" >&2; exit 4; }
  if [ -e "$home/fail-datagen" ]; then echo "java.lang.IllegalStateException: boom" >&2; exit 5; fi
  mkdir -p "$output" && cp -R "$home/datagen-output/." "$output/"
  echo "All providers took: 1 ms"
  exit 0
fi
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
    leave\ *) echo "[12:00:05] [Server thread/INFO]: System chat: ${line#leave } left the game" ;;
    kick\ *) who="${line#kick }"; who="${who%% *}"
      echo "[12:00:05] [Server thread/INFO]: System chat: $who left the game" ;;
    op\ *) who="${line#op }"
      printf '[{"uuid":"11111111-1111-4111-8111-111111111111","name":"%s","level":4,"bypassesPlayerLimit":false}]\n' "$who" > ops.json
      echo "[12:00:05] [Server thread/INFO]: System chat: Made $who a server operator" ;;
    save-off) echo "[12:00:06] [Server thread/INFO]: System chat: Automatic saving is now disabled" ;;
    save-on) echo "[12:00:06] [Server thread/INFO]: System chat: Automatic saving is now enabled" ;;
    save-all\ flush)
      echo "[12:00:06] [Server thread/INFO]: System chat: Saving the game (this may take a moment!)"
      [ -e "$home/no-save" ] || echo "[12:00:06] [Server thread/INFO]: System chat: Saved the game" ;;
    write\ *) mkdir -p world; printf '%s' "${line#write }" > world/level.dat ;;
    help) while IFS= read -r usage; do echo "[12:00:05] [Server thread/INFO]: System chat: $usage"; done < "$home/help.txt" ;;
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
