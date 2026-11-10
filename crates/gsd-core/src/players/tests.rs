#![allow(clippy::unwrap_used)]

use chrono::TimeDelta;

use super::*;
use crate::store::open_in_memory;

async fn history() -> PlayerHistory {
    PlayerHistory::new(open_in_memory().await.unwrap(), EventBus::new())
}

fn event(id: InstanceId, kind: EventKind) -> Event {
    Event {
        instance_id: Some(id),
        kind,
    }
}

fn joined(id: InstanceId, name: &str) -> Event {
    event(
        id,
        EventKind::Signal(GameSignal::PlayerJoined { name: name.into() }),
    )
}

fn left(id: InstanceId, name: &str) -> Event {
    event(
        id,
        EventKind::Signal(GameSignal::PlayerLeft { name: name.into() }),
    )
}

fn stopped(id: InstanceId) -> Event {
    event(id, EventKind::State(ServerState::initial()))
}

fn starting(id: InstanceId, at: DateTime<Utc>) -> Event {
    event(
        id,
        EventKind::State(ServerState::Starting { since: at, pid: None }),
    )
}

#[tokio::test]
async fn visits_are_recorded_and_added_up() {
    let history = history().await;
    let id = InstanceId::generate();
    let t0 = Utc::now() - TimeDelta::hours(2);
    let at = |minutes| t0 + TimeDelta::minutes(minutes);

    history.record(&joined(id, "Steve"), at(0)).await.unwrap();
    history.record(&joined(id, "Alex"), at(5)).await.unwrap();
    history.record(&left(id, "Steve"), at(30)).await.unwrap();
    history.record(&joined(id, "steve"), at(40)).await.unwrap();
    history.record(&left(id, "STEVE"), at(50)).await.unwrap();

    let now = at(60);
    let players = history.players(id, now).await.unwrap();
    assert_eq!(players.len(), 2, "{players:?}");
    let alex = &players[0];
    assert_eq!(alex.name, "Alex", "on now, so seen last");
    assert_eq!(alex.online_since, Some(at(5)));
    assert_eq!(alex.played_seconds, 55 * 60, "counted up to now");
    let steve = &players[1];
    assert_eq!(
        steve.name, "steve",
        "one player whatever the case; the latest spelling"
    );
    assert_eq!((steve.visits, steve.played_seconds), (2, 40 * 60));
    assert_eq!((steve.first_seen, steve.last_seen), (at(0), at(50)));
    assert_eq!(steve.online_since, None);

    let visits = history.sessions(id, "Steve", 10).await.unwrap();
    assert_eq!(visits.len(), 2);
    assert_eq!(visits[0].joined_at, at(40), "newest first");
    assert_eq!(visits[0].length_seconds(), Some(600));
}

#[tokio::test]
async fn a_stop_ends_every_visit_and_a_start_closes_leftovers_without_an_end() {
    let history = history().await;
    let id = InstanceId::generate();
    let t0 = Utc::now() - TimeDelta::hours(1);
    let at = |minutes| t0 + TimeDelta::minutes(minutes);

    history.record(&joined(id, "Steve"), at(0)).await.unwrap();
    history.record(&stopped(id), at(10)).await.unwrap();
    let steve = &history.players(id, at(20)).await.unwrap()[0];
    assert_eq!(
        (steve.played_seconds, steve.online_since),
        (600, None),
        "they left when it stopped"
    );

    // A join whose server never reported stopping, then a fresh start.
    history.record(&joined(id, "Alex"), at(20)).await.unwrap();
    history.record(&starting(id, at(30)), at(30)).await.unwrap();
    let alex = history.sessions(id, "Alex", 10).await.unwrap();
    assert_eq!(
        (alex[0].online, alex[0].left_at),
        (false, None),
        "no end was seen"
    );
    assert_eq!(history.players(id, at(40)).await.unwrap()[0].played_seconds, 0);
}

#[tokio::test]
async fn a_join_without_a_leave_does_not_leave_two_visits_open() {
    let history = history().await;
    let id = InstanceId::generate();
    let now = Utc::now();
    history.record(&joined(id, "Steve"), now).await.unwrap();
    history.record(&joined(id, "Steve"), now).await.unwrap();
    let visits = history.sessions(id, "Steve", 10).await.unwrap();
    assert_eq!(visits.iter().filter(|visit| visit.online).count(), 1);
}

#[tokio::test]
async fn servers_are_kept_apart_and_a_deleted_one_is_forgotten() {
    let history = history().await;
    let (a, b) = (InstanceId::generate(), InstanceId::generate());
    let now = Utc::now();
    history.record(&joined(a, "Steve"), now).await.unwrap();
    history.record(&joined(b, "Alex"), now).await.unwrap();
    assert_eq!(history.players(a, now).await.unwrap()[0].name, "Steve");
    history
        .record(&event(a, EventKind::InstanceDeleted), now)
        .await
        .unwrap();
    assert_eq!(history.players(a, now).await.unwrap(), Vec::<PlayerRecord>::new());
    assert_eq!(history.players(b, now).await.unwrap().len(), 1);
}

#[tokio::test]
async fn starting_closes_what_an_earlier_run_left_open_and_then_records_events() {
    let pool = open_in_memory().await.unwrap();
    let events = EventBus::new();
    let id = InstanceId::generate();
    let earlier = PlayerHistory::new(pool.clone(), events.clone());
    earlier.record(&joined(id, "Steve"), Utc::now()).await.unwrap();

    let history = PlayerHistory::new(pool, events.clone());
    let shutdown = CancellationToken::new();
    let task = history.start(shutdown.clone());
    events.publish(
        Some(id),
        EventKind::Signal(GameSignal::PlayerJoined { name: "Alex".into() }),
    );
    let mut waited = 0;
    while history.players(id, Utc::now()).await.unwrap().len() < 2 && waited < 100 {
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        waited += 1;
    }
    let steve = history.sessions(id, "Steve", 1).await.unwrap();
    assert_eq!((steve[0].online, steve[0].left_at), (false, None));
    assert!(history.sessions(id, "Alex", 1).await.unwrap()[0].online);
    shutdown.cancel();
    task.await.unwrap();
}

#[tokio::test]
async fn only_the_newest_visits_are_kept() {
    let pool = open_in_memory().await.unwrap();
    let repo = SessionRepo::new(pool);
    let id = InstanceId::generate();
    let t0 = Utc::now();
    let total = SESSIONS_KEPT + 3;
    for n in 0..total {
        repo.open(id, "Steve", t0 + TimeDelta::seconds(n)).await.unwrap();
    }
    repo.prune().await.unwrap();
    let left = repo.all(id).await.unwrap();
    assert_eq!(i64::try_from(left.len()).unwrap(), SESSIONS_KEPT);
    assert_eq!(left[0].joined_at, t0 + TimeDelta::seconds(3), "the oldest went");
}
