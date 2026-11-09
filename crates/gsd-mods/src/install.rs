//! Carrying out an [`InstallPlan`] in a server's content folder: each file is downloaded once
//! into the shared content cache (checked against the source's hash), copied into the folder
//! through the file manager (so it belongs to the server's user and can't be redirected by a
//! planted symlink), and recorded in the packwiz index. A replaced file goes to the trash.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use gsd_core::{
    files::{FilesError, SafeRelPath, ServerFiles},
    jobs::JobContext,
    net::{ExpectedHash, download_verified},
};
use tokio_util::io::ReaderStream;

use crate::{
    error::ContentError,
    index::{self, INDEX_FOLDER, IndexEntry},
    installed::{DISABLED, read_entries},
    model::{ContentFile, HashFormat},
    resolver::{InstallPlan, PlannedInstall},
};

/// Installs every step of `plan` into `folder` (`mods` or `plugins`).
pub async fn run(
    files: &ServerFiles,
    folder: &str,
    plan: &InstallPlan,
    client: &reqwest::Client,
    cache_dir: &Path,
    job: &JobContext,
) -> Result<(), ContentError> {
    let folder = SafeRelPath::parse(folder)?;
    let index_folder = folder.join(INDEX_FOLDER)?;
    ensure_folder(files, &folder).await?;
    ensure_folder(files, &index_folder).await?;
    let existing = read_entries(files, &index_folder).await;
    for step in &plan.steps {
        job.check_cancelled().map_err(|_| ContentError::Cancelled)?;
        if step.version.file.manual_url.is_some() {
            job.log(format!(
                "{}: its author only allows downloading it by hand, so upload {} yourself",
                step.project.title, step.version.file.filename
            ));
            continue;
        }
        job.step(format!(
            "Installing {} {}",
            step.project.title, step.version.version_number
        ));
        let cached = download(client, &step.version.file, cache_dir, job).await?;
        place(files, &folder, step, &cached).await?;
        write_index(files, &index_folder, step).await?;
        remove_old_entries(files, &index_folder, step, &existing).await?;
        job.log(format!("Installed {}", step.version.file.filename));
    }
    Ok(())
}

/// One file has one index entry. The entry of a replaced file (or of another project with the same
/// file name) may have another name than the one just written: the plugin came from another
/// source, or its slug changed. Left behind, it would claim the file.
async fn remove_old_entries(
    files: &ServerFiles,
    index_folder: &SafeRelPath,
    step: &PlannedInstall,
    existing: &HashMap<String, (String, IndexEntry)>,
) -> Result<(), ContentError> {
    let current = index::file_name(&step.project.slug);
    let described = [Some(&step.version.file.filename), step.replaces.as_ref()];
    for file in described.into_iter().flatten() {
        if let Some((entry_name, _)) = existing.get(file).filter(|(name, _)| *name != current) {
            match files.delete(vec![index_folder.join(entry_name)?]).await {
                Ok(()) | Err(FilesError::NotFound(_)) => {}
                Err(error) => return Err(error.into()),
            }
        }
    }
    Ok(())
}

/// The file in the content cache (downloaded now if it isn't there yet).
pub(crate) async fn download(
    client: &reqwest::Client,
    file: &ContentFile,
    cache_dir: &Path,
    job: &JobContext,
) -> Result<PathBuf, ContentError> {
    let Some((format, key)) = file.best_hash() else {
        return Err(ContentError::NoChecksum(file.filename.clone()));
    };
    let expected = match format {
        HashFormat::Sha512 => ExpectedHash::Sha512(key.to_owned()),
        HashFormat::Sha256 => ExpectedHash::Sha256(key.to_owned()),
        HashFormat::Sha1 => ExpectedHash::Sha1(key.to_owned()),
    };
    let cached = cache_dir.join(format!("{key}.jar"));
    if tokio::fs::try_exists(&cached).await.unwrap_or(false) {
        return Ok(cached);
    }
    tokio::fs::create_dir_all(cache_dir)
        .await
        .map_err(|source| ContentError::Cache {
            path: cache_dir.to_owned(),
            source,
        })?;
    let progress = |done: u64, total: Option<u64>| {
        if let Some(total) = total.filter(|total| *total > 0) {
            #[allow(clippy::cast_precision_loss)]
            job.progress(done as f32 / total as f32);
        }
    };
    download_verified(
        client,
        &file.url,
        &cached,
        &expected,
        job.cancel_token(),
        &progress,
    )
    .await?;
    Ok(cached)
}

/// Copies the cached file into the folder, then removes the file it replaces.
async fn place(
    files: &ServerFiles,
    folder: &SafeRelPath,
    step: &PlannedInstall,
    cached: &Path,
) -> Result<(), ContentError> {
    let destination = folder.join(&step.version.file.filename)?;
    let source = tokio::fs::File::open(cached)
        .await
        .map_err(|source| ContentError::Cache {
            path: cached.to_owned(),
            source,
        })?;
    files
        .upload(destination, ReaderStream::new(source), true, u64::MAX)
        .await?;
    if let Some(old) = step
        .replaces
        .as_ref()
        .filter(|old| **old != step.version.file.filename)
    {
        for name in [old.clone(), format!("{old}{DISABLED}")] {
            match files.delete(vec![folder.join(&name)?]).await {
                Ok(()) | Err(FilesError::NotFound(_)) => {}
                Err(error) => return Err(error.into()),
            }
        }
    }
    Ok(())
}

/// Writes (or replaces) the step's `.pw.toml`.
async fn write_index(
    files: &ServerFiles,
    index_folder: &SafeRelPath,
    step: &PlannedInstall,
) -> Result<(), ContentError> {
    let entry = IndexEntry::new(&step.project, &step.version);
    write_entry(files, index_folder, &step.project.slug, &entry).await
}

/// Writes `<index_folder>/<slug>.pw.toml`, replacing what is there.
pub async fn write_entry(
    files: &ServerFiles,
    index_folder: &SafeRelPath,
    slug: &str,
    entry: &IndexEntry,
) -> Result<(), ContentError> {
    ensure_folder(files, index_folder).await?;
    let path = index_folder.join(&index::file_name(slug))?;
    let revision = match files.read_text(path.clone()).await {
        Ok(existing) => Some(existing.revision),
        Err(FilesError::NotFound(_)) => None,
        Err(error) => return Err(error.into()),
    };
    files.write_text(path, entry.to_toml(), revision).await?;
    Ok(())
}

async fn ensure_folder(files: &ServerFiles, path: &SafeRelPath) -> Result<(), ContentError> {
    match files.create_dir(path.clone()).await {
        Ok(()) | Err(FilesError::AlreadyExists(_)) => Ok(()),
        Err(error) => Err(error.into()),
    }
}
