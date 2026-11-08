//! The application event bus.
//!
//! Services publish what happened; the WebSocket hub, notifier and (later) scheduler subscribe.
//! It's a `tokio::sync::broadcast` channel: slow subscribers lose old events and receive a
//! `Lagged` error, after which they should re-fetch state over REST.

use serde::Serialize;
use tokio::sync::broadcast;

use crate::{
    game::{Diagnostic, GameSignal, LogLine},
    ids::{InstanceId, JobId},
    jobs::JobSnapshot,
    supervisor::ServerState,
};

/// Buffer size per subscriber before it starts lagging.
const CAPACITY: usize = 4096;

/// Something that happened, optionally tied to one instance.
#[derive(Debug, Clone, Serialize)]
pub struct Event {
    pub instance_id: Option<InstanceId>,
    #[serde(flatten)]
    pub kind: EventKind,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum EventKind {
    /// An instance was created, changed or deleted; clients should refresh it.
    InstanceChanged,
    InstanceDeleted,
    State(ServerState),
    Console(LogLine),
    Signal(GameSignal),
    Diagnostic(Diagnostic),
    Job(JobSnapshot),
    JobRemoved(JobId),
    /// The instance's list of backups changed (one was added, deleted, pinned or expired).
    BackupsChanged,
    /// The instance's schedules changed, or one of them ran.
    SchedulesChanged,
}

#[derive(Debug, Clone)]
pub struct EventBus {
    sender: broadcast::Sender<Event>,
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new()
    }
}

impl EventBus {
    pub fn new() -> Self {
        let (sender, _) = broadcast::channel(CAPACITY);
        Self { sender }
    }

    /// Publishes an event. Having no subscribers is normal and not an error.
    pub fn publish(&self, instance_id: Option<InstanceId>, kind: EventKind) {
        let _ = self.sender.send(Event { instance_id, kind });
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.sender.subscribe()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn subscribers_receive_published_events() {
        let bus = EventBus::new();
        let mut receiver = bus.subscribe();
        let id = InstanceId::generate();
        bus.publish(Some(id), EventKind::InstanceChanged);

        let event = receiver.recv().await.unwrap();
        assert_eq!(event.instance_id, Some(id));
        assert!(matches!(event.kind, EventKind::InstanceChanged));
    }

    #[test]
    fn publishing_without_subscribers_is_fine() {
        EventBus::new().publish(None, EventKind::InstanceDeleted);
    }

    #[test]
    fn events_serialize_with_type_and_data() {
        let event = Event {
            instance_id: None,
            kind: EventKind::JobRemoved(JobId::generate()),
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "job_removed");
        assert!(json["data"].is_string());
    }
}
