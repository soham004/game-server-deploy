//! Installing server files into an instance's server directory.
//!
//! Steps: download the server jar (verified, via a shared content-addressed cache), write
//! `eula.txt` from the recorded consent, and seed `server.properties` with the values the app
//! manages (port, MOTD). The server fills in every other property on first start.

use std::{
    fmt::Write as _,
    path::{Path, PathBuf},
};

use chrono::{DateTime, Utc};
use gsd_core::{
    jobs::JobContext,
    net::{DownloadError, ExpectedHash, download_verified},
};

use crate::{
    catalog::{self, Catalog, CatalogError},
    loader::LoaderKind,
    settings::MinecraftServer,
};

/// File name of the jar the instance launches.
pub fn launch_jar(loader: LoaderKind) -> &'static str {
    match loader {
        LoaderKind::Vanilla | LoaderKind::Paper => "server.jar",
        LoaderKind::Fabric => "fabric-server-launch.jar",
    }
}

#[derive(Debug, thiserror::Error)]
pub enum InstallError {
    #[error(transparent)]
    Catalog(#[from] CatalogError),
    #[error(transparent)]
    Download(#[from] DownloadError),
    #[error("no {loader} build exists for Minecraft {mc_version}")]
    NoBuild { loader: LoaderKind, mc_version: String },
    #[error("{loader} version {version} does not exist for Minecraft {mc_version}")]
    UnknownLoaderVersion {
        loader: LoaderKind,
        mc_version: String,
        version: String,
    },
    #[error("filesystem error at {path}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

/// What to install and where.
#[derive(Debug)]
pub struct InstallTarget<'a> {
    pub server: &'a MinecraftServer,
    pub server_dir: &'a Path,
    /// Shared download cache (`<data>/cache/downloads`).
    pub cache_dir: &'a Path,
    pub port: u16,
    pub motd: &'a str,
}

/// Downloads the server and writes the initial files.
pub async fn install(
    catalog: &Catalog,
    target: &InstallTarget<'_>,
    job: &JobContext,
) -> Result<(), InstallError> {
    let server = target.server;
    let jar = target.server_dir.join(launch_jar(server.loader));
    match server.loader {
        LoaderKind::Vanilla => install_vanilla(catalog, server, &jar, target.cache_dir, job).await?,
        LoaderKind::Paper => install_paper(catalog, server, &jar, target.cache_dir, job).await?,
        LoaderKind::Fabric => install_fabric(catalog, server, &jar, job).await?,
    }
    job.check_cancelled().map_err(|_| DownloadError::Cancelled)?;

    job.step("Writing server configuration");
    write_eula(target.server_dir, server.eula_accepted_at).await?;
    seed_properties(target.server_dir, target.port, target.motd).await?;
    Ok(())
}

async fn install_vanilla(
    catalog: &Catalog,
    server: &MinecraftServer,
    jar: &Path,
    cache: &Path,
    job: &JobContext,
) -> Result<(), InstallError> {
    job.step(format!("Downloading Minecraft {} server", server.mc_version));
    let detail = catalog.version_detail(&server.mc_version).await?;
    let download = detail.server_download()?;
    let cached = cache.join(format!("{}.jar", download.sha1));
    fetch_cached(
        catalog,
        &download.url,
        &cached,
        &ExpectedHash::Sha1(download.sha1.clone()),
        job,
    )
    .await?;
    copy(&cached, jar).await
}

async fn install_paper(
    catalog: &Catalog,
    server: &MinecraftServer,
    jar: &Path,
    cache: &Path,
    job: &JobContext,
) -> Result<(), InstallError> {
    let builds = catalog.paper_builds(&server.mc_version).await?;
    let build = match &server.loader_version {
        Some(wanted) => builds
            .iter()
            .find(|build| build.id.to_string() == *wanted)
            .ok_or_else(|| InstallError::UnknownLoaderVersion {
                loader: LoaderKind::Paper,
                mc_version: server.mc_version.clone(),
                version: wanted.clone(),
            })?,
        None => catalog::paper_pick(&builds).ok_or_else(|| no_build(server))?,
    };
    let download = build.server_download().ok_or_else(|| no_build(server))?;

    job.step(format!(
        "Downloading Paper {} build {}",
        server.mc_version, build.id
    ));
    let cached = cache.join(format!("{}.jar", download.checksums.sha256));
    fetch_cached(
        catalog,
        &download.url,
        &cached,
        &ExpectedHash::Sha256(download.checksums.sha256.clone()),
        job,
    )
    .await?;
    copy(&cached, jar).await
}

async fn install_fabric(
    catalog: &Catalog,
    server: &MinecraftServer,
    jar: &Path,
    job: &JobContext,
) -> Result<(), InstallError> {
    let loaders = catalog.fabric_loaders(&server.mc_version).await?;
    let loader = match &server.loader_version {
        Some(wanted) => loaders.iter().find(|l| l.version == *wanted).ok_or_else(|| {
            InstallError::UnknownLoaderVersion {
                loader: LoaderKind::Fabric,
                mc_version: server.mc_version.clone(),
                version: wanted.clone(),
            }
        })?,
        None => catalog::fabric_pick(&loaders).ok_or_else(|| no_build(server))?,
    };
    let installer = catalog.fabric_installer().await?;
    let url = catalog::fabric_server_jar_url(
        &catalog.endpoints().fabric,
        &server.mc_version,
        &loader.version,
        &installer,
    );

    job.step(format!(
        "Downloading Fabric {} for Minecraft {}",
        loader.version, server.mc_version
    ));
    // WHY: Fabric meta builds this jar on demand and publishes no checksum; it is small and
    // served over HTTPS. On first start it downloads (and verifies) the vanilla server itself.
    download_verified(
        catalog.client(),
        &url,
        jar,
        &ExpectedHash::None,
        job.cancel_token(),
        &progress(job),
    )
    .await?;
    Ok(())
}

/// Downloads into the shared cache unless an identical file is already there.
async fn fetch_cached(
    catalog: &Catalog,
    url: &str,
    cached: &Path,
    expected: &ExpectedHash,
    job: &JobContext,
) -> Result<(), InstallError> {
    if tokio::fs::try_exists(cached).await.unwrap_or(false) {
        job.log(format!("Using cached {}", cached.display()));
        return Ok(());
    }
    download_verified(
        catalog.client(),
        url,
        cached,
        expected,
        job.cancel_token(),
        &progress(job),
    )
    .await?;
    Ok(())
}

fn progress(job: &JobContext) -> impl Fn(u64, Option<u64>) + Send + Sync + '_ {
    move |done, total| {
        if let Some(total) = total.filter(|total| *total > 0) {
            #[allow(clippy::cast_precision_loss)]
            job.progress(done as f32 / total as f32);
        }
    }
}

async fn copy(from: &Path, to: &Path) -> Result<(), InstallError> {
    tokio::fs::copy(from, to)
        .await
        .map_err(|source| InstallError::Io {
            path: to.to_owned(),
            source,
        })?;
    Ok(())
}

async fn write_eula(server_dir: &Path, accepted_at: DateTime<Utc>) -> Result<(), InstallError> {
    let path = server_dir.join("eula.txt");
    let text = format!(
        "# Accepted by the server owner on {} in {}.\n# https://aka.ms/MinecraftEULA\neula=true\n",
        accepted_at.to_rfc3339(),
        gsd_brand::NAME
    );
    tokio::fs::write(&path, text)
        .await
        .map_err(|source| InstallError::Io { path, source })
}

/// Writes the managed properties. Other keys are added by the server on first start.
async fn seed_properties(server_dir: &Path, port: u16, motd: &str) -> Result<(), InstallError> {
    let path = server_dir.join("server.properties");
    if tokio::fs::try_exists(&path).await.unwrap_or(false) {
        return Ok(()); // never clobber an existing configuration (reinstalls)
    }
    let text = format!(
        "server-port={port}\nquery.port={port}\nmotd={}\n",
        escape_property(motd)
    );
    tokio::fs::write(&path, text)
        .await
        .map_err(|source| InstallError::Io { path, source })
}

/// Java `.properties` value escaping (backslashes, line breaks, non-ASCII as `\uXXXX`).
fn escape_property(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_ascii() => out.push(c),
            c => {
                let mut units = [0_u16; 2];
                for unit in c.encode_utf16(&mut units) {
                    let _ = write!(out, "\\u{unit:04x}");
                }
            }
        }
    }
    out
}

