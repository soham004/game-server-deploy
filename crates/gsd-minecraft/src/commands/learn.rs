//! Asking a running server for its commands: `help`, sent through its console, and the lines it
//! answers with (see `help.rs` for what they look like).
//!
//! The command shows in the console as `[autocomplete] > help`, with its answer, so nothing happens
//! behind the operator's back. It is asked once per set of mods or plugins (see
//! [`fingerprint`]), when someone who may type commands opens the console.

use std::time::Duration;

use gsd_core::{
    events::{Event, EventKind},
    files::{EntryKind, FilesError, SafeRelPath, ServerFiles},
    game::LineSource,
    ids::InstanceId,
    instances::{Instance, InstanceService},
    supervisor::ServerState,
};
use sha2::{Digest, Sha256};
use tokio::{
    sync::broadcast::{self, error::RecvError},
    time::Instant,
};

use super::{CommandsError, help};
use crate::settings::MinecraftServer;

/// Echoed in the console as the one who typed `help`.
pub const AUTHOR: &str = "autocomplete";
/// Longest wait for the answer to start.
const FIRST_LINE_WAIT: Duration = Duration::from_secs(10);
/// The answer is printed in one go; this long without another line of it ends it.
const QUIET_AFTER: Duration = Duration::from_millis(1500);
/// However it goes, no longer than this.
const LONGEST_ANSWER: Duration = Duration::from_secs(30);

/// What decides a server's commands besides its version: its loader and the jars in `mods/` and
/// `plugins/` (names and sizes). Learned commands are asked again when this changes.
pub async fn fingerprint(instance: &Instance, server: &MinecraftServer) -> Result<String, FilesError> {
    let files = ServerFiles::open(instance).await?;
    let mut hasher = Sha256::new();
    hasher.update(format!(
        "{}|{}|{}\n",
        server.loader,
        server.mc_version,
        server.loader_version.as_deref().unwrap_or("")
    ));
    for folder in ["mods", "plugins"] {
        let entries = match files.list(SafeRelPath::parse(folder)?).await {
            Ok(entries) => entries,
            Err(FilesError::NotFound(_)) => continue,
            Err(error) => return Err(error),
        };
        let mut jars: Vec<(String, u64)> = entries
            .into_iter()
            .filter(|entry| entry.kind == EntryKind::File && is_jar(&entry.name))
            .map(|entry| (entry.name, entry.size.unwrap_or(0)))
            .collect();
        jars.sort();
        for (name, size) in jars {
            hasher.update(format!("{folder}/{name}|{size}\n"));
        }
    }
    Ok(hex::encode(hasher.finalize()))
}

/// Sends `help` to the running server and returns the lines of its answer.
pub async fn ask_help(
    instances: &InstanceService,
    mut events: broadcast::Receiver<Event>,
    id: InstanceId,
) -> Result<Vec<String>, CommandsError> {
    instances
        .send_command(id, "help", Some(AUTHOR.to_owned()))
        .await
        .map_err(|error| CommandsError::Failed(format!("could not send help: {error}")))?;
    let mut lines = Vec::new();
    // Other servers' events keep arriving meanwhile: the waits are deadlines, not gaps.
    let mut deadline = Instant::now() + FIRST_LINE_WAIT;
    let give_up = Instant::now() + LONGEST_ANSWER;
    loop {
        let event = match tokio::time::timeout_at(deadline.min(give_up), events.recv()).await {
            Err(_) | Ok(Err(RecvError::Closed)) => break,
            Ok(Ok(event)) => event,
            // Lines were missed under heavy output: what arrived is still usable.
            Ok(Err(RecvError::Lagged(_))) => continue,
        };
        if event.instance_id != Some(id) {
            continue;
        }
        match event.kind {
            EventKind::Console(line)
                if line.source == LineSource::Stdout && help::is_help_line(&line.text) =>
            {
                if lines.len() >= help::MAX_LEARNED {
                    break;
                }
                lines.push(line.text);
                deadline = Instant::now() + QUIET_AFTER;
            }
            EventKind::State(
                ServerState::Stopping { .. } | ServerState::Stopped { .. } | ServerState::Crashed { .. },
            ) => {
                return Err(CommandsError::Failed(
                    "the server stopped before it answered".into(),
                ));
            }
            _ => {}
        }
    }
    if lines.is_empty() {
        return Err(CommandsError::Failed(
            "the server didn't answer help with any commands".into(),
        ));
    }
    Ok(lines)
}

fn is_jar(name: &str) -> bool {
    std::path::Path::new(name)
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("jar"))
}
