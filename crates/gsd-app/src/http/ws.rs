//! The live event stream: one WebSocket per browser tab.
//!
//! Client → server (JSON text frames):
//! ```json
//! {"op": "subscribe",   "topics": ["instances", "jobs", "console:<instance id>"]}
//! {"op": "unsubscribe", "topics": ["console:<instance id>"]}
//! ```
//! Server → client: `{"topic": "...", "instance_id": "...", "type": "...", "data": ...}`, plus
//! `{"type": "resync"}` when events were dropped (the client should refetch over REST).
//!
//! Every event is checked against the user's permissions. The session is re-validated
//! periodically, so revoked sessions lose their stream. The upgrade must come from this site
//! (`Origin`), so another site can't open a stream with the user's cookie (the cookie's
//! `SameSite=Strict` already stops browsers from sending it; this doesn't rely on that alone).

use std::{collections::HashSet, time::Duration};

use axum::{
    Router,
    extract::{
        State, WebSocketUpgrade,
        ws::{Message, Utf8Bytes, WebSocket},
    },
    http::HeaderMap,
    response::{IntoResponse, Response},
    routing::get,
};
use gsd_core::{
    auth::{Actor, Permission},
    events::{Event, EventKind},
};
use serde::Deserialize;
use tokio::sync::broadcast::error::RecvError;

use crate::{
    http::{
        csrf::origin_matches_host,
        error::ApiError,
        routes::jobs::can_view,
        session::{CurrentActor, session_token},
    },
    state::AppState,
};

const PING_INTERVAL: Duration = Duration::from_secs(25);
const REVALIDATE_INTERVAL: Duration = Duration::from_secs(60);
const MAX_CLIENT_MESSAGE: usize = 4096;
const MAX_TOPICS: usize = 64;

pub fn routes() -> Router<AppState> {
    Router::new().route("/ws", get(upgrade))
}

async fn upgrade(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    CurrentActor(actor): CurrentActor,
    headers: HeaderMap,
) -> Response {
    if !origin_matches_host(&headers) {
        return ApiError::forbidden("csrf_failed", "Request origin does not match this site.")
            .into_response();
    }
    let token = session_token(&headers).unwrap_or_default();
    ws.max_message_size(MAX_CLIENT_MESSAGE)
        .on_upgrade(move |socket| session(socket, state, actor, token))
}

#[derive(Debug, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
enum ClientMessage {
    Subscribe { topics: Vec<String> },
    Unsubscribe { topics: Vec<String> },
}

