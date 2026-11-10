//! [`ServerFiles`]: async access to one instance's server directory, used by the HTTP layer.
//!
//! The work happens in [`Root`] on blocking threads; this adds streaming uploads (to a hidden
//! temporary file, renamed into place when complete) and streaming downloads (a file, or a folder
//! zipped on the fly).

use std::{io, sync::Arc};

use bytes::Bytes;
use cap_std::fs::OpenOptions;
use futures_util::{Stream, StreamExt};
use tokio::io::AsyncWriteExt;
use tokio_util::io::{ReaderStream, SyncIoBridge};

use super::{
    Entry, ExtractLimits, ExtractSummary, FilesError, SafeRelPath, TextFile,
    root::{Root, temp_sibling},
};
use crate::instances::Instance;

/// File access to one instance's server directory. Cheap to clone.
#[derive(Debug, Clone)]
pub struct ServerFiles {
    root: Arc<Root>,
}

impl ServerFiles {
    pub async fn open(instance: &Instance) -> Result<Self, FilesError> {
        let layout = instance.layout.clone();
        let ids = instance.definition.sandbox;
        let root = run_blocking(move || {
            Root::open(&layout, ids).map_err(|e| FilesError::io(&SafeRelPath::root(), e))
        })
        .await?;
        Ok(Self { root: Arc::new(root) })
    }

    pub async fn list(&self, path: SafeRelPath) -> Result<Vec<Entry>, FilesError> {
        self.run(move |root| root.list(&path)).await
    }

    pub async fn read_text(&self, path: SafeRelPath) -> Result<TextFile, FilesError> {
        self.run(move |root| root.read_text(&path)).await
    }

    /// See [`Root::write_text`]: `expected_revision` `None` creates a new file.
    pub async fn write_text(
        &self,
        path: SafeRelPath,
        content: String,
        expected_revision: Option<String>,
    ) -> Result<TextFile, FilesError> {
        self.run(move |root| root.write_text(&path, &content, expected_revision.as_deref()))
            .await
    }

    /// Bytes in the files under a folder; links aren't followed.
    pub async fn folder_size(&self, path: SafeRelPath) -> Result<u64, FilesError> {
        self.run(move |root| root.folder_size(&path)).await
    }

    /// A binary file and its revision (world settings); the same size limit as text.
    pub async fn read_bytes(&self, path: SafeRelPath) -> Result<(Vec<u8>, String), FilesError> {
        self.run(move |root| root.read_bytes(&path)).await
    }

    /// As [`Self::write_text`], for bytes: atomic, owned by the server's user, refused if the file
    /// changed since `expected_revision`.
    pub async fn write_bytes(
        &self,
        path: SafeRelPath,
        content: Vec<u8>,
        expected_revision: Option<String>,
    ) -> Result<(), FilesError> {
        self.run(move |root| root.write_bytes(&path, &content, expected_revision.as_deref()))
            .await
    }

    pub async fn create_dir(&self, path: SafeRelPath) -> Result<(), FilesError> {
        self.run(move |root| root.create_dir(&path)).await
    }

    pub async fn rename(&self, from: SafeRelPath, to: SafeRelPath) -> Result<(), FilesError> {
        self.run(move |root| root.rename(&from, &to)).await
    }

    pub async fn copy(&self, from: SafeRelPath, to: SafeRelPath) -> Result<(), FilesError> {
        self.run(move |root| root.copy(&from, &to)).await
    }

    /// Removes one file the app made itself, for good (see [`Root::remove_file`]).
    pub async fn remove_file(&self, path: SafeRelPath) -> Result<(), FilesError> {
        self.run(move |root| root.remove_file(&path)).await
    }

    /// Moves `paths` to the instance's trash.
    pub async fn delete(&self, paths: Vec<SafeRelPath>) -> Result<(), FilesError> {
        self.run(move |root| root.delete(&paths)).await
    }

    pub async fn extract(
        &self,
        archive: SafeRelPath,
        destination: SafeRelPath,
        limits: ExtractLimits,
    ) -> Result<ExtractSummary, FilesError> {
        self.run(move |root| root.extract(&archive, &destination, limits))
            .await
    }

    pub async fn compress(
        &self,
        sources: Vec<SafeRelPath>,
        destination: SafeRelPath,
    ) -> Result<(), FilesError> {
        self.run(move |root| root.compress(&sources, &destination)).await
    }

    /// Opens a file for download: the file and its size. A folder gives `IsADirectory` (use
    /// [`zip_folder`](Self::zip_folder)).
    pub async fn open_file(&self, path: SafeRelPath) -> Result<(tokio::fs::File, u64), FilesError> {
        let (file, size) = self
            .run(move |root| {
                let metadata = root
                    .dir
                    .metadata(path.as_path())
                    .map_err(|e| FilesError::io(&path, e))?;
                if metadata.is_dir() {
                    return Err(FilesError::IsADirectory(path));
                }
                let file = root
                    .dir
                    .open(path.as_path())
                    .map_err(|e| FilesError::io(&path, e))?;
                Ok((file.into_std(), metadata.len()))
            })
            .await?;
        Ok((tokio::fs::File::from_std(file), size))
    }

