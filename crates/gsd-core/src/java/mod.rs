//! Java runtimes: find installed JDKs/JREs and download Eclipse Temurin on demand.
//!
//! Each Minecraft version needs a specific Java major (26.x → 25, 1.20.5–1.21 → 21, …), and the
//! wrong one is the most common reason a server won't start. Managed runtimes live in
//! `<data>/runtimes/java/<vendor>-<major>-<arch>` and are preferred over system installs.

mod adoptium;
mod detect;

use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use serde::Serialize;
use tokio::sync::{Mutex, RwLock};

pub use adoptium::ADOPTIUM_API;
pub use detect::{parse_major, parse_release_file};

use crate::jobs::JobContext;

/// One usable Java installation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct JavaRuntime {
    /// Stable id: the directory name for managed runtimes, the home path otherwise.
    pub id: String,
    pub home: PathBuf,
    pub executable: PathBuf,
    pub major: u32,
    pub version: String,
    pub vendor: Option<String>,
    /// Downloaded and owned by this app.
    pub managed: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum JavaError {
    #[error("no Java {major} build is available for this platform ({os}/{arch})")]
    NotAvailable {
        major: u32,
        os: &'static str,
        arch: &'static str,
    },
    #[error("could not query the Adoptium API")]
    Api(#[source] reqwest::Error),
    #[error(transparent)]
    Download(#[from] crate::net::DownloadError),
    #[error("could not unpack the Java archive")]
    Extract(#[source] std::io::Error),
    #[error("the downloaded archive contains no bin/java")]
    NoExecutable,
    #[error("filesystem error at {path}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

/// Detects and installs Java runtimes. Cheap to clone.
#[derive(Debug, Clone)]
pub struct JavaRuntimes {
    inner: Arc<Inner>,
}

#[derive(Debug)]
struct Inner {
    managed_dir: PathBuf,
    client: reqwest::Client,
    api_base: String,
    /// Extra homes to scan (tests); system locations are always scanned.
    scan_system: bool,
    cache: RwLock<Option<Vec<JavaRuntime>>>,
    install_lock: Mutex<()>,
}

impl JavaRuntimes {
    pub fn new(managed_dir: PathBuf, client: reqwest::Client) -> Self {
        Self::with_api(managed_dir, client, ADOPTIUM_API.to_owned(), true)
    }

    /// For tests: a different API base and optionally no system scan.
    pub fn with_api(
        managed_dir: PathBuf,
        client: reqwest::Client,
        api_base: String,
        scan_system: bool,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                managed_dir,
                client,
                api_base,
                scan_system,
                cache: RwLock::new(None),
                install_lock: Mutex::new(()),
            }),
        }
    }

    pub fn managed_dir(&self) -> &Path {
        &self.inner.managed_dir
    }

    /// All runtimes, managed first. Scans once and caches; `refresh` rescans.
    pub async fn list(&self, refresh: bool) -> Vec<JavaRuntime> {
        if !refresh && let Some(cached) = self.inner.cache.read().await.as_ref() {
            return cached.clone();
        }
        let managed_dir = self.inner.managed_dir.clone();
        let scan_system = self.inner.scan_system;
        let found = tokio::task::spawn_blocking(move || detect::scan(&managed_dir, scan_system))
            .await
            .unwrap_or_default();
        *self.inner.cache.write().await = Some(found.clone());
        found
    }

    /// A runtime with exactly this major version (managed preferred).
    pub async fn find(&self, major: u32) -> Option<JavaRuntime> {
        self.list(false)
            .await
            .into_iter()
            .find(|runtime| runtime.major == major)
    }

    /// Returns a runtime of this major, downloading Temurin if none is installed.
    pub async fn ensure(&self, major: u32, job: &JobContext) -> Result<JavaRuntime, JavaError> {
        if let Some(runtime) = self.find(major).await {
            return Ok(runtime);
        }
        // Serialise installs so two jobs needing the same Java don't download it twice.
        let _guard = self.inner.install_lock.lock().await;
        if let Some(runtime) = self.find(major).await {
            return Ok(runtime);
        }
        let runtime = adoptium::install(
            &self.inner.client,
            &self.inner.api_base,
            &self.inner.managed_dir,
            major,
            job,
        )
        .await?;
        self.list(true).await;
        Ok(runtime)
    }
}
