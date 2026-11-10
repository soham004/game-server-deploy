//! What the players page shows, from the server's own files, who its console says is on, and the app's
//! history of visits.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use gsd_core::players::PlayerRecord;
use serde::Serialize;

use super::{
    lists::{self, BanEntry, CacheEntry, Operator, Player},
    names::offline_uuid,
};
use crate::properties::PropertiesDoc;

/// The game's default for `op-permission-level`.
pub const DEFAULT_OP_LEVEL: u8 = 4;
/// Known players listed; the rest are the ones seen longest ago.
pub const MAX_KNOWN: usize = 500;

/// How a change is made right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionChannel {
    /// The running server's management protocol (1.21.9 and later).
    Management,
    /// Commands typed into the running server's console.
    Console,
    /// The stopped server's files.
    Files,
}

#[derive(Debug, Clone, Serialize)]
pub struct PlayersView {
    /// `None` while the server starts or stops: nothing can change then.
    pub channel: Option<ActionChannel>,
    pub online_mode: bool,
    pub default_op_level: u8,
    pub online: Vec<OnlinePlayer>,
    pub allowlist: AllowlistView,
    pub operators: Vec<Operator>,
    pub bans: Vec<Ban>,
    pub ip_bans: Vec<IpBan>,
    pub known: Vec<KnownPlayer>,
}

