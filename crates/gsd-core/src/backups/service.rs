//! The backup service: listing, creating, restoring, pinning and deleting an instance's backups.
//!
//! Creating and restoring run as jobs that hold the instance's lock; the steps themselves are in
//! `create.rs` and `restore.rs`. [`BackupService::create`] is public so the scheduler can take a
//! backup inside its own job.

use std::{path::PathBuf, sync::Arc, time::Duration};

use sqlx::SqlitePool;
use tokio::sync::Semaphore;

use super::{
    error::{BackupError, MAX_NOTE_CHARS},
    model::{BackupKind, BackupRecord, BackupScope},
    repo::BackupRepo,
};
use crate::{
    events::{EventBus, EventKind},
    ids::{BackupId, InstanceId, JobId, UserId},
    instances::{Instance, InstanceService},
    jobs::{JobRunner, JobSpec},
    supervisor::ServerState,
};

/// How long a running server gets to write its world to disk before a backup gives up.
const SAVE_TIMEOUT: Duration = Duration::from_secs(300);

/// What to back up, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackupRequest {
    pub scope: BackupScope,
    pub kind: BackupKind,
    pub note: Option<String>,
    pub by: Option<UserId>,
}

/// How to restore.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RestoreOptions {
    /// Back up the current files first (a `pre_restore` backup), so the restore can be undone.
    pub safety_backup: bool,
}

/// A change to a backup's pin or note; `None` leaves it as it is.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BackupChange {
    pub pinned: Option<bool>,
    /// `Some(None)` removes the note.
    pub note: Option<Option<String>>,
}

#[derive(Debug, Clone)]
pub struct BackupService {
    pub(super) inner: Arc<Inner>,
    /// How long a running server gets to confirm its world save.
    pub(super) save_timeout: Duration,
}

#[derive(Debug)]
pub(super) struct Inner {
    pub(super) repo: BackupRepo,
    pub(super) instances: InstanceService,
    pub(super) jobs: JobRunner,
    pub(super) events: EventBus,
    /// `<data>/backups`: one folder per instance id.
    pub(super) dir: PathBuf,
    /// Writing an archive is disk- and CPU-heavy: one at a time across all servers.
    pub(super) archiving: Semaphore,
}

