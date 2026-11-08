//! What a backup is: its kind (why it was taken), its scope (what it holds) and its record.

use std::{fmt, str::FromStr};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::ids::{BackupId, InstanceId, UserId};

/// Why a backup was taken.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackupKind {
    /// Someone pressed "Back up now".
    Manual,
    /// Taken by a schedule.
    Scheduled,
    /// The state just before a restore replaced it.
    PreRestore,
    /// The state just before the server's version changed.
    PreVersionChange,
}

impl BackupKind {
    pub const ALL: [Self; 4] = [
        Self::Manual,
        Self::Scheduled,
        Self::PreRestore,
        Self::PreVersionChange,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Manual => "manual",
            Self::Scheduled => "scheduled",
            Self::PreRestore => "pre_restore",
            Self::PreVersionChange => "pre_version_change",
        }
    }

    /// Safety backups are taken automatically before a risky change; they have their own limit
    /// so a burst of restores can't push the user's own backups out.
    pub fn is_safety(self) -> bool {
        matches!(self, Self::PreRestore | Self::PreVersionChange)
    }
}

/// What a backup holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackupScope {
    /// Only the world folders. Restoring it replaces only those.
    World,
    /// The whole server folder except what the game recreates (logs, libraries, caches).
    Full,
}

impl BackupScope {
    pub const ALL: [Self; 2] = [Self::World, Self::Full];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::World => "world",
            Self::Full => "full",
        }
    }
}

impl fmt::Display for BackupKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for BackupKind {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|kind| kind.as_str() == text)
            .ok_or_else(|| format!("unknown backup kind {text:?}"))
    }
}

impl fmt::Display for BackupScope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for BackupScope {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|scope| scope.as_str() == text)
            .ok_or_else(|| format!("unknown backup scope {text:?}"))
    }
}

/// A finished backup. Only complete, verified archives get a record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackupRecord {
    pub id: BackupId,
    pub instance_id: InstanceId,
    pub kind: BackupKind,
    pub scope: BackupScope,
    /// The archive's file name inside the instance's backup folder.
    pub file_name: String,
    pub size_bytes: u64,
    /// SHA-256 of the archive, hex; checked before a restore.
    pub sha256: String,
    /// The server software when the backup was taken, e.g. "Paper 26.3".
    pub software: Option<String>,
    pub note: Option<String>,
    /// Pinned backups are never deleted automatically.
    pub pinned: bool,
    pub created_at: DateTime<Utc>,
    /// Who asked for it (`None` for schedules and the app itself).
    pub created_by: Option<UserId>,
}

/// The description stored inside every archive as `gsd-backup.json`, so an archive explains
/// itself even without the database (copied to another machine, or the database was lost).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    /// Version of this layout.
    pub format: u32,
    pub backup_id: BackupId,
    pub instance_id: InstanceId,
    pub instance_name: String,
    pub game: String,
    pub software: Option<String>,
    pub kind: BackupKind,
    pub scope: BackupScope,
    pub created_at: DateTime<Utc>,
    pub note: Option<String>,
    /// The top-level entries of the server folder that the archive holds (under `server/`).
    pub entries: Vec<String>,
    /// What the archive holds under `server/`: files, folders and the files' total size before
    /// compression. A restore refuses an archive that unpacks to more.
    pub files: u64,
    pub folders: u64,
    pub bytes: u64,
}

impl Manifest {
    pub const FORMAT: u32 = 1;
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn kinds_and_scopes_round_trip_through_text() {
        for kind in BackupKind::ALL {
            assert_eq!(kind.to_string().parse::<BackupKind>().unwrap(), kind);
            assert_eq!(serde_json::to_value(kind).unwrap(), kind.as_str());
        }
        for scope in BackupScope::ALL {
            assert_eq!(scope.to_string().parse::<BackupScope>().unwrap(), scope);
        }
        assert!("nightly".parse::<BackupKind>().is_err());
    }

    #[test]
    fn only_automatic_pre_change_backups_are_safety_backups() {
        let safety: Vec<_> = BackupKind::ALL.into_iter().filter(|k| k.is_safety()).collect();
        assert_eq!(safety, [BackupKind::PreRestore, BackupKind::PreVersionChange]);
    }
}