#[derive(Debug, Clone, Serialize)]
pub struct OnlinePlayer {
    pub name: String,
    pub uuid: Option<String>,
    pub since: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AllowlistView {
    /// `white-list`: only listed players may join.
    pub on: bool,
    /// `enforce-whitelist`: players are kicked when they are taken off it.
    pub enforced: bool,
    pub players: Vec<Player>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Ban {
    pub name: Option<String>,
    pub uuid: Option<String>,
    pub reason: Option<String>,
    pub source: Option<String>,
    pub created: Option<DateTime<Utc>>,
    /// `None`: it doesn't end.
    pub expires: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct IpBan {
    pub ip: String,
    pub reason: Option<String>,
    pub source: Option<String>,
    pub created: Option<DateTime<Utc>>,
    pub expires: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct KnownPlayer {
    pub name: String,
    pub uuid: Option<String>,
    pub first_seen: Option<DateTime<Utc>>,
    pub last_seen: Option<DateTime<Utc>>,
    pub visits: u32,
    /// From the game's own statistics where it has them (they count play from before this app), else
    /// the visits the app saw; `None` for someone never seen playing.
    pub played_seconds: Option<u64>,
    pub online: bool,
}

/// The files and facts a view is made from.
#[derive(Debug, Default)]
pub struct Sources {
    pub properties: Option<String>,
    pub ops: Option<String>,
    pub allowlist: Option<String>,
    pub bans: Option<String>,
    pub ip_bans: Option<String>,
    pub user_cache: Option<String>,
    /// Names on now, from the console.
    pub online: Vec<String>,
    pub history: Vec<PlayerRecord>,
}

/// The settings a view needs from `server.properties`.
#[derive(Debug, Clone)]
pub struct ServerSettings {
    pub online_mode: bool,
    pub allowlist_on: bool,
    pub allowlist_enforced: bool,
    pub default_op_level: u8,
    pub level_name: String,
}

impl ServerSettings {
    pub fn from_properties(text: Option<&str>) -> Self {
        let doc = PropertiesDoc::parse(text.unwrap_or(""));
        let flag = |key, default| doc.get(key).map_or(default, |value| value.trim() == "true");
        Self {
            online_mode: flag("online-mode", true),
            allowlist_on: flag("white-list", false),
            allowlist_enforced: flag("enforce-whitelist", false),
            default_op_level: doc
                .get("op-permission-level")
                .and_then(|value| value.trim().parse().ok())
                .filter(|level| (1..=4).contains(level))
                .unwrap_or(DEFAULT_OP_LEVEL),
            level_name: crate::worlds::level_name_in(&doc),
        }
    }
}

/// Builds the view, with play time from the app's history; see [`use_game_play_time`].
pub fn build(channel: Option<ActionChannel>, sources: Sources) -> PlayersView {
    let settings = ServerSettings::from_properties(sources.properties.as_deref());
    let cache: Vec<CacheEntry> = lists::read(sources.user_cache.as_deref());
    let uuid_of = |name: &str| -> Option<String> {
        cache
            .iter()
            .find(|entry| entry.name.eq_ignore_ascii_case(name))
            .map(|entry| entry.uuid.clone())
            .or_else(|| (!settings.online_mode).then(|| offline_uuid(name)))
    };
    let history: HashMap<String, &PlayerRecord> = sources
        .history
        .iter()
        .map(|record| (record.name.to_ascii_lowercase(), record))
        .collect();

    let online: Vec<OnlinePlayer> = sources
        .online
        .iter()
        .map(|name| OnlinePlayer {
            name: name.clone(),
            uuid: uuid_of(name),
            since: history
                .get(&name.to_ascii_lowercase())
                .and_then(|record| record.online_since),
        })
        .collect();

    let bans = lists::read::<BanEntry>(sources.bans.as_deref())
        .into_iter()
        .map(|entry| Ban {
            name: entry.name,
            uuid: entry.uuid,
            reason: entry.reason,
            source: entry.source,
            created: entry.created.as_deref().and_then(lists::parse_time),
            expires: entry.expires.as_deref().and_then(lists::parse_time),
        })
        .collect();
    let ip_bans = lists::read::<BanEntry>(sources.ip_bans.as_deref())
        .into_iter()
        .filter_map(|entry| {
            Some(IpBan {
                ip: entry.ip?,
                reason: entry.reason,
                source: entry.source,
                created: entry.created.as_deref().and_then(lists::parse_time),
                expires: entry.expires.as_deref().and_then(lists::parse_time),
            })
        })
        .collect();

    let known = known_players(&sources, &cache, &uuid_of);
    PlayersView {
        channel,
        online_mode: settings.online_mode,
        default_op_level: settings.default_op_level,
        online,
        allowlist: AllowlistView {
            on: settings.allowlist_on,
            enforced: settings.allowlist_enforced,
            players: lists::read(sources.allowlist.as_deref()),
        },
        operators: lists::read(sources.ops.as_deref()),
        bans,
        ip_bans,
        known,
    }
}

/// Everyone seen here or in the server's name cache: those on now first, then by when they were last seen.
fn known_players(
    sources: &Sources,
    cache: &[CacheEntry],
    uuid_of: &dyn Fn(&str) -> Option<String>,
) -> Vec<KnownPlayer> {
    let mut known: Vec<KnownPlayer> = sources
        .history
        .iter()
        .map(|record| KnownPlayer {
            name: record.name.clone(),
            uuid: uuid_of(&record.name),
            first_seen: Some(record.first_seen),
            last_seen: Some(record.last_seen),
            visits: record.visits,
            played_seconds: Some(record.played_seconds),
            online: false,
        })
        .collect();
    for entry in cache {
        if !known
            .iter()
            .any(|player| player.name.eq_ignore_ascii_case(&entry.name))
        {
            known.push(KnownPlayer {
                name: entry.name.clone(),
                uuid: Some(entry.uuid.clone()),
                first_seen: None,
                last_seen: None,
                visits: 0,
                played_seconds: None,
                online: false,
            });
        }
    }
    for player in &mut known {
        player.online = sources
            .online
            .iter()
            .any(|name| name.eq_ignore_ascii_case(&player.name));
    }
    known.sort_by(|a, b| {
        b.online
            .cmp(&a.online)
            .then_with(|| b.last_seen.cmp(&a.last_seen))
            .then_with(|| a.name.to_ascii_lowercase().cmp(&b.name.to_ascii_lowercase()))
    });
    known.truncate(MAX_KNOWN);
    known
}

/// Uses the game's own play time (by UUID) where it has one: it counts play from before this app.
pub fn use_game_play_time<S: std::hash::BuildHasher>(
    view: &mut PlayersView,
    played: &HashMap<String, u64, S>,
) {
    for player in &mut view.known {
        if let Some(seconds) = player.uuid.as_ref().and_then(|uuid| played.get(uuid)) {
            player.played_seconds = Some(*seconds);
        }
    }
}

/// Play time in a player's statistics file (`<world>/stats/<uuid>.json`), in seconds. The game counts
/// ticks, 20 a second; before 1.17 the statistic was called `play_one_minute`.
pub fn played_seconds_in_stats(text: &str) -> Option<u64> {
    let stats: serde_json::Value = serde_json::from_str(text).ok()?;
    let custom = stats.pointer("/stats/minecraft:custom")?;
    let ticks = custom
        .get("minecraft:play_time")
        .or_else(|| custom.get("minecraft:play_one_minute"))?
        .as_u64()?;
    Some(ticks / 20)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use chrono::TimeDelta;

    use super::*;

    fn record(name: &str, seen_minutes_ago: i64, online: bool) -> PlayerRecord {
        let now = Utc::now();
        PlayerRecord {
            name: name.into(),
            first_seen: now - TimeDelta::days(1),
            last_seen: now - TimeDelta::minutes(seen_minutes_ago),
            visits: 3,
            played_seconds: 600,
            online_since: online.then(|| now - TimeDelta::minutes(5)),
        }
    }

    #[test]
    fn a_view_of_a_running_offline_mode_server() {
        let sources = Sources {
            properties: Some("online-mode=false\nwhite-list=true\nop-permission-level=3\n".into()),
            ops: Some(r#"[{"uuid":"u-op","name":"Alex","level":3}]"#.into()),
            allowlist: Some(r#"[{"uuid":"u1","name":"Steve"}]"#.into()),
            bans: Some(r#"[{"uuid":"u2","name":"Griefer","created":"2026-11-09 17:01:34 +0000","source":"admin","expires":"forever","reason":"Griefing"}]"#.into()),
            ip_bans: Some(r#"[{"ip":"10.0.0.1","created":"2026-11-09 17:01:34 +0000","source":"admin","expires":"2026-12-01 00:00:00 +0000","reason":"Spam"}]"#.into()),
            user_cache: Some(r#"[{"name":"Steve","uuid":"cached-steve","expiresOn":"2026-12-09 17:01:13 +0000"},{"name":"Looked_Up","uuid":"u3"}]"#.into()),
            online: vec!["Steve".into()],
            history: vec![record("Steve", 0, true), record("Alex", 60, false)],
        };
        let mut view = build(Some(ActionChannel::Console), sources);
        use_game_play_time(&mut view, &HashMap::from([("cached-steve".to_owned(), 7200)]));
        assert!(!view.online_mode);
        assert_eq!(view.default_op_level, 3);
        assert!(view.allowlist.on && !view.allowlist.enforced);
        assert_eq!(
            view.online[0].uuid.as_deref(),
            Some("cached-steve"),
            "the server's own cache first"
        );
        assert!(view.online[0].since.is_some());
        assert_eq!(view.bans[0].expires, None, "forever");
        assert!(view.ip_bans[0].expires.is_some());
        let names: Vec<_> = view.known.iter().map(|player| player.name.as_str()).collect();
        assert_eq!(
            names,
            ["Steve", "Alex", "Looked_Up"],
            "online, then last seen, then only cached"
        );
        assert_eq!(
            view.known[0].played_seconds,
            Some(7200),
            "the game's statistics win"
        );
        assert_eq!(view.known[1].played_seconds, Some(600));
        assert_eq!(
            view.known[1].uuid,
            Some(offline_uuid("Alex")),
            "an offline server's own rule"
        );
        assert_eq!(view.known[2].played_seconds, None);
    }

    #[test]
    fn a_fresh_server_has_defaults() {
        let view = build(Some(ActionChannel::Files), Sources::default());
        assert!(view.online_mode);
        assert_eq!(view.default_op_level, DEFAULT_OP_LEVEL);
        assert!(!view.allowlist.on);
        assert!(view.operators.is_empty() && view.known.is_empty());
    }

    #[test]
    fn play_time_from_statistics() {
        let stats = r#"{"stats":{"minecraft:custom":{"minecraft:play_time":72000,"minecraft:jump":3}},"DataVersion":4000}"#;
        assert_eq!(played_seconds_in_stats(stats), Some(3600));
        let old = r#"{"stats":{"minecraft:custom":{"minecraft:play_one_minute":200}}}"#;
        assert_eq!(played_seconds_in_stats(old), Some(10));
        assert_eq!(played_seconds_in_stats("{}"), None);
    }
}
