//! What can go wrong with a backup or a restore, in terms the user can act on.

use std::io;

use super::quiesce::QuiesceError;
use crate::{files::FilesError, instances::InstanceError, jobs::JobError, store::RepoError};

/// Longest note a backup can carry.
pub const MAX_NOTE_CHARS: usize = 200;

#[derive(Debug, thiserror::Error)]
pub enum BackupError {
    #[error("backup not found")]
    NotFound,
    #[error(transparent)]
    Instance(#[from] InstanceError),
    /// Starting, stopping or about to restart after a crash: its files are changing.
    #[error("the server is {0}; try again once it is running or stopped")]
    ServerBusy(&'static str),
    #[error("not enough disk space: about {needed_mb} MB are needed and {available_mb} MB are free")]
    NoSpace { needed_mb: u64, available_mb: u64 },
    #[error("could not hold the world still for the backup")]
    Quiesce(#[from] QuiesceError),
    #[error(
        "the server stopped or restarted during the backup, so the copy could be inconsistent; it was discarded"
    )]
    ServerChanged,
    #[error("the backup file is missing from the backup folder")]
    FileMissing,
    #[error("the backup file is damaged ({0}), so it can't be restored")]
    Damaged(String),
    #[error("the note must be at most {MAX_NOTE_CHARS} characters")]
    InvalidNote,
    #[error("could not {action}")]
    Io {
        action: String,
        #[source]
        source: io::Error,
    },
    #[error("could not unpack the backup")]
    Unpack(#[from] FilesError),
    #[error(transparent)]
    Repo(#[from] RepoError),
    #[error(transparent)]
    Job(#[from] JobError),
}

impl BackupError {
    pub(crate) fn io(action: impl Into<String>) -> impl FnOnce(io::Error) -> Self {
        let action = action.into();
        move |source| Self::Io { action, source }
    }
}
