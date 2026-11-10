//! Who played on each server, and when.
//!
//! Every visit is a row in `player_sessions`, from the "joined" and "left" signals the game adapter finds in
//! the console. It works the same on every version, and on servers that were never told about this app.
//! Players are known by name here: the game decides what a name's account is (see the Minecraft players
//! module, which pairs names with UUIDs).
//!
//! A visit ends when the player leaves, or when the server stops (then everyone has left). A visit whose
//! end the app missed, because the app itself stopped first, keeps no end time and doesn't count towards
//! play time: it would otherwise be a guess.

mod repo;

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::SqlitePool;
use tokio::{sync::broadcast::error::RecvError, task::JoinHandle};
use tokio_util::sync::CancellationToken;

use crate::{
    events::{Event, EventBus, EventKind},
    game::GameSignal,
    ids::InstanceId,
    store::RepoError,
    supervisor::ServerState,
};
pub use repo::SESSIONS_KEPT;
use repo::SessionRepo;

/// One visit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Session {
    pub name: String,
    pub joined_at: DateTime<Utc>,
    /// When they left; `None` while they are on, or when the app didn't see the end.
    pub left_at: Option<DateTime<Utc>>,
    pub online: bool,
}

impl Session {
    /// How long it lasted, if it is over and the app saw it end.
    pub fn length_seconds(&self) -> Option<u64> {
        let left = self.left_at?;
        u64::try_from((left - self.joined_at).num_seconds()).ok()
    }
}

/// Everything known about one player on one server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PlayerRecord {
    pub name: String,
    pub first_seen: DateTime<Utc>,
    pub last_seen: DateTime<Utc>,
    pub visits: u32,
    /// The sum of the visits whose end the app saw.
    pub played_seconds: u64,
    /// When the current visit began, while they are on.
    pub online_since: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone)]
pub struct PlayerHistory {
    repo: SessionRepo,
    events: EventBus,
}

impl PlayerHistory {
    pub fn new(pool: SqlitePool, events: EventBus) -> Self {
        Self {
            repo: SessionRepo::new(pool),
            events,
        }
    }

    /// Records visits from the event bus until `shutdown`, and what was published before it. Visits left
    /// open by an earlier run are closed first, without an end time.
    pub fn start(&self, shutdown: CancellationToken) -> JoinHandle<()> {
        let history = self.clone();
        // Subscribed before the task runs, so nothing published after `start` returns is missed.
        let mut events = self.events.subscribe();
        tokio::spawn(async move {
            if let Err(error) = history.repo.close_all_unseen().await {
                tracing::error!(%error, "could not close the visits an earlier run left open");
            }
            if let Err(error) = history.repo.prune().await {
                tracing::warn!(%error, "could not remove old visits");
            }
            loop {
                tokio::select! {
                    () = shutdown.cancelled() => {
                        // The servers' stops come just before; they end the last visits.
                        while let Ok(event) = events.try_recv() {
                            if let Err(error) = history.record(&event, Utc::now()).await {
                                tracing::error!(%error, "could not record a visit");
                            }
                        }
                        return;
                    }
                    event = events.recv() => match event {
                        Ok(event) => {
                            if let Err(error) = history.record(&event, Utc::now()).await {
                                tracing::error!(%error, "could not record a visit");
                            }
                        }
                        // Joins or leaves were missed. The next start of each server closes what is open.
                        Err(RecvError::Lagged(missed)) => tracing::warn!(missed, "player history missed events"),
                        Err(RecvError::Closed) => return,
                    },
                }
            }
        })
    }

    /// Updates the history from one event that happened at `now`.
    pub async fn record(&self, event: &Event, now: DateTime<Utc>) -> Result<(), RepoError> {
        let Some(id) = event.instance_id else {
            return Ok(());
        };
        match &event.kind {
            EventKind::Signal(GameSignal::PlayerJoined { name }) => {
                // A visit still open for this name means its leave line was missed.
                self.repo.close(id, Some(name), None).await?;
                self.repo.open(id, name, now).await
            }
            EventKind::Signal(GameSignal::PlayerLeft { name }) => {
                self.repo.close(id, Some(name), Some(now)).await
            }
            // A stopped server has nobody on: they all left when it stopped.
            EventKind::State(ServerState::Stopped { .. } | ServerState::Crashed { .. }) => {
                self.repo.close(id, None, Some(now)).await
            }
            // A server that has just started has nobody on yet; anything open is left over.
            EventKind::State(ServerState::Starting { .. }) => self.repo.close(id, None, None).await,
            EventKind::InstanceDeleted => self.repo.delete_instance(id).await,
            _ => Ok(()),
        }
    }

    /// Every player seen on the server, most recently seen first. A name is one player whatever its case,
    /// as Minecraft has it; the latest spelling is shown.
    pub async fn players(&self, id: InstanceId, now: DateTime<Utc>) -> Result<Vec<PlayerRecord>, RepoError> {
        Ok(summarise(self.repo.all(id).await?, now))
    }

    /// One player's latest visits, newest first.
    pub async fn sessions(&self, id: InstanceId, name: &str, limit: u32) -> Result<Vec<Session>, RepoError> {
        self.repo.of_player(id, name, limit).await
    }
}

/// Groups visits (oldest first) by player.
fn summarise(sessions: Vec<Session>, now: DateTime<Utc>) -> Vec<PlayerRecord> {
    let mut players: HashMap<String, PlayerRecord> = HashMap::new();
    for session in sessions {
        let seen = if session.online {
            now
        } else {
            session.left_at.unwrap_or(session.joined_at)
        };
        let record = players
            .entry(session.name.to_ascii_lowercase())
            .or_insert_with(|| PlayerRecord {
                name: session.name.clone(),
                first_seen: session.joined_at,
                last_seen: seen,
                visits: 0,
                played_seconds: 0,
                online_since: None,
            });
        record.name.clone_from(&session.name);
        record.visits += 1;
        record.last_seen = record.last_seen.max(seen);
        record.played_seconds += session.length_seconds().unwrap_or(0);
        if session.online {
            record.played_seconds += u64::try_from((now - session.joined_at).num_seconds()).unwrap_or(0);
            record.online_since = Some(session.joined_at);
        }
    }
    let mut players: Vec<_> = players.into_values().collect();
    players.sort_by(|a, b| b.last_seen.cmp(&a.last_seen).then_with(|| a.name.cmp(&b.name)));
    players
}

#[cfg(test)]
mod tests;
