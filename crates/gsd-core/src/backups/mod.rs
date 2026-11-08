//! Backups: zip archives of a server's worlds (or its whole folder) and putting them back.
//!
//! - `service.rs`: the API (list, start a backup or restore job, pin, note, delete).
//! - `create.rs`: taking a backup. A running server's saving is paused and its world flushed
//!   (`quiesce.rs`) while `archive.rs` writes the zip; retention (`retention.rs`) runs after.
//! - `restore.rs`: verifying, stopping the server, a safety backup, unpacking into a staging
//!   folder and swapping the files in (`swap.rs`).
//! - `repo.rs`: the `backups` table; `space.rs`: disk-space checks; `model.rs`: the types.
//!
//! Archives live in `<data>/backups/<instance id>/` (root only); each holds a `gsd-backup.json`
//! describing it. See `claude/plan/features/backups.md` and decision D-035.

mod archive;
mod create;
mod error;
mod model;
mod quiesce;
mod repo;
mod restore;
mod retention;
mod service;
mod space;
mod swap;

pub use archive::{MANIFEST_NAME, read_manifest};
pub use error::{BackupError, MAX_NOTE_CHARS};
pub use model::{BackupKind, BackupRecord, BackupScope, Manifest};
pub use quiesce::QuiesceError;
pub use service::{BackupChange, BackupRequest, BackupService, RestoreOptions};
