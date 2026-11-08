//! Restoring a backup: verify the archive, stop the server, take a safety backup, unpack into a
//! staging folder, swap the files in, and start the server again if it was running.
//!
//! The staging folders (`restore/` and `restore-old/` in the instance folder) are outside the
//! server's reach and on the same filesystem as `server/`, so the swap is a set of renames
//! (see `swap.rs`). Never is anything unpacked over live files.

use std::path::{Path, PathBuf};

use chrono::Utc;

use super::{
    BackupService,
    archive::{self, SERVER_FOLDER},
    create::{restrict_to_owner, selection_for},
    error::{BackupError, MAX_NOTE_CHARS},
    model::{BackupKind, BackupRecord, Manifest},
    service::{BackupRequest, RestoreOptions},
    space, swap,
};
use crate::{
    files::{ExtractLimits, unpack_zip},
    ids::UserId,
    instances::{Instance, SandboxIds},
    jobs::JobContext,
};

impl BackupService {
    pub(super) async fn restore(
        &self,
        instance: &Instance,
        backup: &BackupRecord,
        options: RestoreOptions,
        by: Option<UserId>,
        job: &JobContext,
    ) -> Result<(), BackupError> {
        job.step("Checking the backup");
        let archive = self.archive_path(backup);
        let manifest = verify(&archive, backup, instance).await?;
        let staging = Staging::new(instance.layout.root());
        staging.check_space(&manifest)?;
        if let (Some(then), Some(now)) = (&manifest.software, self.software(instance))
            && then != &now
        {
            job.log(format!(
                "The backup was taken on {then}; the server now runs {now}."
            ));
        }

        job.step("Stopping the server");
        let was_running = self.inner.instances.stop_and_wait(instance.id()).await?;
        if options.safety_backup {
            job.step("Backing up the current files");
            self.back_up(instance, &safety_request(backup, by), job).await?;
        }

        job.step("Unpacking the backup");
        staging
            .unpack(&archive, &manifest, instance.definition.sandbox)
            .await?;
        job.step("Replacing the files");
        let adapter = self.inner.instances.adapter(instance)?;
        let server_dir = instance.layout.server_dir();
        let replaced = selection_for(backup.scope, adapter.as_ref(), &server_dir);
        let summary = staging
            .swap_into(server_dir, move |name| replaced.includes(name))
            .await?;
        job.log(format!(
            "Restored {} entries; {} current entries were replaced",
            summary.restored, summary.replaced
        ));
        staging.clean_up().await;
        self.apply_retention(instance, job).await;

        if was_running {
            job.step("Starting the server");
            self.inner.instances.start_from_job(instance.id()).await?;
        }
        Ok(())
    }

    fn software(&self, instance: &Instance) -> Option<String> {
        let adapter = self.inner.instances.adapter(instance).ok()?;
        adapter.software(&instance.definition.server)
    }
}

fn safety_request(backup: &BackupRecord, by: Option<UserId>) -> BackupRequest {
    BackupRequest {
        scope: backup.scope,
        kind: BackupKind::PreRestore,
        note: Some(safety_note(backup)),
        by,
    }
}

/// Names the restored backup by its note when it has one (the UI shows times in local time, so
/// a UTC time here would read as a different backup), otherwise by its time.
fn safety_note(backup: &BackupRecord) -> String {
    let note = match &backup.note {
        Some(note) => format!("Before restoring \u{201c}{note}\u{201d}"),
        None => format!(
            "Before restoring the backup from {}",
            backup.created_at.format("%Y-%m-%d %H:%M UTC")
        ),
    };
    note.chars().take(MAX_NOTE_CHARS).collect()
}

/// The archive is there, intact (its checksum matches), and is this backup of this instance.
async fn verify(archive: &Path, backup: &BackupRecord, instance: &Instance) -> Result<Manifest, BackupError> {
    if !archive.is_file() {
        return Err(BackupError::FileMissing);
    }
    let path = archive.to_owned();
    let (sha256, manifest) = tokio::task::spawn_blocking(move || {
        let sha256 = archive::sha256_of(&path)?;
        archive::read_manifest(&path).map(|manifest| (sha256, manifest))
    })
    .await
    .map_err(|e| BackupError::io("read the backup")(std::io::Error::other(e)))?
    .map_err(|error| BackupError::Damaged(error.to_string()))?;
    if sha256 != backup.sha256 {
        return Err(BackupError::Damaged("its checksum doesn't match".into()));
    }
    if manifest.backup_id != backup.id || manifest.instance_id != instance.id() {
        return Err(BackupError::Damaged("it belongs to a different backup".into()));
    }
    Ok(manifest)
}

/// The folders a restore works in, inside the instance folder.
#[derive(Debug, Clone)]
struct Staging {
    /// The archive is unpacked here (`restore/server/…`).
    unpacked: PathBuf,
    /// Entries the restore replaced (`restore-old/`).
    old: PathBuf,
    /// Leftovers of an interrupted restore are moved here, never deleted.
    trash: PathBuf,
}

