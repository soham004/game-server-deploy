//! Taking a backup: check the disk, hold a running world still, write the archive, record it,
//! then apply the instance's retention policy.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};

use super::{
    BackupService,
    archive::{self, Selection, WrittenArchive},
    error::BackupError,
    model::{BackupRecord, BackupScope, Manifest},
    quiesce::{self, SavingPaused},
    retention,
    service::BackupRequest,
    space,
};
use crate::{
    events::EventKind,
    game::GameAdapter,
    ids::BackupId,
    instances::Instance,
    jobs::JobContext,
    supervisor::{ServerState, SupervisorHandle},
};

/// A running server whose saving is paused for the backup.
struct Paused {
    saving: SavingPaused<SupervisorHandle>,
    handle: SupervisorHandle,
    /// When the server started; a different value afterwards means it restarted meanwhile.
    running_since: DateTime<Utc>,
}

impl BackupService {
    /// Backs up `instance` from inside a job that holds the instance's lock, then deletes the
    /// backups its retention policy no longer keeps.
    pub async fn create(
        &self,
        instance: &Instance,
        request: &BackupRequest,
        job: &JobContext,
    ) -> Result<BackupRecord, BackupError> {
        let record = self.back_up(instance, request, job).await?;
        self.apply_retention(instance, job).await;
        Ok(record)
    }

    /// [`Self::create`] without retention. A restore uses it for its safety backup, which must
    /// not expire the very backup being restored.
    pub(super) async fn back_up(
        &self,
        instance: &Instance,
        request: &BackupRequest,
        job: &JobContext,
    ) -> Result<BackupRecord, BackupError> {
        self.ensure_backup_possible(instance)?;
        let adapter = self.inner.instances.adapter(instance)?;
        let server_dir = instance.layout.server_dir();
        let selection = selection_for(request.scope, adapter.as_ref(), &server_dir);
        let folder = self.prepare_folder(instance).await?;
        self.check_space(instance, request.scope, &selection, &folder)
            .await?;

        // One archive at a time; a queued backup waits here rather than failing.
        let _permit = self.inner.archiving.acquire().await;
        let paused = self.pause_saving(instance, adapter.as_ref(), job).await?;
        job.step("Writing the archive");
        let written = self.write(instance, request, selection, &folder, job).await;
        let written = self.finish_saving(paused, written, job).await?;

        let record = self.record(instance, request, written).await?;
        self.inner
            .events
            .publish(Some(instance.id()), EventKind::BackupsChanged);
        Ok(record)
    }

    /// The instance's backup folder (root-only), without archives left over from a crash.
    async fn prepare_folder(&self, instance: &Instance) -> Result<PathBuf, BackupError> {
        let folder = self.instance_dir(instance.id());
        let created = folder.clone();
        tokio::task::spawn_blocking(move || -> std::io::Result<()> {
            std::fs::create_dir_all(&created)?;
            restrict_to_owner(&created)?;
            for entry in std::fs::read_dir(&created)? {
                let path = entry?.path();
                if path.extension().is_some_and(|ext| ext == "partial") {
                    std::fs::remove_file(path)?;
                }
            }
            Ok(())
        })
        .await
        .map_err(|e| BackupError::io("prepare the backup folder")(std::io::Error::other(e)))?
        .map_err(BackupError::io("prepare the backup folder"))?;
        Ok(folder)
    }

    /// Estimates the archive from the last backup of the same scope (or the files' size) and
    /// refuses when the disk can't take it.
    async fn check_space(
        &self,
        instance: &Instance,
        scope: BackupScope,
        selection: &Selection,
        folder: &Path,
    ) -> Result<(), BackupError> {
        let previous = self
            .inner
            .repo
            .list(instance.id())
            .await?
            .into_iter()
            .find(|backup| backup.scope == scope)
            .map(|backup| backup.size_bytes);
        let estimate = if let Some(size) = previous {
            size
        } else {
            let (server_dir, selection) = (instance.layout.server_dir(), selection.clone());
            tokio::task::spawn_blocking(move || archive::measure(&server_dir, &selection))
                .await
                .map_err(|e| BackupError::io("measure the server files")(std::io::Error::other(e)))?
                .map_err(BackupError::io("measure the server files"))?
        };
        let needed = space::with_margin(estimate);
        match space::shortfall(folder, needed) {
            Some(available) => Err(BackupError::NoSpace {
                needed_mb: needed.div_ceil(1024 * 1024),
                available_mb: available / (1024 * 1024),
            }),
            None => Ok(()),
        }
    }

