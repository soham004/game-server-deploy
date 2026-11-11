//! What can be done to a player, checked before it goes anywhere, and how each channel says it: a call of
//! the management protocol, a console command, or (see `service`) an edit of the files.
//!
//! Console commands are built from checked parts only: names an account could have, IP addresses, and
//! reasons on one line. A name such as `@a` would mean everyone to the game.

use std::net::IpAddr;

use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{names::is_valid_name, view::PlayersView};

/// Longest reason or message; the game shows them in one line.
pub const MAX_TEXT_CHARS: usize = 256;
/// Operator levels: 1 (bypass spawn protection) to 4 (everything, `stop` included).
pub const MAX_OP_LEVEL: u8 = 4;
/// Levels above this need the right to run any command, since that is what they give.
pub const PLAYERS_MANAGE_MAX_LEVEL: u8 = 2;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum PlayerAction {
    /// `level` defaults to the server's `op-permission-level`.
    Op {
        name: String,
        #[serde(default)]
        level: Option<u8>,
    },
    Deop {
        name: String,
    },
    Allow {
        name: String,
    },
    Disallow {
        name: String,
    },
    Ban {
        name: String,
        #[serde(default)]
        reason: Option<String>,
        /// When it ends; never, without one.
        #[serde(default)]
        expires: Option<DateTime<Utc>>,
    },
    Pardon {
        name: String,
    },
    /// `target` is an address, or the name of a player who is on (their address is banned).
    BanIp {
        target: String,
        #[serde(default)]
        reason: Option<String>,
        #[serde(default)]
        expires: Option<DateTime<Utc>>,
    },
    PardonIp {
        ip: String,
    },
    Kick {
        name: String,
        #[serde(default)]
        reason: Option<String>,
    },
    Message {
        name: String,
        text: String,
    },
    /// Turns the allowlist (`white-list`) on or off.
    Allowlist {
        on: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum InvalidAction {
    #[error("\"{0}\" isn't a Minecraft name: up to 16 letters, digits and _")]
    Name(String),
    #[error("\"{0}\" isn't an IP address")]
    Ip(String),
    #[error("\"{0}\" is neither an IP address nor a player's name")]
    IpOrName(String),
    #[error("operator levels go from 1 to 4")]
    Level,
    #[error("that has to be one line of at most 256 characters")]
    Text,
    #[error("the message is empty")]
    EmptyMessage,
    #[error("that ban would already have ended")]
    ExpiresInThePast,
}

impl PlayerAction {
    pub fn validate(&self, now: DateTime<Utc>) -> Result<(), InvalidAction> {
        let name = |name: &str| {
            if is_valid_name(name) {
                Ok(())
            } else {
                Err(InvalidAction::Name(name.to_owned()))
            }
        };
        let reason = |reason: &Option<String>| reason.as_deref().map_or(Ok(()), check_text);
        let expires = |expires: &Option<DateTime<Utc>>| match expires {
            Some(at) if *at <= now => Err(InvalidAction::ExpiresInThePast),
            _ => Ok(()),
        };
        match self {
            Self::Op { name: player, level } => {
                name(player)?;
                match level {
                    Some(level) if !(1..=MAX_OP_LEVEL).contains(level) => Err(InvalidAction::Level),
                    _ => Ok(()),
                }
            }
            Self::Deop { name: player }
            | Self::Allow { name: player }
            | Self::Disallow { name: player }
            | Self::Pardon { name: player } => name(player),
            Self::Ban {
                name: player,
                reason: why,
                expires: until,
            } => {
                name(player)?;
                reason(why)?;
                expires(until)
            }
            Self::BanIp {
                target,
                reason: why,
                expires: until,
            } => {
                if target.parse::<IpAddr>().is_err() && !is_valid_name(target) {
                    return Err(InvalidAction::IpOrName(target.clone()));
                }
                reason(why)?;
                expires(until)
            }
            Self::PardonIp { ip } => ip_address(ip).map(|_| ()),
            Self::Kick {
                name: player,
                reason: why,
            } => {
                name(player)?;
                reason(why)
            }
            Self::Message { name: player, text } => {
                name(player)?;
                if text.trim().is_empty() {
                    return Err(InvalidAction::EmptyMessage);
                }
                check_text(text)
            }
            Self::Allowlist { .. } => Ok(()),
        }
    }

    /// The level an `op` gives: the one asked for, or the server's default.
    pub fn op_level(&self, default_level: u8) -> Option<u8> {
        match self {
            Self::Op { level, .. } => Some(level.unwrap_or(default_level)),
            _ => None,
        }
    }

    /// What only the management protocol (or a stopped server's files) can do, if this asks for it.
    pub fn needs_more_than_console(&self, default_level: u8) -> Option<&'static str> {
        match self {
            Self::Op {
                level: Some(level), ..
            } if *level != default_level => Some("choosing an operator level"),
            Self::Ban { expires: Some(_), .. } | Self::BanIp { expires: Some(_), .. } => {
                Some("a ban that ends")
            }
            _ => None,
        }
    }

    /// Whether it needs the server to be running.
    pub fn needs_running_server(&self) -> bool {
        matches!(self, Self::Kick { .. } | Self::Message { .. })
            || matches!(self, Self::BanIp { target, .. } if target.parse::<IpAddr>().is_err())
    }

    /// The console command. Only call after [`Self::validate`].
    pub fn console_command(&self) -> String {
        let with_reason = |command: String, reason: &Option<String>| match reason.as_deref().map(str::trim) {
            Some(reason) if !reason.is_empty() => format!("{command} {reason}"),
            _ => command,
        };
        match self {
            Self::Op { name, .. } => format!("op {name}"),
            Self::Deop { name } => format!("deop {name}"),
            Self::Allow { name } => format!("whitelist add {name}"),
            Self::Disallow { name } => format!("whitelist remove {name}"),
            Self::Ban { name, reason, .. } => with_reason(format!("ban {name}"), reason),
            Self::Pardon { name } => format!("pardon {name}"),
            Self::BanIp { target, reason, .. } => with_reason(format!("ban-ip {target}"), reason),
            Self::PardonIp { ip } => format!("pardon-ip {ip}"),
            Self::Kick { name, reason } => with_reason(format!("kick {name}"), reason),
            Self::Message { name, text } => format!("tell {name} {}", text.trim()),
            Self::Allowlist { on } => format!("whitelist {}", if *on { "on" } else { "off" }),
        }
    }

    /// The management protocol's method and parameters (verified against a real 26.3 server's).
    ///
    /// Players go by name only: the server resolves the name itself, and does nothing at all when an
    /// `id` disagrees with what it resolved (seen on a real 26.3 server).
    ///
    /// An operator's level is always sent, `default_op_level` when none was asked for. Left out, the
    /// running server would use the `op-permission-level` it started with, which can differ from the
    /// file's value that the caller's rights were checked against.
    pub fn management_call(&self, source: &str, default_op_level: u8) -> (&'static str, Value) {
        let player = |name: &str| json!({ "name": name });
        let expiry =
            |expires: &Option<DateTime<Utc>>| expires.map(|at| at.to_rfc3339_opts(SecondsFormat::Secs, true));
        let ban = |mut entry: Value, reason: &Option<String>, expires: &Option<DateTime<Utc>>| {
            entry["source"] = source.into();
            if let Some(reason) = reason
                .as_deref()
                .map(str::trim)
                .filter(|reason| !reason.is_empty())
            {
                entry["reason"] = reason.into();
            }
            if let Some(expires) = expiry(expires) {
                entry["expires"] = expires.into();
            }
            entry
        };
        match self {
            Self::Op { name, level } => {
                let operator = json!({
                    "player": player(name),
                    "permissionLevel": level.unwrap_or(default_op_level),
                    "bypassesPlayerLimit": false,
                });
                ("minecraft:operators/add", json!({ "add": [operator] }))
            }
            Self::Deop { name } => ("minecraft:operators/remove", json!({ "remove": [player(name)] })),
            Self::Allow { name } => ("minecraft:allowlist/add", json!({ "add": [player(name)] })),
            Self::Disallow { name } => ("minecraft:allowlist/remove", json!({ "remove": [player(name)] })),
            Self::Ban {
                name,
                reason,
                expires,
            } => {
                let entry = ban(json!({ "player": player(name) }), reason, expires);
                ("minecraft:bans/add", json!({ "add": [entry] }))
            }
            Self::Pardon { name } => ("minecraft:bans/remove", json!({ "remove": [player(name)] })),
            Self::BanIp {
                target,
                reason,
                expires,
            } => {
                let who = if target.parse::<IpAddr>().is_ok() {
                    json!({ "ip": target })
                } else {
                    json!({ "player": player(target) })
                };
                (
                    "minecraft:ip_bans/add",
                    json!({ "add": [ban(who, reason, expires)] }),
                )
            }
            Self::PardonIp { ip } => ("minecraft:ip_bans/remove", json!({ "ip": [ip] })),
            Self::Kick { name, reason } => {
                let mut kick = json!({ "player": player(name) });
                if let Some(reason) = reason
                    .as_deref()
                    .map(str::trim)
                    .filter(|reason| !reason.is_empty())
                {
                    kick["message"] = json!({ "literal": reason });
                }
                ("minecraft:players/kick", json!({ "kick": [kick] }))
            }
            Self::Message { name, text } => (
                "minecraft:server/system_message",
                json!({ "message": { "message": { "literal": text.trim() }, "receivingPlayers": [player(name)], "overlay": false } }),
            ),
            Self::Allowlist { on } => ("minecraft:serversettings/use_allowlist/set", json!({ "use": on })),
        }
    }

    /// Whether the server's lists show it done; `None` where they can't tell (a message; banning the
    /// address of a player by name, which the app doesn't know).
    pub fn is_done(&self, view: &PlayersView) -> Option<bool> {
        let operator = |name: &str| {
            view.operators
                .iter()
                .find(|op| op.name.eq_ignore_ascii_case(name))
        };
        let allowed = |name: &str| {
            view.allowlist
                .players
                .iter()
                .any(|p| p.name.eq_ignore_ascii_case(name))
        };
        let banned = |name: &str| {
            view.bans
                .iter()
                .any(|ban| ban.name.as_deref().is_some_and(|n| n.eq_ignore_ascii_case(name)))
        };
        let online = |name: &str| view.online.iter().any(|p| p.name.eq_ignore_ascii_case(name));
        Some(match self {
            Self::Op { name, level } => {
                operator(name).is_some_and(|op| level.is_none_or(|level| op.level == level))
            }
            Self::Deop { name } => operator(name).is_none(),
            Self::Allow { name } => allowed(name),
            Self::Disallow { name } => !allowed(name),
            Self::Ban { name, .. } => banned(name),
            Self::Pardon { name } => !banned(name),
            Self::BanIp { target, .. } if target.parse::<IpAddr>().is_ok() => {
                view.ip_bans.iter().any(|ban| ban.ip == *target)
            }
            Self::BanIp { .. } | Self::Message { .. } => return None,
            Self::PardonIp { ip } => !view.ip_bans.iter().any(|ban| ban.ip == *ip),
            Self::Kick { name, .. } => !online(name),
            Self::Allowlist { on } => view.allowlist.on == *on,
        })
    }
}

/// An address in the form the game writes it.
pub fn ip_address(text: &str) -> Result<String, InvalidAction> {
    text.parse::<IpAddr>()
        .map(|ip| ip.to_string())
        .map_err(|_| InvalidAction::Ip(text.to_owned()))
}

fn check_text(text: &str) -> Result<(), InvalidAction> {
    if text.chars().count() > MAX_TEXT_CHARS || text.chars().any(char::is_control) {
        return Err(InvalidAction::Text);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use chrono::TimeDelta;

    use super::*;

    fn action(value: Value) -> PlayerAction {
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn requests_read_as_actions() {
        assert_eq!(
            action(json!({ "action": "op", "name": "Steve", "level": 2 })),
            PlayerAction::Op {
                name: "Steve".into(),
                level: Some(2)
            }
        );
        assert_eq!(
            action(json!({ "action": "ban_ip", "target": "10.0.0.1" })),
            PlayerAction::BanIp {
                target: "10.0.0.1".into(),
                reason: None,
                expires: None
            }
        );
        assert!(serde_json::from_value::<PlayerAction>(json!({ "action": "explode" })).is_err());
    }

    #[test]
    fn anything_that_could_change_a_command_is_refused() {
        let now = Utc::now();
        for bad in [
            json!({ "action": "op", "name": "@a" }),
            json!({ "action": "op", "name": "Steve stop" }),
            json!({ "action": "op", "name": "Steve", "level": 5 }),
            json!({ "action": "op", "name": "Steve", "level": 0 }),
            json!({ "action": "kick", "name": "Steve", "reason": "bye\nstop" }),
            json!({ "action": "ban", "name": "Steve", "reason": "x".repeat(257) }),
            json!({ "action": "ban_ip", "target": "10.0.0.1; stop" }),
            json!({ "action": "pardon_ip", "ip": "Steve" }),
            json!({ "action": "message", "name": "Steve", "text": "  " }),
            json!({ "action": "message", "name": "Steve", "text": "a\u{1b}[2J" }),
        ] {
            assert!(action(bad.clone()).validate(now).is_err(), "{bad}");
        }
        let past =
            json!({ "action": "ban", "name": "Steve", "expires": (now - TimeDelta::hours(1)).to_rfc3339() });
        assert_eq!(action(past).validate(now), Err(InvalidAction::ExpiresInThePast));
        let good = json!({ "action": "ban", "name": "Steve", "reason": "Griefing the spawn", "expires": (now + TimeDelta::hours(1)).to_rfc3339() });
        assert_eq!(action(good).validate(now), Ok(()));
    }

    #[test]
    fn console_commands() {
        for (request, command) in [
            (json!({ "action": "op", "name": "Steve" }), "op Steve"),
            (
                json!({ "action": "allow", "name": "Steve" }),
                "whitelist add Steve",
            ),
            (
                json!({ "action": "ban", "name": "Steve", "reason": " Griefing " }),
                "ban Steve Griefing",
            ),
            (
                json!({ "action": "ban", "name": "Steve", "reason": "" }),
                "ban Steve",
            ),
            (
                json!({ "action": "ban_ip", "target": "10.0.0.1" }),
                "ban-ip 10.0.0.1",
            ),
            (
                json!({ "action": "pardon_ip", "ip": "10.0.0.1" }),
                "pardon-ip 10.0.0.1",
            ),
            (
                json!({ "action": "kick", "name": "Steve", "reason": "Later" }),
                "kick Steve Later",
            ),
            (
                json!({ "action": "message", "name": "Steve", "text": "Hi there" }),
                "tell Steve Hi there",
            ),
            (json!({ "action": "allowlist", "on": false }), "whitelist off"),
        ] {
            assert_eq!(action(request).console_command(), command);
        }
    }

    #[test]
    fn what_the_console_cant_say() {
        let at = Utc::now() + TimeDelta::hours(1);
        assert!(
            PlayerAction::Op {
                name: "S".into(),
                level: Some(2)
            }
            .needs_more_than_console(4)
            .is_some()
        );
        assert!(
            PlayerAction::Op {
                name: "S".into(),
                level: Some(4)
            }
            .needs_more_than_console(4)
            .is_none()
        );
        assert!(
            PlayerAction::Ban {
                name: "S".into(),
                reason: None,
                expires: Some(at)
            }
            .needs_more_than_console(4)
            .is_some()
        );
        assert!(
            PlayerAction::Kick {
                name: "S".into(),
                reason: None
            }
            .needs_running_server()
        );
        assert!(
            PlayerAction::BanIp {
                target: "S".into(),
                reason: None,
                expires: None
            }
            .needs_running_server()
        );
        assert!(
            !PlayerAction::BanIp {
                target: "::1".into(),
                reason: None,
                expires: None
            }
            .needs_running_server()
        );
    }

    /// The shapes a real 26.3 server took.
    #[test]
    fn management_calls() {
        let (method, params) = PlayerAction::Op {
            name: "E2E_Bot".into(),
            level: Some(3),
        }
        .management_call("admin", 4);
        assert_eq!(method, "minecraft:operators/add");
        assert_eq!(
            params,
            json!({ "add": [{ "player": { "name": "E2E_Bot" }, "permissionLevel": 3, "bypassesPlayerLimit": false }] })
        );
        let at = "2026-11-09T18:01:34Z".parse().unwrap();
        let (_, params) = PlayerAction::Ban {
            name: "E2E_Bot".into(),
            reason: Some("Testing".into()),
            expires: Some(at),
        }
        .management_call("admin", 4);
        assert_eq!(
            params,
            json!({ "add": [{ "player": { "name": "E2E_Bot" }, "source": "admin", "reason": "Testing", "expires": "2026-11-09T18:01:34Z" }] })
        );
        let (method, params) = PlayerAction::PardonIp {
            ip: "10.1.2.3".into(),
        }
        .management_call("admin", 4);
        assert_eq!(
            (method, params),
            ("minecraft:ip_bans/remove", json!({ "ip": ["10.1.2.3"] }))
        );
        let (_, params) = PlayerAction::Kick {
            name: "E2E_Bot".into(),
            reason: Some("Bye".into()),
        }
        .management_call("admin", 4);
        assert_eq!(
            params,
            json!({ "kick": [{ "player": { "name": "E2E_Bot" }, "message": { "literal": "Bye" } }] })
        );
    }
}
