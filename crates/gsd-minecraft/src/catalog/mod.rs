//! What can be installed: Minecraft versions and, per version, which loaders and loader
//! versions exist. Backed by the Mojang, `PaperMC` (Fill v3) and Fabric meta APIs.
//!
//! Responses are cached in memory for a few minutes and on disk indefinitely, so the version
//! picker keeps working (with stale data) when an upstream API is down or the host is offline.

mod fabric;
mod mojang;
mod paper;

use std::{
    collections::HashMap,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

use chrono::{DateTime, Utc};
use serde::{Serialize, de::DeserializeOwned};
use tokio::sync::Mutex;

pub use fabric::{FabricLoaderVersion, pick_loader as fabric_pick, server_jar_url as fabric_server_jar_url};
pub use mojang::{ServerDownload, VersionDetail, VersionKind};
pub use paper::{PaperBuild, pick_build as paper_pick};

use crate::loader::LoaderKind;

/// How long API responses are reused before asking again.
const MEMORY_TTL: Duration = Duration::from_secs(10 * 60);

/// Upstream base URLs (overridable for tests).
#[derive(Debug, Clone)]
pub struct Endpoints {
    pub mojang_manifest: String,
    pub paper: String,
    pub fabric: String,
}

impl Default for Endpoints {
    fn default() -> Self {
        Self {
            mojang_manifest: "https://piston-meta.mojang.com/mc/game/version_manifest_v2.json".into(),
            paper: "https://fill.papermc.io".into(),
            fabric: "https://meta.fabricmc.net".into(),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum CatalogError {
    #[error("could not reach {url} and nothing is cached")]
    Unreachable {
        url: String,
        #[source]
        source: reqwest::Error,
    },
    #[error("{url} answered HTTP {status}")]
    Status { url: String, status: u16 },
    #[error("unexpected response from {url}: {detail}")]
    Malformed { url: String, detail: String },
    #[error("Minecraft {0} does not exist")]
    UnknownVersion(String),
    #[error("Minecraft {0} has no dedicated server download")]
    NoServerDownload(String),
}

/// A Minecraft version in the picker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GameVersion {
    pub id: String,
    pub kind: VersionKind,
    pub released: DateTime<Utc>,
}

/// Whether a loader can run a given Minecraft version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LoaderAvailability {
    pub loader: LoaderKind,
    pub name: &'static str,
    pub available: bool,
}

/// One selectable loader version (Paper build, Fabric loader version).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LoaderVersion {
    pub id: String,
    pub released: Option<DateTime<Utc>>,
    pub stable: bool,
    /// The one to pick by default (★ in the picker).
    pub recommended: bool,
}

/// Fetches and caches catalog data. Cheap to clone.
#[derive(Debug, Clone)]
pub struct Catalog {
    inner: Arc<Inner>,
}

#[derive(Debug)]
struct Inner {
    client: reqwest::Client,
    endpoints: Endpoints,
    disk_cache: PathBuf,
    memory: Mutex<HashMap<String, (Instant, serde_json::Value)>>,
}

impl Catalog {
    pub fn new(client: reqwest::Client, endpoints: Endpoints, disk_cache: PathBuf) -> Self {
        Self {
            inner: Arc::new(Inner {
                client,
                endpoints,
                disk_cache,
                memory: Mutex::new(HashMap::new()),
            }),
        }
    }

    pub fn client(&self) -> &reqwest::Client {
        &self.inner.client
    }

    pub fn endpoints(&self) -> &Endpoints {
        &self.inner.endpoints
    }

    /// Every Minecraft version, newest first.
    pub async fn game_versions(&self) -> Result<Vec<GameVersion>, CatalogError> {
        Ok(self.manifest().await?.versions())
    }

    /// Download and Java requirements of one Minecraft version.
    pub async fn version_detail(&self, mc_version: &str) -> Result<VersionDetail, CatalogError> {
        let manifest = self.manifest().await?;
        let url = manifest
            .detail_url(mc_version)
            .ok_or_else(|| CatalogError::UnknownVersion(mc_version.into()))?;
        self.fetch_json(url).await
    }

    /// Which loaders support `mc_version`.
    pub async fn loaders_for(&self, mc_version: &str) -> Result<Vec<LoaderAvailability>, CatalogError> {
        let manifest = self.manifest().await?;
        if manifest.find(mc_version).is_none() {
            return Err(CatalogError::UnknownVersion(mc_version.into()));
        }
        let paper = self
            .paper_versions()
            .await
            .map(|versions| versions.iter().any(|v| v == mc_version));
        let fabric = self
            .fabric_game_versions()
            .await
            .map(|versions| versions.iter().any(|v| v == mc_version));
        // A loader whose API is down is reported unavailable rather than failing the whole list.
        Ok(vec![
            availability(LoaderKind::Vanilla, true),
            availability(LoaderKind::Paper, paper.unwrap_or(false)),
            availability(LoaderKind::Fabric, fabric.unwrap_or(false)),
        ])
    }

    /// Loader versions for `mc_version`, newest first. Empty for vanilla.
    pub async fn loader_versions(
        &self,
        loader: LoaderKind,
        mc_version: &str,
    ) -> Result<Vec<LoaderVersion>, CatalogError> {
        match loader {
            LoaderKind::Vanilla => Ok(Vec::new()),
            LoaderKind::Paper => Ok(paper::to_loader_versions(&self.paper_builds(mc_version).await?)),
            LoaderKind::Fabric => Ok(fabric::to_loader_versions(
                &self.fabric_loaders(mc_version).await?,
            )),
        }
    }

    pub async fn paper_builds(&self, mc_version: &str) -> Result<Vec<PaperBuild>, CatalogError> {
        let url = format!(
            "{}/v3/projects/paper/versions/{mc_version}/builds",
            self.inner.endpoints.paper
        );
        let builds: Vec<PaperBuild> = self.fetch_json(&url).await?;
        Ok(paper::sorted(builds))
    }

    pub async fn fabric_loaders(&self, mc_version: &str) -> Result<Vec<FabricLoaderVersion>, CatalogError> {
        let url = format!("{}/v2/versions/loader/{mc_version}", self.inner.endpoints.fabric);
        let entries: Vec<fabric::LoaderEntry> = self.fetch_json(&url).await?;
        Ok(entries.into_iter().map(|entry| entry.loader).collect())
    }

    /// The newest stable Fabric installer version (part of the server-launcher URL).
    pub async fn fabric_installer(&self) -> Result<String, CatalogError> {
        let url = format!("{}/v2/versions/installer", self.inner.endpoints.fabric);
        let installers: Vec<fabric::InstallerEntry> = self.fetch_json(&url).await?;
        fabric::pick_installer(&installers).ok_or(CatalogError::Malformed {
            url,
            detail: "no installer versions".into(),
        })
    }

    async fn manifest(&self) -> Result<mojang::Manifest, CatalogError> {
        let url = self.inner.endpoints.mojang_manifest.clone();
        self.fetch_json(&url).await
    }

    async fn paper_versions(&self) -> Result<Vec<String>, CatalogError> {
        let url = format!("{}/v3/projects/paper", self.inner.endpoints.paper);
        let project: paper::Project = self.fetch_json(&url).await?;
        Ok(project.all_versions())
    }

    async fn fabric_game_versions(&self) -> Result<Vec<String>, CatalogError> {
        let url = format!("{}/v2/versions/game", self.inner.endpoints.fabric);
        let entries: Vec<fabric::GameEntry> = self.fetch_json(&url).await?;
        Ok(entries.into_iter().map(|entry| entry.version).collect())
    }

    /// GET + JSON with memory cache, disk cache and stale-on-error fallback.
    async fn fetch_json<T: DeserializeOwned>(&self, url: &str) -> Result<T, CatalogError> {
        let value = self.fetch_value(url).await?;
        serde_json::from_value(value).map_err(|error| CatalogError::Malformed {
            url: url.into(),
            detail: error.to_string(),
        })
    }

    async fn fetch_value(&self, url: &str) -> Result<serde_json::Value, CatalogError> {
        if let Some((fetched, value)) = self.inner.memory.lock().await.get(url)
            && fetched.elapsed() < MEMORY_TTL
        {
            return Ok(value.clone());
        }
        match self.download_value(url).await {
            Ok(value) => {
                self.remember(url, &value).await;
                Ok(value)
            }
            Err(error) => match self.read_disk(url).await {
                Some(stale) => {
                    tracing::warn!(%url, %error, "using cached catalog data");
                    Ok(stale)
                }
                None => Err(error),
            },
        }
    }

    async fn download_value(&self, url: &str) -> Result<serde_json::Value, CatalogError> {
        let response =
            self.inner
                .client
                .get(url)
                .send()
                .await
                .map_err(|source| CatalogError::Unreachable {
                    url: url.into(),
                    source,
                })?;
        if !response.status().is_success() {
            return Err(CatalogError::Status {
                url: url.into(),
                status: response.status().as_u16(),
            });
        }
        response.json().await.map_err(|error| CatalogError::Malformed {
            url: url.into(),
            detail: error.to_string(),
        })
    }

    async fn remember(&self, url: &str, value: &serde_json::Value) {
        self.inner
            .memory
            .lock()
            .await
            .insert(url.to_owned(), (Instant::now(), value.clone()));
        let path = self.disk_path(url);
        if let Ok(text) = serde_json::to_vec(value) {
            let _ = tokio::fs::create_dir_all(&self.inner.disk_cache).await;
            let _ = tokio::fs::write(path, text).await;
        }
    }

    async fn read_disk(&self, url: &str) -> Option<serde_json::Value> {
        let bytes = tokio::fs::read(self.disk_path(url)).await.ok()?;
        serde_json::from_slice(&bytes).ok()
    }

    fn disk_path(&self, url: &str) -> PathBuf {
        let name: String = url
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '.' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        self.inner.disk_cache.join(format!("{name}.json"))
    }
}

fn availability(loader: LoaderKind, available: bool) -> LoaderAvailability {
    LoaderAvailability {
        loader,
        name: loader.display_name(),
        available,
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{method, path},
    };

    use super::*;

    pub(crate) fn fixture(name: &str) -> serde_json::Value {
        let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    /// A mock upstream serving the recorded fixtures. The manifest's per-version URLs are
    /// rewritten to point at the mock.
    pub(crate) async fn mock_upstream() -> MockServer {
        let server = MockServer::start().await;
        let mut manifest = fixture("mojang_manifest.json");
        for version in manifest["versions"].as_array_mut().unwrap() {
            let id = version["id"].as_str().unwrap().to_owned();
            version["url"] = format!("{}/mojang/versions/{id}.json", server.uri()).into();
        }
        Mock::given(method("GET"))
            .and(path("/mojang/manifest.json"))
            .respond_with(ResponseTemplate::new(200).set_body_json(manifest))
            .mount(&server)
            .await;

        // Downloads point at the mock and carry the checksums of FAKE_JAR.
        let mut detail = fixture("mojang_version_26.3.json");
        detail["downloads"]["server"]["url"] = format!("{}/downloads/vanilla-26.3.jar", server.uri()).into();
        detail["downloads"]["server"]["sha1"] = hash_hex::<sha1::Sha1>(FAKE_JAR).into();
        let mut builds = fixture("paper_builds_26.3.json");
        for build in builds.as_array_mut().unwrap() {
            let id = build["id"].clone();
            let download = &mut build["downloads"]["server:default"];
            download["url"] = format!("{}/downloads/paper-{id}.jar", server.uri()).into();
            download["checksums"]["sha256"] = hash_hex::<sha2::Sha256>(FAKE_JAR).into();
        }

        let json_routes = [
            ("/mojang/versions/26.3.json".to_owned(), detail),
            (
                "/mojang/versions/1.0.json".to_owned(),
                fixture("mojang_version_1.0.json"),
            ),
            ("/v3/projects/paper".to_owned(), fixture("paper_project.json")),
            ("/v3/projects/paper/versions/26.3/builds".to_owned(), builds),
            ("/v2/versions/game".to_owned(), fixture("fabric_game.json")),
            (
                "/v2/versions/loader/26.3".to_owned(),
                fixture("fabric_loaders_26.3.json"),
            ),
            (
                "/v2/versions/installer".to_owned(),
                fixture("fabric_installers.json"),
            ),
        ];
        for (route, body) in json_routes {
            Mock::given(method("GET"))
                .and(path(route))
                .respond_with(ResponseTemplate::new(200).set_body_json(body))
                .mount(&server)
                .await;
        }
        Mock::given(method("GET"))
            .and(wiremock::matchers::path_regex(
                r"^/(downloads/.*\.jar|v2/versions/loader/26\.3/[^/]+/[^/]+/server/jar)$",
            ))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(FAKE_JAR))
            .mount(&server)
            .await;
        server
    }

    /// Stand-in for every server jar served by the mock.
    pub(crate) const FAKE_JAR: &[u8] = b"PK\x03\x04 not really a jar";

    fn hash_hex<D: sha2::Digest>(bytes: &[u8]) -> String {
        hex::encode(D::digest(bytes))
    }

    pub(crate) fn catalog_for(server: &MockServer, cache: &std::path::Path) -> Catalog {
        let endpoints = Endpoints {
            mojang_manifest: format!("{}/mojang/manifest.json", server.uri()),
            paper: server.uri(),
            fabric: server.uri(),
        };
        Catalog::new(gsd_core::net::http_client().unwrap(), endpoints, cache.to_owned())
    }

    #[tokio::test]
    async fn lists_versions_newest_first() {
        let server = mock_upstream().await;
        let dir = tempfile::tempdir().unwrap();
        let versions = catalog_for(&server, dir.path()).game_versions().await.unwrap();
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
    async fn loader_availability_per_version() {
        let server = mock_upstream().await;
        let dir = tempfile::tempdir().unwrap();
        let catalog = catalog_for(&server, dir.path());

        let loaders = catalog.loaders_for("26.3").await.unwrap();
        assert!(loaders.iter().all(|l| l.available), "{loaders:?}");

        let old = catalog.loaders_for("1.0").await.unwrap();
        assert!(
            old.iter()
                .find(|l| l.loader == LoaderKind::Paper)
                .is_some_and(|l| !l.available)
        );
        assert!(matches!(
            catalog.loaders_for("9.9.9").await,
            Err(CatalogError::UnknownVersion(_))
        ));
    }

    #[tokio::test]
    async fn paper_builds_prefer_the_newest_stable() {
        let server = mock_upstream().await;
        let dir = tempfile::tempdir().unwrap();
        let versions = catalog_for(&server, dir.path())
            .loader_versions(LoaderKind::Paper, "26.3")
            .await
            .unwrap();
        assert_ne!(versions.len(), 0);
        assert_eq!(versions.iter().filter(|v| v.recommended).count(), 1);
        let recommended = versions.iter().find(|v| v.recommended).unwrap();
        assert!(recommended.stable);
        assert!(
            versions
                .windows(2)
                .all(|w| w[0].id.parse::<u32>().unwrap() > w[1].id.parse::<u32>().unwrap())
        );
    }

    #[tokio::test]
    async fn fabric_versions_and_installer() {
        let server = mock_upstream().await;
        let dir = tempfile::tempdir().unwrap();
        let catalog = catalog_for(&server, dir.path());
        let versions = catalog.loader_versions(LoaderKind::Fabric, "26.3").await.unwrap();
        assert_eq!(versions[0].id, "0.20.1");
        assert!(versions[0].recommended);
        assert_eq!(catalog.fabric_installer().await.unwrap(), "1.1.2");
        assert_eq!(
            catalog
                .loader_versions(LoaderKind::Vanilla, "26.3")
                .await
                .unwrap()
                .len(),
            0
        );
    }

    #[tokio::test]
    async fn version_details_give_java_and_server_download() {
        let server = mock_upstream().await;
        let dir = tempfile::tempdir().unwrap();
        let catalog = catalog_for(&server, dir.path());

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
        let online = catalog_for(&server, dir.path());
        let fresh = online.game_versions().await.unwrap();

        server.reset().await; // upstream now answers 404 to everything
        let offline = catalog_for(&server, dir.path()); // new catalog: empty memory cache
        assert_eq!(offline.game_versions().await.unwrap(), fresh);

        let empty_dir = tempfile::tempdir().unwrap();
        assert!(
            catalog_for(&server, empty_dir.path())
                .game_versions()
                .await
                .is_err()
        );
    }
}