fn no_build(server: &MinecraftServer) -> InstallError {
    InstallError::NoBuild {
        loader: server.loader,
        mc_version: server.mc_version.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_property_values() {
        assert_eq!(escape_property(r"A \ B"), r"A \\ B");
        assert_eq!(escape_property("two\nlines"), r"two\nlines");
        let backslash = '\\';
        assert_eq!(
            escape_property("Caf\u{e9} \u{2713}"),
            format!("Caf{backslash}u00e9 {backslash}u2713")
        );
    }

    #[test]
    fn plain_ascii_is_unchanged() {
        assert_eq!(escape_property("Friends' server #1"), "Friends' server #1");
    }

    #[tokio::test]
    async fn writes_eula_and_seeds_properties_once() {
        let dir = tempfile::tempdir().unwrap();
        write_eula(dir.path(), Utc::now()).await.unwrap();
        assert!(
            std::fs::read_to_string(dir.path().join("eula.txt"))
                .unwrap()
                .contains("eula=true")
        );

        seed_properties(dir.path(), 25570, "Friends' server")
            .await
            .unwrap();
        let props = std::fs::read_to_string(dir.path().join("server.properties")).unwrap();
        assert!(props.contains("server-port=25570") && props.contains("motd=Friends' server"));

        seed_properties(dir.path(), 1, "changed").await.unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.path().join("server.properties")).unwrap(),
            props,
            "not overwritten"
        );
    }
}
