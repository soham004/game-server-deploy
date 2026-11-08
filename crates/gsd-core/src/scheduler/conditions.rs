//! Whether a schedule's conditions hold at the moment it fires. Pure: the engine gathers the
//! facts ([`Situation`]) and this decides, so every rule is unit-tested.

use chrono::{DateTime, NaiveTime, Utc};

use super::{model::Condition, trigger::parse_time_zone};

/// The facts conditions look at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Situation {
    pub running: bool,
    pub players_online: u32,
    /// Someone was online at some point since the schedule last ran.
    pub played_since_last_run: bool,
    pub now: DateTime<Utc>,
}

/// The first condition that doesn't hold, explained for the run history; `None` when all hold.
pub fn unmet(conditions: &[Condition], situation: &Situation) -> Option<String> {
    conditions
        .iter()
        .find_map(|condition| check(condition, situation).err())
}

fn check(condition: &Condition, situation: &Situation) -> Result<(), String> {
    match condition {
        Condition::ServerRunning if !situation.running => Err("the server wasn't running".into()),
        Condition::ServerStopped if situation.running => Err("the server was running".into()),
        Condition::PlayersOnline { min } if situation.players_online < *min => Err(format!(
            "{} online, fewer than {min}",
            plural(situation.players_online, "player")
        )),
        Condition::PlayersSinceLastRun if !situation.played_since_last_run => {
            Err("nobody played since the last run".into())
        }
        Condition::TimeWindow { from, to, time_zone } => {
            let zone = parse_time_zone(time_zone).map_err(|e| e.to_string())?;
            let local = situation.now.with_timezone(&zone).time();
            if in_window(local, *from, *to) {
                Ok(())
            } else {
                Err(format!(
                    "{} is outside {}–{} ({time_zone})",
                    local.format("%H:%M"),
                    from.format("%H:%M"),
                    to.format("%H:%M")
                ))
            }
        }
        _ => Ok(()),
    }
}

/// `from` inclusive, `to` exclusive; a window whose `from` is after `to` spans midnight.
fn in_window(time: NaiveTime, from: NaiveTime, to: NaiveTime) -> bool {
    if from <= to {
        from <= time && time < to
    } else {
        time >= from || time < to
    }
}

fn plural(count: u32, noun: &str) -> String {
    format!("{count} {noun}{}", if count == 1 { "" } else { "s" })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn situation(running: bool, players_online: u32, played: bool, now: &str) -> Situation {
        Situation {
            running,
            players_online,
            played_since_last_run: played,
            now: DateTime::parse_from_rfc3339(now).unwrap().to_utc(),
        }
    }

    fn time(hour: u32, minute: u32) -> NaiveTime {
        NaiveTime::from_hms_opt(hour, minute, 0).unwrap()
    }

    #[test]
    fn server_and_player_conditions_explain_themselves() {
        let quiet = situation(true, 0, false, "2026-11-08T12:00:00Z");
        assert_eq!(unmet(&[Condition::ServerRunning], &quiet), None);
        assert_eq!(
            unmet(&[Condition::ServerStopped], &quiet).as_deref(),
            Some("the server was running")
        );
        assert_eq!(
            unmet(&[Condition::PlayersOnline { min: 2 }], &quiet).as_deref(),
            Some("0 players online, fewer than 2")
        );
        assert_eq!(
            unmet(
                &[Condition::ServerRunning, Condition::PlayersSinceLastRun],
                &quiet
            )
            .as_deref(),
            Some("nobody played since the last run"),
            "the first unmet one is reported"
        );
        let busy = situation(true, 1, true, "2026-11-08T12:00:00Z");
        assert_eq!(
            unmet(
                &[
                    Condition::PlayersOnline { min: 1 },
                    Condition::PlayersSinceLastRun
                ],
                &busy
            ),
            None
        );
        assert_eq!(
            unmet(&[], &situation(false, 0, false, "2026-11-08T12:00:00Z")),
            None
        );
    }

    #[test]
    fn time_windows_use_the_time_zone_and_may_span_midnight() {
        let night = Condition::TimeWindow {
            from: time(22, 0),
            to: time(6, 0),
            time_zone: "Asia/Kolkata".into(),
        };
        // 18:00 UTC is 23:30 in Kolkata: inside.
        assert_eq!(
            unmet(
                std::slice::from_ref(&night),
                &situation(true, 0, false, "2026-11-08T18:00:00Z")
            ),
            None
        );
        // 06:00 UTC is 11:30 there: outside.
        assert_eq!(
            unmet(&[night], &situation(true, 0, false, "2026-11-08T06:00:00Z")).as_deref(),
            Some("11:30 is outside 22:00–06:00 (Asia/Kolkata)")
        );
        assert!(
            in_window(time(9, 0), time(9, 0), time(17, 0)),
            "from is inclusive"
        );
        assert!(
            !in_window(time(17, 0), time(9, 0), time(17, 0)),
            "to is exclusive"
        );
    }
}
