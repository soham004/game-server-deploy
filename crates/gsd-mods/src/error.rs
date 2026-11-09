//! What can go wrong finding, installing or managing content, in terms the user can act on.

use std::path::PathBuf;

use gsd_core::{
    backups::BackupError,
    files::{FilesError, PathError},
    instances::InstanceError,
    jobs::JobError,
    net::DownloadError,
};

use crate::resolver::ResolveError;

#[derive(Debug, thiserror::Error)]
pub enum SourceError {
    #[error("could not reach the content source")]
    Request(#[source] reqwest::Error),
    #[error("the content source answered {status} for {url}")]
    Status { url: String, status: u16 },
    #[error("the content source sent something unexpected from {url}: {detail}")]
    Decode { url: String, detail: String },
    #[error("not found on the content source")]
    NotFound,
    #[error("the content source is busy (rate limit); try again in a minute")]
    RateLimited,
    #[error("CurseForge needs an API key; an owner can add one in Settings")]
    NoKey,
    #[error("the content source refused the API key; an owner can check it in Settings")]
    KeyRefused,
    /// Modrinth retires old API versions with HTTP 410.
    #[error("this version of the app uses an API the content source no longer offers; update the app")]
    ApiRetired,
}

impl SourceError {
    pub(crate) fn request(error: reqwest::Error) -> Self {
        Self::Request(error)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ContentError {
    #[error(
        "vanilla servers run neither mods nor plugins; use Fabric, Quilt, Forge, NeoForge, Paper or Purpur"
    )]
    NoContent,
    #[error("{0} is not installed")]
    NotInstalled(String),
    #[error("{0} has no checksum, so it can't be checked after downloading")]
    NoChecksum(String),
    #[error("{0} is not a jar file")]
    NotAJar(String),
    #[error("cancelled")]
    Cancelled,
    #[error("everything is up to date")]
    NothingToUpdate,
    #[error("could not back up the server first, so nothing was updated")]
    Backup(#[source] BackupError),
    #[error("could not use the download cache {path}")]
    Cache {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error(transparent)]
    Resolve(#[from] ResolveError),
    #[error(transparent)]
    Source(#[from] SourceError),
    #[error(transparent)]
    Download(#[from] DownloadError),
    #[error(transparent)]
    Files(#[from] FilesError),
    #[error(transparent)]
    Instance(#[from] InstanceError),
    #[error(transparent)]
    Job(#[from] JobError),
}

impl From<PathError> for ContentError {
    fn from(error: PathError) -> Self {
        Self::Files(error.into())
    }
}