    /// For a running server: turn saving off and flush the world. `None` when it isn't running.
    async fn pause_saving(
        &self,
        instance: &Instance,
        adapter: &dyn GameAdapter,
        job: &JobContext,
    ) -> Result<Option<Paused>, BackupError> {
        let Some(handle) = self.inner.instances.supervisors().get(instance.id()) else {
            return Ok(None);
        };
        let ServerState::Running { since, .. } = handle.state() else {
            return Ok(None);
        };
        let Some(commands) = adapter.save_commands() else {
            job.log("This game can't pause saving; the backup is taken while it runs.");
            return Ok(None);
        };
        job.step("Saving the world");
        let saving = quiesce::pause_saving(
            handle.clone(),
            &self.inner.events,
            instance.id(),
            commands,
            self.save_timeout,
        )
        .await?;
        Ok(Some(Paused {
            saving,
            handle,
            running_since: since,
        }))
    }

    /// Writes the archive next to its final name, then renames it into place.
    async fn write(
        &self,
        instance: &Instance,
        request: &BackupRequest,
        selection: Selection,
        folder: &Path,
        job: &JobContext,
    ) -> Result<(PathBuf, WrittenArchive), BackupError> {
        let created_at = Utc::now();
        let final_path = unused_path(folder, &file_name(created_at, request));
        let partial = final_path.with_extension("zip.partial");
        let manifest = self.manifest(instance, request, created_at);
        let server_dir = instance.layout.server_dir();
        let cancel = job.cancel_token().clone();
        let (from, to) = (partial.clone(), final_path.clone());
        tokio::task::spawn_blocking(move || {
            let written = archive::write_archive(&server_dir, &selection, &from, manifest, &cancel)?;
            std::fs::rename(&from, &to)?;
            Ok(written)
        })
        .await
        .map_err(|e| BackupError::io("write the archive")(std::io::Error::other(e)))?
        .map(|written| (final_path, written))
        .map_err(|error: std::io::Error| {
            let _ = std::fs::remove_file(&partial);
            BackupError::io("write the archive")(error)
        })
    }

    /// Turns saving back on. If the server stopped or restarted while the files were being
    /// read, they may have changed mid-copy, so the archive is discarded.
    async fn finish_saving(
        &self,
        paused: Option<Paused>,
        written: Result<(PathBuf, WrittenArchive), BackupError>,
        job: &JobContext,
    ) -> Result<(PathBuf, WrittenArchive), BackupError> {
        let Some(Paused {
            saving,
            handle,
            running_since,
        }) = paused
        else {
            return written;
        };
        let still_running = matches!(
            handle.state(),
            ServerState::Running { since, .. } if since == running_since
        );
        if let Err(error) = saving.resume().await {
            job.log(format!("Could not turn saving back on: {error}"));
        }
        let (path, archive) = written?;
        if !still_running {
            let _ = tokio::fs::remove_file(&path).await;
            return Err(BackupError::ServerChanged);
        }
        Ok((path, archive))
    }

    async fn record(
        &self,
        instance: &Instance,
        request: &BackupRequest,
        (path, written): (PathBuf, WrittenArchive),
    ) -> Result<BackupRecord, BackupError> {
        let manifest = written.manifest;
        let record = BackupRecord {
            id: manifest.backup_id,
            instance_id: instance.id(),
            kind: request.kind,
            scope: request.scope,
            file_name: path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default(),
            size_bytes: written.size_bytes,
            sha256: written.sha256,
            software: manifest.software,
            note: request.note.clone(),
            pinned: false,
            created_at: manifest.created_at,
            created_by: request.by,
        };
        if let Err(error) = self.inner.repo.insert(&record).await {
            let _ = tokio::fs::remove_file(&path).await;
            return Err(error.into());
        }
        Ok(record)
    }