async fn session(mut socket: WebSocket, state: AppState, mut actor: Actor, token: String) {
    let mut events = state.events.subscribe();
    let mut topics: HashSet<String> = HashSet::new();
    let mut ping = tokio::time::interval(PING_INTERVAL);
    let mut revalidate = tokio::time::interval(REVALIDATE_INTERVAL);
    revalidate.tick().await; // the first tick fires immediately

    loop {
        tokio::select! {
            () = state.shutdown.cancelled() => break,
            incoming = socket.recv() => match incoming {
                Some(Ok(Message::Text(text))) => apply_client_message(&text, &mut topics),
                Some(Ok(Message::Close(_)) | Err(_)) | None => break,
                Some(Ok(_)) => {} // pings/pongs/binary: ignore
            },
            event = events.recv() => match event {
                Ok(event) => {
                    if let Some(frame) = frame_for(&event, &topics, &actor)
                        && socket.send(Message::Text(frame)).await.is_err()
                    {
                        break;
                    }
                }
                Err(RecvError::Lagged(_)) => {
                    if socket.send(Message::Text(Utf8Bytes::from_static(r#"{"type":"resync"}"#))).await.is_err() {
                        break;
                    }
                }
                Err(RecvError::Closed) => break,
            },
            _ = ping.tick() => {
                if socket.send(Message::Ping(axum::body::Bytes::new())).await.is_err() {
                    break;
                }
            }
            _ = revalidate.tick() => match state.auth.authenticate(&token).await {
                Ok(fresh) => actor = fresh, // picks up permission changes too
                Err(_) => break,
            },
        }
    }
    let _ = socket.send(Message::Close(None)).await;
}

fn apply_client_message(text: &str, topics: &mut HashSet<String>) {
    match serde_json::from_str::<ClientMessage>(text) {
        Ok(ClientMessage::Subscribe { topics: wanted }) => {
            for topic in wanted {
                if topics.len() < MAX_TOPICS && topic.len() <= 64 {
                    topics.insert(topic);
                }
            }
        }
        Ok(ClientMessage::Unsubscribe { topics: unwanted }) => {
            for topic in unwanted {
                topics.remove(&topic);
            }
        }
        Err(error) => tracing::debug!(%error, "ignoring malformed WebSocket message"),
    }
}

/// The topic an event belongs to.
fn topic_of(event: &Event) -> String {
    match (&event.kind, event.instance_id) {
        (EventKind::Console(_), Some(id)) => format!("console:{id}"),
        (EventKind::Job(_) | EventKind::JobRemoved(_), _) => "jobs".to_owned(),
        _ => "instances".to_owned(),
    }
}

/// Serialises `event` if the client subscribed to its topic and may see it.
fn frame_for(event: &Event, topics: &HashSet<String>, actor: &Actor) -> Option<Utf8Bytes> {
    let topic = topic_of(event);
    if !topics.contains(&topic) || !permitted(event, actor) {
        return None;
    }
    let mut value = serde_json::to_value(event).ok()?;
    value.as_object_mut()?.insert("topic".into(), topic.into());
    Some(Utf8Bytes::from(value.to_string()))
}

fn permitted(event: &Event, actor: &Actor) -> bool {
    match (&event.kind, event.instance_id) {
        (EventKind::Job(job), _) => can_view(actor, job),
        // Removal notices carry no data worth hiding.
        (EventKind::JobRemoved(_), _) => true,
        (EventKind::Console(_), Some(id)) => actor.can(Permission::ConsoleRead, id),
        (_, Some(id)) => actor.can(Permission::InstanceView, id),
        (_, None) => actor.role().can_access_all_instances(),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use chrono::Utc;
    use gsd_core::{
        auth::{GlobalRole, Grant, User, Username},
        game::{LineSource, LogLevel, LogLine},
        ids::{InstanceId, UserId},
    };

    use super::*;

    fn actor(role: GlobalRole, grants: HashMap<InstanceId, gsd_core::auth::PermissionSet>) -> Actor {
        let user = User {
            id: UserId::generate(),
            username: Username::parse("someone").unwrap(),
            role,
            must_change_password: false,
            disabled: false,
            created_at: Utc::now(),
        };
        Actor::new(user, grants)
    }

    fn console_event(id: InstanceId) -> Event {
        let line = LogLine {
            seq: 1,
            received_at: Utc::now(),
            source: LineSource::Stdout,
            level: LogLevel::Info,
            thread: None,
            text: "hi".into(),
            raw: "hi".into(),
        };
        Event {
            instance_id: Some(id),
            kind: EventKind::Console(line),
        }
    }

    #[test]
    fn subscriptions_are_managed_by_messages() {
        let mut topics = HashSet::new();
        apply_client_message(r#"{"op":"subscribe","topics":["instances","jobs"]}"#, &mut topics);
        apply_client_message(r#"{"op":"unsubscribe","topics":["jobs"]}"#, &mut topics);
        apply_client_message("not json", &mut topics);
        assert_eq!(topics, HashSet::from(["instances".to_owned()]));
    }

    #[test]
    fn console_needs_console_read_on_that_instance() {
        let watched = InstanceId::generate();
        let event = console_event(watched);
        let topics = HashSet::from([format!("console:{watched}")]);

        let viewer = actor(
            GlobalRole::Member,
            HashMap::from([(watched, Grant::Viewer.permissions())]),
        );
        assert!(frame_for(&event, &topics, &viewer).is_some());

        let stranger = actor(GlobalRole::Member, HashMap::new());
        assert!(frame_for(&event, &topics, &stranger).is_none());

        let admin = actor(GlobalRole::Admin, HashMap::new());
        assert!(
            frame_for(&event, &HashSet::new(), &admin).is_none(),
            "not subscribed"
        );
    }

    #[test]
    fn frames_carry_topic_type_and_data() {
        let id = InstanceId::generate();
        let admin = actor(GlobalRole::Owner, HashMap::new());
        let frame = frame_for(
            &console_event(id),
            &HashSet::from([format!("console:{id}")]),
            &admin,
        )
        .unwrap();
        let json: serde_json::Value = serde_json::from_str(&frame).unwrap();
        assert_eq!(json["topic"], format!("console:{id}"));
        assert_eq!(json["type"], "console");
        assert_eq!(json["data"]["text"], "hi");
    }
}
