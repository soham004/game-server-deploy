//! A server's log history: `logs/` (the current log, the compressed ones of earlier runs, and the
//! debug log), `crash-reports/`, and the JVM's own `hs_err_pid*.log` in the server's folder.
//!
//! Only files found here can be read, by their plain name. That keeps `logs.read` to logs, since it
//! is a smaller right than `files.read`: someone who may read logs can't read `server.properties`.

mod crash;
mod read;

use chrono::{DateTime, Utc};
use gsd_core::files::{EntryKind, FilesError, SafeRelPath, ServerFiles};
use serde::Serialize;

pub use crash::{CrashSummary, summarize};
pub use read::{LogError, LogLine, LogPage, LogQuery, MAX_PAGE, read};

/// What a log file is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LogKind {
    /// The current or last run's log.
    Latest,
    /// An earlier run's log, usually compressed.
    Archive,
    /// The debug log some loaders write.
    Debug,
    /// A crash report the game wrote.
    Crash,
    /// A crash report the JVM wrote.
    JvmCrash,
}

impl LogKind {
    fn is_crash(self) -> bool {
        matches!(self, Self::Crash | Self::JvmCrash)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LogFile {
    pub name: String,
    pub kind: LogKind,
    pub size: u64,
    pub modified: Option<DateTime<Utc>>,
    #[serde(skip)]
    path: Option<SafeRelPath>,
}

impl LogFile {
    /// Where it is in the server's folder.
    pub fn path(&self) -> SafeRelPath {
        self.path.clone().unwrap_or_else(SafeRelPath::root)
    }
}

/// Whether a file name ends in `suffix`, whatever its case.
fn has_suffix(name: &str, suffix: &str) -> bool {
    name.len()
        .checked_sub(suffix.len())
        .and_then(|start| name.get(start..))
        .is_some_and(|end| end.eq_ignore_ascii_case(suffix))
}

/// What a file in `folder` is, if it is a log at all.
fn kind_of(folder: &str, name: &str) -> Option<LogKind> {
    match folder {
        "logs" => match name {
            "latest.log" => Some(LogKind::Latest),
            "debug.log" => Some(LogKind::Debug),
            _ if has_suffix(name, ".log") || has_suffix(name, ".log.gz") => Some(LogKind::Archive),
            _ => None,
        },
        "crash-reports" => has_suffix(name, ".txt").then_some(LogKind::Crash),
        "" => (name.starts_with("hs_err_pid") && has_suffix(name, ".log")).then_some(LogKind::JvmCrash),
        _ => None,
    }
}

/// Every log file of the server: the current log first, then newest first. A folder that isn't there
/// yet just has none.
pub async fn list(files: &ServerFiles) -> Result<Vec<LogFile>, FilesError> {
    let mut found = Vec::new();
    for folder in ["logs", "crash-reports", ""] {
        let path = if folder.is_empty() {
            SafeRelPath::root()
        } else {
            SafeRelPath::parse(folder).map_err(FilesError::from)?
        };
        let entries = match files.list(path).await {
            Ok(entries) => entries,
            Err(FilesError::NotFound(_)) => continue,
            Err(error) => return Err(error),
        };
        for entry in entries.into_iter().filter(|entry| entry.kind == EntryKind::File) {
            if let Some(kind) = kind_of(folder, &entry.name) {
                found.push(LogFile {
                    name: entry.name,
                    kind,
                    size: entry.size.unwrap_or(0),
                    modified: entry.modified,
                    path: Some(entry.path),
                });
            }
        }
    }
    sort(&mut found);
    Ok(found)
}

fn sort(files: &mut [LogFile]) {
    files.sort_by(|a, b| {
        let current = |file: &LogFile| file.kind != LogKind::Latest;
        current(a)
            .cmp(&current(b))
            .then_with(|| b.modified.cmp(&a.modified))
            .then_with(|| a.name.cmp(&b.name))
    });
}

/// The log file called `name`. Nothing else can be reached through a name.
pub async fn find(files: &ServerFiles, name: &str) -> Result<LogFile, LogError> {
    list(files)
        .await?
        .into_iter()
        .find(|file| file.name == name)
        .ok_or(LogError::NotFound)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_what_is_a_log_in_its_folder_counts() {
        let cases = [
            ("logs", "latest.log", Some(LogKind::Latest)),
            ("logs", "debug.log", Some(LogKind::Debug)),
            ("logs", "2026-11-09-1.log.gz", Some(LogKind::Archive)),
            ("logs", "debug-2.log.gz", Some(LogKind::Archive)),
            ("logs", "notes.txt", None),
            (
                "crash-reports",
                "crash-2026-11-09_12.00.00-server.txt",
                Some(LogKind::Crash),
            ),
            ("crash-reports", "secret.json", None),
            ("", "hs_err_pid123.log", Some(LogKind::JvmCrash)),
            ("", "server.properties", None),
            ("", "latest.log", None),
            ("config", "latest.log", None),
        ];
        for (folder, name, kind) in cases {
            assert_eq!(kind_of(folder, name), kind, "{folder}/{name}");
        }
    }

    #[test]
    fn the_current_log_comes_first_then_the_newest() {
        let file = |name: &str, kind, second: i64| LogFile {
            name: name.into(),
            kind,
            size: 0,
            modified: DateTime::from_timestamp(second, 0),
            path: None,
        };
        let mut files = [
            file("old.log.gz", LogKind::Archive, 1),
            file("crash.txt", LogKind::Crash, 3),
            file("latest.log", LogKind::Latest, 2),
        ];
        sort(&mut files);
        let names: Vec<&str> = files.iter().map(|file| file.name.as_str()).collect();
        assert_eq!(names, ["latest.log", "crash.txt", "old.log.gz"]);
    }
}