    /// A folder as a zip, produced while it is being sent. A failure midway ends the stream
    /// early (the download is then visibly incomplete) and is logged.
    pub fn zip_folder(&self, path: SafeRelPath) -> impl Stream<Item = io::Result<Bytes>> + use<> {
        let (reader, writer) = tokio::io::duplex(64 * 1024);
        let writer = SyncIoBridge::new(writer);
        let root = Arc::clone(&self.root);
        tokio::task::spawn_blocking(move || {
            if let Err(error) = root.zip_folder_to(&path, writer) {
                tracing::warn!(%path, %error, "zipping a folder for download failed");
            }
        });
        ReaderStream::new(reader)
    }

    /// Stores `body` as the file `path` (its folder must exist). At most `limit` bytes; an
    /// existing file is replaced only when `overwrite` is set. The file appears only once
    /// complete, owned by the instance's user.
    pub async fn upload<S, E>(
        &self,
        path: SafeRelPath,
        body: S,
        overwrite: bool,
        limit: u64,
    ) -> Result<Entry, FilesError>
    where
        S: Stream<Item = Result<Bytes, E>> + Unpin + Send,
        E: std::fmt::Display,
    {
        let (temp, file) = self.create_upload_file(&path, overwrite).await?;
        let written = write_stream(file, body, &path, limit).await;
        let finished = match written {
            Ok(()) => {
                let (temp, path) = (temp.clone(), path.clone());
                self.run(move |root| finish_upload(root, &temp, &path, overwrite))
                    .await
            }
            Err(error) => Err(error),
        };
        if finished.is_err() {
            let _ = self
                .run(move |root| Ok(root.dir.remove_file(temp.as_path())))
                .await;
        }
        finished
    }

    async fn create_upload_file(
        &self,
        path: &SafeRelPath,
        overwrite: bool,
    ) -> Result<(SafeRelPath, tokio::fs::File), FilesError> {
        let path = path.clone();
        let (temp, file) = self
            .run(move |root| {
                let name = path
                    .file_name()
                    .ok_or_else(|| FilesError::InvalidOperation("choose a file name".into()))?;
                root.open_dir(&path.parent().unwrap_or_default())?;
                if !overwrite && root.exists(&path) {
                    return Err(FilesError::AlreadyExists(path.clone()));
                }
                let temp = temp_sibling(&path, name)?;
                let mut options = OpenOptions::new();
                options.write(true).create_new(true);
                let file = root
                    .dir
                    .open_with(temp.as_path(), &options)
                    .map_err(|e| FilesError::io(&path, e))?;
                Ok((temp, file.into_std()))
            })
            .await?;
        Ok((temp, tokio::fs::File::from_std(file)))
    }

    async fn run<T, F>(&self, work: F) -> Result<T, FilesError>
    where
        T: Send + 'static,
        F: FnOnce(&Root) -> Result<T, FilesError> + Send + 'static,
    {
        let root = Arc::clone(&self.root);
        run_blocking(move || work(&root)).await
    }
}

async fn run_blocking<T, F>(work: F) -> Result<T, FilesError>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, FilesError> + Send + 'static,
{
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|error| FilesError::Io {
            path: SafeRelPath::root(),
            source: io::Error::other(format!("file task failed: {error}")),
        })?
}

async fn write_stream<S, E>(
    mut file: tokio::fs::File,
    mut body: S,
    path: &SafeRelPath,
    limit: u64,
) -> Result<(), FilesError>
where
    S: Stream<Item = Result<Bytes, E>> + Unpin,
    E: std::fmt::Display,
{
    let mut total: u64 = 0;
    while let Some(chunk) = body.next().await {
        let chunk = chunk.map_err(|e| FilesError::Io {
            path: path.clone(),
            source: io::Error::other(format!("the upload was interrupted: {e}")),
        })?;
        total += chunk.len() as u64;
        if total > limit {
            return Err(FilesError::TooLarge {
                path: path.clone(),
                limit,
            });
        }
        file.write_all(&chunk)
            .await
            .map_err(|e| FilesError::io(path, e))?;
    }
    file.flush().await.map_err(|e| FilesError::io(path, e))?;
    file.sync_all().await.map_err(|e| FilesError::io(path, e))
}

fn finish_upload(
    root: &Root,
    temp: &SafeRelPath,
    path: &SafeRelPath,
    overwrite: bool,
) -> Result<Entry, FilesError> {
    root.own(temp)?;
    match root.dir.symlink_metadata(path.as_path()) {
        Ok(_) if !overwrite => return Err(FilesError::AlreadyExists(path.clone())),
        Ok(existing) if existing.is_dir() => return Err(FilesError::IsADirectory(path.clone())),
        _ => {}
    }
    root.dir
        .rename(temp.as_path(), &root.dir, path.as_path())
        .map_err(|e| FilesError::io(path, e))?;
    root.entry(path)
}