impl Staging {
    fn new(instance_root: &Path) -> Self {
        Self {
            unpacked: instance_root.join("restore"),
            old: instance_root.join("restore-old"),
            trash: instance_root.join("trash"),
        }
    }

    /// Unpacking needs room for every file (the swap itself is renames).
    fn check_space(&self, manifest: &Manifest) -> Result<(), BackupError> {
        let needed = space::with_margin(manifest.bytes);
        let folder = self.unpacked.parent().unwrap_or(&self.unpacked);
        match space::shortfall(folder, needed) {
            Some(available) => Err(BackupError::NoSpace {
                needed_mb: needed.div_ceil(1024 * 1024),
                available_mb: available / (1024 * 1024),
            }),
            None => Ok(()),
        }
    }

    async fn unpack(&self, archive: &Path, manifest: &Manifest, ids: SandboxIds) -> Result<(), BackupError> {
        let (staging, archive) = (self.clone(), archive.to_owned());
        // The manifest says what the archive holds; anything bigger was tampered with.
        let limits = ExtractLimits {
            max_bytes: manifest.bytes + 1024 * 1024,
            max_entries: manifest.files + manifest.folders + 16,
        };
        tokio::task::spawn_blocking(move || -> Result<(), BackupError> {
            staging
                .set_aside_leftovers()
                .map_err(BackupError::io("clear the restore folder"))?;
            std::fs::create_dir(&staging.unpacked).map_err(BackupError::io("create the restore folder"))?;
            restrict_to_owner(&staging.unpacked).map_err(BackupError::io("protect the restore folder"))?;
            unpack_zip(&archive, &staging.unpacked, ids, limits)?;
            // A backup of an empty selection has no server folder.
            std::fs::create_dir_all(staging.unpacked.join(SERVER_FOLDER))
                .map_err(BackupError::io("create the restore folder"))?;
            Ok(())
        })
        .await
        .map_err(|e| BackupError::io("unpack the backup")(std::io::Error::other(e)))?
    }

    async fn swap_into(
        &self,
        server_dir: PathBuf,
        replaces: impl Fn(&str) -> bool + Send + 'static,
    ) -> Result<swap::SwapSummary, BackupError> {
        let staging = self.clone();
        tokio::task::spawn_blocking(move || {
            swap::swap_in(
                &server_dir,
                &staging.unpacked.join(SERVER_FOLDER),
                &staging.old,
                replaces,
            )
        })
        .await
        .map_err(|e| BackupError::io("replace the server files")(std::io::Error::other(e)))?
        .map_err(BackupError::io("replace the server files (nothing was changed)"))
    }

    /// Deletes both staging folders after a successful swap.
    async fn clean_up(&self) {
        for folder in [&self.unpacked, &self.old] {
            if let Err(error) = tokio::fs::remove_dir_all(folder).await {
                tracing::warn!(%error, folder = %folder.display(), "could not remove a restore folder");
            }
        }
    }

    /// An interrupted restore can leave the staging folders behind; `restore-old/` may hold
    /// the only copy of some files, so both go to the instance's trash instead of away.
    fn set_aside_leftovers(&self) -> std::io::Result<()> {
        let now = Utc::now();
        for folder in [&self.unpacked, &self.old] {
            if !folder.exists() {
                continue;
            }
            std::fs::create_dir_all(&self.trash)?;
            let name = folder
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            std::fs::rename(folder, self.trash.join(crate::trash::entry_name(&name, now)))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use chrono::DateTime;

    use super::*;
    use crate::{
        backups::model::BackupScope,
        ids::{BackupId, InstanceId},
    };

    fn backup(note: Option<&str>) -> BackupRecord {
        BackupRecord {
            id: BackupId::generate(),
            instance_id: InstanceId::generate(),
            kind: BackupKind::Manual,
            scope: BackupScope::World,
            file_name: "b.zip".into(),
            size_bytes: 1,
            sha256: String::new(),
            software: None,
            note: note.map(str::to_owned),
            pinned: false,
            created_at: DateTime::parse_from_rfc3339("2026-11-08T05:56:00Z")
                .unwrap_or_default()
                .to_utc(),
            created_by: None,
        }
    }

    #[test]
    fn the_safety_note_names_the_restored_backup() {
        assert_eq!(
            safety_note(&backup(Some("before the raid"))),
            "Before restoring \u{201c}before the raid\u{201d}"
        );
        assert_eq!(
            safety_note(&backup(None)),
            "Before restoring the backup from 2026-11-08 05:56 UTC"
        );
        let long = "x".repeat(MAX_NOTE_CHARS);
        assert_eq!(safety_note(&backup(Some(&long))).chars().count(), MAX_NOTE_CHARS);
    }
}
