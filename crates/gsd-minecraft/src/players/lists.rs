//! The server's own player files: `ops.json`, `whitelist.json`, `banned-players.json`, `banned-ips.json`
//! and `usercache.json`. Every version and loader keeps them, and saves them as soon as they change, so
//! they are read in every state. They are only written while the server is stopped: a running one holds
//! them in memory and would overwrite the change.
//!
//! Entries are read leniently (one a mod wrote oddly is skipped, not the file) and edited as JSON values,
//! so fields this app doesn't know survive.

use chrono::{DateTime, NaiveDateTime, Utc};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};

pub const OPS: &str = "ops.json";
pub const ALLOWLIST: &str = "whitelist.json";
pub const BANS: &str = "banned-players.json";
pub const IP_BANS: &str = "banned-ips.json";
pub const USER_CACHE: &str = "usercache.json";

/// How the files write a time: Java's `yyyy-MM-dd HH:mm:ss Z`.
const FILE_TIME: &str = "%Y-%m-%d %H:%M:%S %z";
/// A ban that doesn't end.
const FOREVER: &str = "forever";
/// What the game writes when nobody gave a reason.
pub const DEFAULT_BAN_REASON: &str = "Banned by an operator.";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Player {
    pub uuid: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Operator {
    pub uuid: String,
    pub name: String,
    pub level: u8,
    #[serde(default, rename = "bypassesPlayerLimit")]
    pub bypasses_player_limit: bool,
}

/// A ban as the file has it; times are read with [`parse_time`].
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct BanEntry {
    #[serde(default)]
    pub uuid: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub ip: Option<String>,
    #[serde(default)]
    pub created: Option<String>,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub expires: Option<String>,
    #[serde(default)]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct CacheEntry {
    pub name: String,
    pub uuid: String,
}

/// The entries of a list file that make sense; a missing or broken file is an empty list.
pub fn read<T: DeserializeOwned>(text: Option<&str>) -> Vec<T> {
    values(text)
        .into_iter()
        .filter_map(|value| serde_json::from_value(value).ok())
        .collect()
}

/// The raw entries, for editing.
pub fn values(text: Option<&str>) -> Vec<Value> {
    text.and_then(|text| serde_json::from_str::<Vec<Value>>(text).ok())
        .unwrap_or_default()
}

/// As the game writes them: two-space indents.
pub fn render(entries: &[Value]) -> String {
    let mut text = serde_json::to_string_pretty(entries).unwrap_or_else(|_| "[]".into());
    text.push('\n');
    text
}

/// A time from one of the files; `None` for "forever" and anything unreadable.
pub fn parse_time(text: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_str(text, FILE_TIME)
        .map(|time| time.with_timezone(&Utc))
        .ok()
        .or_else(|| {
            NaiveDateTime::parse_from_str(text, "%Y-%m-%d %H:%M:%S")
                .ok()
                .map(|time| time.and_utc())
        })
}

pub fn format_time(time: DateTime<Utc>) -> String {
    time.format(FILE_TIME).to_string()
}

fn same_player(entry: &Value, uuid: &str, name: &str) -> bool {
    let field = |key| entry.get(key).and_then(Value::as_str);
    field("uuid").is_some_and(|value| value.eq_ignore_ascii_case(uuid))
        || field("name").is_some_and(|value| value.eq_ignore_ascii_case(name))
}

/// Puts `entry` in place of the player's old one, or at the end.
pub fn upsert(entries: &mut Vec<Value>, player: &Player, entry: Value) {
    match entries
        .iter_mut()
        .find(|old| same_player(old, &player.uuid, &player.name))
    {
        Some(old) => *old = entry,
        None => entries.push(entry),
    }
}

/// Removes a player by name (any case); returns whether it was there.
pub fn remove_named(entries: &mut Vec<Value>, name: &str) -> bool {
    let before = entries.len();
    entries.retain(|entry| {
        !entry
            .get("name")
            .and_then(Value::as_str)
            .is_some_and(|value| value.eq_ignore_ascii_case(name))
    });
    entries.len() != before
}

pub fn remove_ip(entries: &mut Vec<Value>, ip: &str) -> bool {
    let before = entries.len();
    entries.retain(|entry| entry.get("ip").and_then(Value::as_str) != Some(ip));
    entries.len() != before
}

pub fn player_entry(player: &Player) -> Value {
    json!({ "uuid": player.uuid, "name": player.name })
}

/// How long the game keeps a name in its cache.
const CACHE_DAYS: i64 = 30;

/// A name cache entry, as the game writes one after looking a name up.
pub fn cache_entry(player: &Player, now: DateTime<Utc>) -> Value {
    let expires = now + chrono::TimeDelta::days(CACHE_DAYS);
    json!({ "name": player.name, "uuid": player.uuid, "expiresOn": format_time(expires) })
}

pub fn operator_entry(player: &Player, level: u8) -> Value {
    json!({ "uuid": player.uuid, "name": player.name, "level": level, "bypassesPlayerLimit": false })
}

/// What a ban says, for either file.
#[derive(Debug, Clone)]
pub struct BanDetails<'a> {
    pub reason: Option<&'a str>,
    pub source: &'a str,
    pub expires: Option<DateTime<Utc>>,
    pub now: DateTime<Utc>,
}