    /// Deletes backups beyond the instance's policy. Failures are logged, not fatal: the new
    /// backup is safe either way.
    pub(super) async fn apply_retention(&self, instance: &Instance, job: &JobContext) {
        let backups = match self.inner.repo.list(instance.id()).await {
            Ok(backups) => backups,
            Err(error) => {
                job.log(format!("Could not check old backups: {error}"));
                return;
            }
        };
        let expired = retention::expired(&backups, &instance.definition.backups);
        for backup in backups.iter().filter(|backup| expired.contains(&backup.id)) {
            match self.remove(backup).await {
                Ok(()) => job.log(format!("Deleted the old backup {}", backup.file_name)),
                Err(error) => job.log(format!(
                    "Could not delete the old backup {}: {error}",
                    backup.file_name
                )),
            }
        }
        if !expired.is_empty() {
            self.inner
                .events
                .publish(Some(instance.id()), EventKind::BackupsChanged);
        }
    }

    fn manifest(&self, instance: &Instance, request: &BackupRequest, created_at: DateTime<Utc>) -> Manifest {
        let definition = &instance.definition;
        let software = self
            .inner
            .instances
            .adapter(instance)
            .ok()
            .and_then(|adapter| adapter.software(&definition.server));
        Manifest {
            format: Manifest::FORMAT,
            backup_id: BackupId::generate(),
            instance_id: definition.id,
            instance_name: definition.name.clone(),
            game: definition.game.clone(),
            software,
            kind: request.kind,
            scope: request.scope,
            created_at,
            note: request.note.clone(),
            entries: vec![],
            files: 0,
            folders: 0,
            bytes: 0,
        }
    }
}

/// What goes into a backup of `scope`.
pub(super) fn selection_for(scope: BackupScope, adapter: &dyn GameAdapter, server_dir: &Path) -> Selection {
    match scope {
        BackupScope::World => Selection::Only(adapter.world_folders(server_dir)),
        BackupScope::Full => Selection::AllExcept(
            adapter
                .backup_excludes()
                .iter()
                .map(|name| (*name).to_owned())
                .collect(),
        ),
    }
}

/// `20261108-031452-manual-world.zip`: sorts by time, says what it is.
fn file_name(created_at: DateTime<Utc>, request: &BackupRequest) -> String {
    format!(
        "{}-{}-{}.zip",
        created_at.format("%Y%m%d-%H%M%S"),
        request.kind,
        request.scope
    )
}

/// `name`, or `name` with `-2`, `-3`, … before `.zip` if taken (two backups in one second).
/// Writing never overwrites, so even the unlikely fallback can't replace an archive.
fn unused_path(folder: &Path, name: &str) -> PathBuf {
    let stem = name.trim_end_matches(".zip");
    std::iter::once(folder.join(name))
        .chain((2..1000).map(|n| folder.join(format!("{stem}-{n}.zip"))))
        .find(|path| !path.exists())
        .unwrap_or_else(|| folder.join(name))
}

/// Only the app (root) may enter: backups hold every world, staging folders the next ones.
pub(super) fn restrict_to_owner(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use crate::backups::model::BackupKind;

    #[test]
    fn file_names_sort_by_time_and_never_collide() {
        let dir = tempfile::tempdir().unwrap();
        let request = BackupRequest {
            scope: BackupScope::World,
            kind: BackupKind::PreRestore,
            note: None,
            by: None,
        };
        let at = DateTime::parse_from_rfc3339("2026-11-08T03:14:52Z")
            .unwrap()
            .to_utc();
        let name = file_name(at, &request);
        assert_eq!(name, "20261108-031452-pre_restore-world.zip");

        assert_eq!(unused_path(dir.path(), &name), dir.path().join(&name));
        std::fs::write(dir.path().join(&name), "").unwrap();
        std::fs::write(dir.path().join("20261108-031452-pre_restore-world-2.zip"), "").unwrap();
        assert_eq!(
            unused_path(dir.path(), &name),
            dir.path().join("20261108-031452-pre_restore-world-3.zip")
        );
    }
}
