//! Which backups to delete. Per instance, the newest `keep` manual and scheduled backups stay,
//! and separately the newest `keep_safety` safety backups (taken before a restore or a version
//! change). Pinned backups are never counted and never deleted.

use crate::{backups::model::BackupRecord, ids::BackupId, instances::BackupPolicy};

/// The backups of one instance that fall outside `policy`.
pub fn expired(backups: &[BackupRecord], policy: &BackupPolicy) -> Vec<BackupId> {
    let mut newest_first: Vec<&BackupRecord> = backups.iter().filter(|backup| !backup.pinned).collect();
    newest_first.sort_by(|a, b| b.created_at.cmp(&a.created_at).then(b.id.cmp(&a.id)));
    let (safety, regular): (Vec<&BackupRecord>, Vec<&BackupRecord>) = newest_first
        .into_iter()
        .partition(|backup| backup.kind.is_safety());
    regular
        .into_iter()
        .skip(usize::try_from(policy.keep).unwrap_or(usize::MAX))
        .chain(
            safety
                .into_iter()
                .skip(usize::try_from(policy.keep_safety).unwrap_or(usize::MAX)),
        )
        .map(|backup| backup.id)
        .collect()
}

#[cfg(test)]
mod tests {
    use chrono::{DateTime, Duration, Utc};

    use super::*;
    use crate::{
        backups::model::{BackupKind, BackupScope},
        ids::InstanceId,
    };

    fn backup(kind: BackupKind, hours_ago: i64, pinned: bool) -> BackupRecord {
        let created_at: DateTime<Utc> = Utc::now() - Duration::hours(hours_ago);
        BackupRecord {
            id: BackupId::generate(),
            instance_id: InstanceId::generate(),
            kind,
            scope: BackupScope::World,
            file_name: format!("{hours_ago}.zip"),
            size_bytes: 1,
            sha256: String::new(),
            software: None,
            note: None,
            pinned,
            created_at,
            created_by: None,
        }
    }

    fn policy(keep: u32, keep_safety: u32) -> BackupPolicy {
        BackupPolicy { keep, keep_safety }
    }

    #[test]
    fn keeps_the_newest_and_expires_the_rest_whatever_the_order() {
        let backups = vec![
            backup(BackupKind::Manual, 5, false),
            backup(BackupKind::Scheduled, 1, false),
            backup(BackupKind::Manual, 9, false),
            backup(BackupKind::Scheduled, 3, false),
        ];
        let expired = expired(&backups, &policy(2, 3));
        assert_eq!(expired, [backups[0].id, backups[2].id]);
    }

    #[test]
    fn pinned_backups_neither_count_nor_expire() {
        let backups = vec![
            backup(BackupKind::Manual, 1, true),
            backup(BackupKind::Manual, 2, false),
            backup(BackupKind::Manual, 30, true),
            backup(BackupKind::Manual, 3, false),
        ];
        assert_eq!(expired(&backups, &policy(1, 3)), [backups[3].id]);
    }

    #[test]
    fn safety_backups_have_their_own_limit() {
        let backups = vec![
            backup(BackupKind::PreRestore, 1, false),
            backup(BackupKind::PreRestore, 2, false),
            backup(BackupKind::PreVersionChange, 3, false),
            backup(BackupKind::Manual, 4, false),
        ];
        // A burst of restores doesn't push out the user's own backup, and vice versa.
        assert_eq!(expired(&backups, &policy(1, 2)), [backups[2].id]);
        assert_eq!(expired(&backups, &policy(10, 10)), Vec::<BackupId>::new());
    }
}
