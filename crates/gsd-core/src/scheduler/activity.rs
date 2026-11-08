//! What the scheduler remembers about each server from its events: who is online, when a
//! player was last seen, and since when a running server has been empty. Fed by
//! [`Activity::observe`]; read by conditions ("somebody played since the last run") and the
//! "nobody online for …" trigger.

use std::collections::{BTreeSet, HashMap};

use chrono::{DateTime, Utc};

use crate::{
    events::{Event, EventKind},
    game::GameSignal,
    ids::InstanceId,
    supervisor::ServerState,
};

#[derive(Debug, Default)]
pub struct Activity {
    servers: HashMap<InstanceId, ServerActivity>,
}

#[derive(Debug, Default)]
struct ServerActivity {
    online: BTreeSet<String>,
    last_player_seen: Option<DateTime<Utc>>,
    /// Running with nobody online since then; `None` while players are on or it isn't running.
    empty_since: Option<DateTime<Utc>>,
}

impl Activity {
    /// Updates what is known from one event that happened at `now`.
    pub fn observe(&mut self, event: &Event, now: DateTime<Utc>) {
        let Some(id) = event.instance_id else {
            return;
        };
        match &event.kind {
            EventKind::State(ServerState::Running { .. }) => {
                let server = self.servers.entry(id).or_default();
                server.online.clear();
                server.empty_since = Some(now);
            }
            EventKind::State(ServerState::Stopped { .. } | ServerState::Crashed { .. }) => {
                let server = self.servers.entry(id).or_default();
                server.online.clear();
                server.empty_since = None;
            }
            EventKind::Signal(GameSignal::PlayerJoined { name }) => {
                let server = self.servers.entry(id).or_default();
                server.online.insert(name.clone());
                server.last_player_seen = Some(now);
                server.empty_since = None;
            }
            EventKind::Signal(GameSignal::PlayerLeft { name }) => {
                let server = self.servers.entry(id).or_default();
                server.online.remove(name);
                server.last_player_seen = Some(now);
                if server.online.is_empty() {
                    server.empty_since = Some(now);
                }
            }
            EventKind::InstanceDeleted => {
                self.servers.remove(&id);
            }
            _ => {}
        }
    }

    /// Whether someone is online now or was after `since` (`None`: ever, while the app ran).
    pub fn played_since(&self, id: InstanceId, since: Option<DateTime<Utc>>) -> bool {
        self.servers.get(&id).is_some_and(|server| {
            !server.online.is_empty()
                || server
                    .last_player_seen
                    .is_some_and(|seen| since.is_none_or(|since| seen > since))
        })
    }

    pub fn players_online(&self, id: InstanceId) -> u32 {
        self.servers
            .get(&id)
            .map_or(0, |server| u32::try_from(server.online.len()).unwrap_or(u32::MAX))
    }

    /// Since when the running server has had nobody online.
    pub fn empty_since(&self, id: InstanceId) -> Option<DateTime<Utc>> {
        self.servers.get(&id).and_then(|server| server.empty_since)
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeDelta;

    use super::*;

    fn event(id: InstanceId, kind: EventKind) -> Event {
        Event {
            instance_id: Some(id),
            kind,
        }
    }

    fn running(now: DateTime<Utc>) -> EventKind {
        EventKind::State(ServerState::Running {
            since: now,
            pid: None,
        })
    }

    fn joined(name: &str) -> EventKind {
        EventKind::Signal(GameSignal::PlayerJoined { name: name.into() })
    }

    fn left(name: &str) -> EventKind {
        EventKind::Signal(GameSignal::PlayerLeft { name: name.into() })
    }

    #[test]
    fn tracks_players_and_when_the_server_became_empty() {
        let mut activity = Activity::default();
        let id = InstanceId::generate();
        let t0 = Utc::now();
        let at = |minutes| t0 + TimeDelta::minutes(minutes);

        activity.observe(&event(id, running(t0)), t0);
        assert_eq!(activity.empty_since(id), Some(t0), "empty from the start");
        assert!(!activity.played_since(id, None));

        activity.observe(&event(id, joined("Steve")), at(1));
        activity.observe(&event(id, joined("Alex")), at(2));
        assert_eq!((activity.players_online(id), activity.empty_since(id)), (2, None));
        activity.observe(&event(id, left("Steve")), at(3));
        assert_eq!(activity.empty_since(id), None, "Alex is still on");
        activity.observe(&event(id, left("Alex")), at(5));
        assert_eq!(activity.empty_since(id), Some(at(5)));

        assert!(activity.played_since(id, Some(at(4))), "Alex left after that");
        assert!(!activity.played_since(id, Some(at(6))));

        activity.observe(&event(id, EventKind::State(ServerState::initial())), at(7));
        assert_eq!(
            (activity.players_online(id), activity.empty_since(id)),
            (0, None),
            "stopped"
        );
    }

    #[test]
    fn servers_are_kept_apart_and_forgotten_when_deleted() {
        let mut activity = Activity::default();
        let (a, b) = (InstanceId::generate(), InstanceId::generate());
        let now = Utc::now();
        activity.observe(&event(a, joined("Steve")), now);
        assert!(activity.played_since(a, None));
        assert!(!activity.played_since(b, None));
        activity.observe(&event(a, EventKind::InstanceDeleted), now);
        assert!(!activity.played_since(a, None));
        activity.observe(
            &Event {
                instance_id: None,
                kind: EventKind::InstanceChanged,
            },
            now,
        );
    }
}