impl BackupService {
    pub fn new(
        pool: SqlitePool,
        instances: InstanceService,
        jobs: JobRunner,
        events: EventBus,
        dir: PathBuf,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                repo: BackupRepo::new(pool),
                instances,
                jobs,
                events,
                dir,
                archiving: Semaphore::new(1),
            }),
            save_timeout: SAVE_TIMEOUT,
        }
    }

    /// Shorter wait for the world save (tests).
    #[must_use]
    pub fn with_save_timeout(mut self, timeout: Duration) -> Self {
        self.save_timeout = timeout;
        self
    }

    /// An instance's backups, newest first.
    pub async fn list(&self, id: InstanceId) -> Result<Vec<BackupRecord>, BackupError> {
        self.inner.instances.get(id).await?;
        Ok(self.inner.repo.list(id).await?)
    }

    pub async fn get(&self, id: InstanceId, backup: BackupId) -> Result<BackupRecord, BackupError> {
        self.inner
            .repo
            .get(id, backup)
            .await?
            .ok_or(BackupError::NotFound)
    }

    /// The archive of a backup, for downloading.
    pub async fn archive(
        &self,
        id: InstanceId,
        backup: BackupId,
    ) -> Result<(BackupRecord, PathBuf), BackupError> {
        let record = self.get(id, backup).await?;
        let path = self.archive_path(&record);
        if !path.is_file() {
            return Err(BackupError::FileMissing);
        }
        Ok((record, path))
    }

    /// Checks the request, then backs up in a job.
    pub async fn start_backup(&self, id: InstanceId, request: BackupRequest) -> Result<JobId, BackupError> {
        let instance = self.inner.instances.get(id).await?;
        let request = BackupRequest {
            note: clean_note(request.note)?,
            ..request
        };
        self.ensure_backup_possible(&instance)?;
        let spec = JobSpec {
            kind: "backup.create",
            title: format!("Backing up {}", instance.definition.name),
            instance_id: Some(id),
        };
        let service = self.clone();
        Ok(self.inner.jobs.spawn(spec, move |job| async move {
            let record = service.create(&instance, &request, &job).await?;
            job.log(format!(
                "Saved {} ({} bytes)",
                record.file_name, record.size_bytes
            ));
            Ok(())
        })?)
    }

    /// Checks the backup exists, then restores it in a job.
    pub async fn start_restore(
        &self,
        id: InstanceId,
        backup: BackupId,
        options: RestoreOptions,
        by: Option<UserId>,
    ) -> Result<JobId, BackupError> {
        let instance = self.inner.instances.get(id).await?;
        let (record, _) = self.archive(id, backup).await?;
        let spec = JobSpec {
            kind: "backup.restore",
            title: format!(
                "Restoring {} from {}",
                instance.definition.name,
                record.created_at.format("%Y-%m-%d %H:%M UTC")
            ),
            instance_id: Some(id),
        };
        let service = self.clone();
        Ok(self.inner.jobs.spawn(spec, move |job| async move {
            service.restore(&instance, &record, options, by, &job).await?;
            Ok(())
        })?)
    }

    /// Pins or unpins a backup, or changes its note.
    pub async fn update(
        &self,
        id: InstanceId,
        backup: BackupId,
        change: BackupChange,
    ) -> Result<BackupRecord, BackupError> {
        let current = self.get(id, backup).await?;
        let pinned = change.pinned.unwrap_or(current.pinned);
        let note = match change.note {
            Some(note) => clean_note(note)?,
            None => current.note,
        };
        self.inner
            .repo
            .update(id, backup, pinned, note.as_deref())
            .await?;
        self.inner.events.publish(Some(id), EventKind::BackupsChanged);
        self.get(id, backup).await
    }

    /// Deletes a backup and its archive. Refused while a job (such as a restore) holds the instance.
    pub async fn delete(&self, id: InstanceId, backup: BackupId) -> Result<(), BackupError> {
        let record = self.get(id, backup).await?;
        if self.inner.instances.is_busy(id) {
            return Err(BackupError::Instance(crate::instances::InstanceError::Busy));
        }
        self.remove(&record).await?;
        self.inner.events.publish(Some(id), EventKind::BackupsChanged);
        Ok(())
    }

    /// Deletes the backups of a server that was deleted: their records go, and the folder of
    /// archives moves to `trash` (named after the server), so it can still be recovered until the
    /// trash is emptied. Returns how many backups there were.
    pub async fn discard_all(
        &self,
        id: InstanceId,
        slug: &str,
        trash: &std::path::Path,
    ) -> Result<u64, BackupError> {
        let count = self.inner.repo.delete_all(id).await?;
        let folder = self.instance_dir(id);
        if folder.exists() {
            let target = trash.join(crate::trash::entry_name(
                &format!("{slug}-backups"),
                chrono::Utc::now(),
            ));
            tokio::fs::create_dir_all(trash)
                .await
                .map_err(BackupError::io("create the trash folder"))?;
            tokio::fs::rename(&folder, &target)
                .await
                .map_err(BackupError::io("move the backups to the trash"))?;
        }
        Ok(count)
    }

    pub(super) fn instance_dir(&self, id: InstanceId) -> PathBuf {
        self.inner.dir.join(id.to_string())
    }

    pub(super) fn archive_path(&self, record: &BackupRecord) -> PathBuf {
        self.instance_dir(record.instance_id).join(&record.file_name)
    }

    /// Deletes the archive (if still there) and the record.
    pub(super) async fn remove(&self, record: &BackupRecord) -> Result<(), BackupError> {
        match tokio::fs::remove_file(self.archive_path(record)).await {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                return Err(BackupError::io(format!("delete {}", record.file_name))(error));
            }
            _ => {}
        }
        self.inner.repo.delete(record.instance_id, record.id).await?;
        Ok(())
    }

    /// A running server can be backed up and a stopped one too; one whose files are about to
    /// change can't.
    pub(super) fn ensure_backup_possible(&self, instance: &Instance) -> Result<(), BackupError> {
        if instance.definition.installed_at.is_none() {
            return Err(BackupError::Instance(
                crate::instances::InstanceError::NotInstalled,
            ));
        }
        match self.inner.instances.state(instance.id()) {
            ServerState::Starting { .. } => Err(BackupError::ServerBusy("starting")),
            ServerState::Stopping { .. } => Err(BackupError::ServerBusy("stopping")),
            ServerState::Crashed {
                restart_at: Some(_), ..
            } => Err(BackupError::ServerBusy("about to restart after a crash")),
            _ => Ok(()),
        }
    }
}

/// Trims a note; empty means none.
fn clean_note(note: Option<String>) -> Result<Option<String>, BackupError> {
    let Some(note) = note else {
        return Ok(None);
    };
    let note = note.trim();
    if note.chars().count() > MAX_NOTE_CHARS || note.chars().any(char::is_control) {
        return Err(BackupError::InvalidNote);
    }
    Ok((!note.is_empty()).then(|| note.to_owned()))
}
