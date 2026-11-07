//! [`Catalog`]: the questions the version picker and the installer ask, answered by
//! dispatching to the source of each loader.

use gsd_core::net::ExpectedHash;

use super::{
    Artifact, CatalogError, Endpoints, Fetcher, GameVersion, LoaderAvailability, LoaderVersion, fabric,
    forge,
    mojang::{self, VersionDetail},
    neoforge, paper, purpur, quilt,
};
use crate::loader::LoaderKind;

/// Minecraft versions and server software. Cheap to clone.
#[derive(Debug, Clone)]
pub struct Catalog {
    fetcher: Fetcher,
    endpoints: Endpoints,
}

impl Catalog {
    pub fn new(client: reqwest::Client, endpoints: Endpoints, disk_cache: std::path::PathBuf) -> Self {
        Self {
            fetcher: Fetcher::new(client, disk_cache),
            endpoints,
        }
    }

    pub fn client(&self) -> &reqwest::Client {
        self.fetcher.client()
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
        self.fetcher.json(url).await
    }

    /// Which loaders support `mc_version`. A loader whose API is down is reported unavailable
    /// rather than failing the whole list.
    pub async fn loaders_for(&self, mc_version: &str) -> Result<Vec<LoaderAvailability>, CatalogError> {
        if self.manifest().await?.find(mc_version).is_none() {
            return Err(CatalogError::UnknownVersion(mc_version.into()));
        }
        let checks = LoaderKind::ALL.map(|loader| async move {
            let available = self.supports(loader, mc_version).await.unwrap_or_else(|error| {
                tracing::warn!(%loader, %error, "could not check loader availability");
                false
            });
            LoaderAvailability::new(loader, available)
        });
        Ok(futures_util::future::join_all(checks).await)
    }

    /// Loader versions for `mc_version`, newest first. Empty for vanilla.
    pub async fn loader_versions(
        &self,
        loader: LoaderKind,
        mc_version: &str,
    ) -> Result<Vec<LoaderVersion>, CatalogError> {
        let (f, e) = (&self.fetcher, &self.endpoints);
        match loader {
            LoaderKind::Vanilla => Ok(Vec::new()),
            LoaderKind::Paper => paper::versions(f, &e.paper, mc_version).await,
            LoaderKind::Purpur => purpur::versions(f, &e.purpur, mc_version).await,
            LoaderKind::Fabric => fabric::versions(f, &e.fabric, mc_version).await,
            LoaderKind::Quilt => quilt::versions(f, &e.quilt_meta, mc_version).await,
            LoaderKind::Forge => forge::versions(f, &e.forge_files, mc_version).await,
            LoaderKind::NeoForge => neoforge::versions(f, &e.neoforge_maven, mc_version).await,
        }
    }

    /// What to download for a server: the server jar itself (Vanilla, Paper, Purpur, Fabric) or
    /// the loader's installer (Quilt, Forge, `NeoForge`; see [`LoaderKind::uses_installer`]).
    /// `loader_version` is ignored for vanilla and required otherwise.
    pub async fn artifact(
        &self,
        loader: LoaderKind,
        mc_version: &str,
        loader_version: Option<&str>,
    ) -> Result<Artifact, CatalogError> {
        let (f, e) = (&self.fetcher, &self.endpoints);
        let version = || {
            loader_version.ok_or_else(|| CatalogError::UnknownLoaderVersion {
                loader,
                mc_version: mc_version.into(),
                version: "(none)".into(),
            })
        };
        match loader {
            LoaderKind::Vanilla => self.vanilla_server(mc_version).await,
            LoaderKind::Paper => paper::artifact(f, &e.paper, mc_version, version()?).await,
            LoaderKind::Purpur => purpur::artifact(f, &e.purpur, mc_version, version()?).await,
            LoaderKind::Fabric => fabric::artifact(f, &e.fabric, mc_version, version()?).await,
            LoaderKind::Quilt => quilt::installer(f, &e.quilt_maven).await,
            LoaderKind::Forge => {
                forge::installer(f, &e.forge_files, &e.forge_maven, mc_version, version()?).await
            }
            LoaderKind::NeoForge => neoforge::installer(f, &e.neoforge_maven, mc_version, version()?).await,
        }
    }

    async fn supports(&self, loader: LoaderKind, mc_version: &str) -> Result<bool, CatalogError> {
        let (f, e) = (&self.fetcher, &self.endpoints);
        match loader {
            LoaderKind::Vanilla => Ok(true),
            LoaderKind::Paper => paper::supports(f, &e.paper, mc_version).await,
            LoaderKind::Purpur => purpur::supports(f, &e.purpur, mc_version).await,
            LoaderKind::Fabric => fabric::supports(f, &e.fabric, mc_version).await,
            LoaderKind::Quilt => quilt::supports(f, &e.quilt_meta, mc_version).await,
            LoaderKind::Forge => forge::supports(f, &e.forge_files, mc_version).await,
            LoaderKind::NeoForge => neoforge::supports(f, &e.neoforge_maven, mc_version).await,
        }
    }

    async fn vanilla_server(&self, mc_version: &str) -> Result<Artifact, CatalogError> {
        let detail = self.version_detail(mc_version).await?;
        let download = detail.server_download()?;
        Ok(Artifact {
            url: download.url.clone(),
            hash: ExpectedHash::Sha1(download.sha1.clone()),
        })
    }

    async fn manifest(&self) -> Result<mojang::Manifest, CatalogError> {
        self.fetcher.json(&self.endpoints.mojang_manifest).await
    }
}