impl BanDetails<'_> {
    fn fields(&self) -> serde_json::Map<String, Value> {
        let mut fields = serde_json::Map::new();
        fields.insert("created".into(), format_time(self.now).into());
        fields.insert("source".into(), self.source.into());
        fields.insert(
            "expires".into(),
            self.expires
                .map_or_else(|| FOREVER.to_owned(), format_time)
                .into(),
        );
        fields.insert("reason".into(), self.reason.unwrap_or(DEFAULT_BAN_REASON).into());
        fields
    }
}

pub fn ban_entry(player: &Player, details: &BanDetails<'_>) -> Value {
    let mut fields = serde_json::Map::new();
    fields.insert("uuid".into(), player.uuid.clone().into());
    fields.insert("name".into(), player.name.clone().into());
    fields.extend(details.fields());
    Value::Object(fields)
}

pub fn ip_ban_entry(ip: &str, details: &BanDetails<'_>) -> Value {
    let mut fields = serde_json::Map::new();
    fields.insert("ip".into(), ip.into());
    fields.extend(details.fields());
    Value::Object(fields)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use chrono::TimeZone;

    use super::*;

    /// Written by a real 26.3 server.
    const OPS_FILE: &str = r#"[
  {
    "uuid": "75facf7f-9320-3e29-b801-7bae224ea921",
    "name": "E2E_Bot",
    "level": 3,
    "bypassesPlayerLimit": false
  }
]"#;
    const BANS_FILE: &str = r#"[ { "uuid": "75facf7f-9320-3e29-b801-7bae224ea921", "name": "E2E_Bot", "created": "2026-11-09 17:01:34 +0000", "source": "Probe", "expires": "2026-11-09 18:01:34 +0000", "reason": "Testing" } ]"#;

    #[test]
    fn real_files_are_read() {
        let ops: Vec<Operator> = read(Some(OPS_FILE));
        assert_eq!(ops[0].name, "E2E_Bot");
        assert_eq!((ops[0].level, ops[0].bypasses_player_limit), (3, false));
        let bans: Vec<BanEntry> = read(Some(BANS_FILE));
        assert_eq!(bans[0].reason.as_deref(), Some("Testing"));
        let expires = parse_time(bans[0].expires.as_deref().unwrap()).unwrap();
        assert_eq!(expires, Utc.with_ymd_and_hms(2026, 11, 9, 18, 1, 34).unwrap());
        assert_eq!(parse_time("forever"), None);
    }

    #[test]
    fn broken_entries_and_files_are_skipped() {
        let ops: Vec<Operator> = read(Some(
            r#"[{"name": "x"}, {"uuid": "u", "name": "Ok", "level": 4}]"#,
        ));
        assert_eq!(ops.len(), 1);
        assert_eq!(ops[0].name, "Ok");
        assert_eq!(read::<Operator>(Some("not json")), Vec::<Operator>::new());
        assert_eq!(read::<Operator>(None), Vec::<Operator>::new());
    }

    #[test]
    fn edits_keep_what_they_dont_know_and_replace_by_uuid_or_name() {
        let mut entries = values(Some(
            r#"[{"uuid": "u1", "name": "Steve", "level": 4, "modded": true}]"#,
        ));
        let alex = Player {
            uuid: "u2".into(),
            name: "Alex".into(),
        };
        upsert(&mut entries, &alex, operator_entry(&alex, 2));
        let steve = Player {
            uuid: "u1".into(),
            name: "Steve".into(),
        };
        upsert(&mut entries, &steve, operator_entry(&steve, 3));
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0]["level"], 3);
        assert!(remove_named(&mut entries, "alex"), "any case");
        assert!(!remove_named(&mut entries, "alex"));
        let other = values(Some(r#"[{"uuid": "u1", "name": "Steve", "modded": true}]"#));
        assert_eq!(other[0]["modded"], true);
        assert!(render(&entries).starts_with("[\n  {"));
    }

    #[test]
    fn bans_are_written_as_the_game_writes_them() {
        let now = Utc.with_ymd_and_hms(2026, 11, 9, 17, 1, 34).unwrap();
        let player = Player {
            uuid: "u1".into(),
            name: "Steve".into(),
        };
        let forever = ban_entry(
            &player,
            &BanDetails {
                reason: None,
                source: "admin",
                expires: None,
                now,
            },
        );
        assert_eq!(forever["created"], "2026-11-09 17:01:34 +0000");
        assert_eq!(forever["expires"], "forever");
        assert_eq!(forever["reason"], DEFAULT_BAN_REASON);
        let until = now + chrono::TimeDelta::hours(1);
        let ip = ip_ban_entry(
            "10.0.0.1",
            &BanDetails {
                reason: Some("spam"),
                source: "admin",
                expires: Some(until),
                now,
            },
        );
        assert_eq!(ip["expires"], "2026-11-09 18:01:34 +0000");
        assert_eq!(ip["ip"], "10.0.0.1");
        let mut list = vec![ip];
        assert!(remove_ip(&mut list, "10.0.0.1"));
        assert_eq!(list, Vec::<Value>::new());
    }
}
