//! When a time trigger fires next, and how to say it in words.
//!
//! Every time is computed from one reference moment: the last run, or "now" when a schedule is
//! created, enabled, or skips runs it missed while the app was off. Event triggers have no time.

use std::str::FromStr;

use chrono::{DateTime, TimeDelta, Utc};
use chrono_tz::Tz;
use croner::Cron;

use super::model::{ServerEvent, Trigger};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TriggerError {
    #[error("{0:?} is not a valid cron expression: {1}")]
    Cron(String, String),
    #[error("{0:?} is not a known time zone (use a name like Europe/London)")]
    TimeZone(String),
}

/// The first moment after `from` at which `trigger` fires; `None` if it never will.
pub fn next_time(trigger: &Trigger, from: DateTime<Utc>) -> Result<Option<DateTime<Utc>>, TriggerError> {
    match trigger {
        Trigger::Cron { expr, time_zone } => {
            let cron = parse_cron(expr)?;
            let zone = parse_time_zone(time_zone)?;
            let next = cron
                .find_next_occurrence(&from.with_timezone(&zone), false)
                .map_err(|error| TriggerError::Cron(expr.clone(), error.to_string()))?;
            Ok(Some(next.with_timezone(&Utc)))
        }
        Trigger::Interval { every_secs } => {
            let every = TimeDelta::seconds(i64::try_from(*every_secs).unwrap_or(i64::MAX));
            Ok(from.checked_add_signed(every))
        }
        Trigger::Once { at } => Ok((*at > from).then_some(*at)),
        Trigger::Event { .. } => Ok(None),
    }
}

/// "At 04:00 (Europe/London)", "Every 6 hours", "When the server starts", …
pub fn describe(trigger: &Trigger) -> String {
    match trigger {
        Trigger::Cron { expr, time_zone } => match parse_cron(expr) {
            Ok(cron) => format!("{} ({time_zone})", cron.describe().trim_end_matches('.')),
            Err(_) => expr.clone(),
        },
        Trigger::Interval { every_secs } => {
            let words = duration_words(*every_secs);
            // "Every hour", not "Every 1 hour" (but "Every 1 hour 30 minutes").
            match words.strip_prefix("1 ") {
                Some(unit) if !unit.contains(' ') => format!("Every {unit}"),
                _ => format!("Every {words}"),
            }
        }
        Trigger::Once { at } => format!("Once, at {} UTC", at.format("%Y-%m-%d %H:%M")),
        Trigger::Event { event } => match event {
            ServerEvent::Started => "When the server has started".into(),
            ServerEvent::Stopped => "When the server stops".into(),
            ServerEvent::Crashed => "When the server crashes".into(),
            ServerEvent::PlayerJoined => "When a player joins".into(),
            ServerEvent::EmptyFor { secs } => {
                format!("When nobody has been online for {}", duration_words(*secs))
            }
        },
    }
}

pub(super) fn parse_cron(expr: &str) -> Result<Cron, TriggerError> {
    Cron::from_str(expr.trim()).map_err(|error| TriggerError::Cron(expr.to_owned(), error.to_string()))
}

pub(super) fn parse_time_zone(name: &str) -> Result<Tz, TriggerError> {
    name.parse().map_err(|_| TriggerError::TimeZone(name.to_owned()))
}

/// "90 seconds", "10 minutes", "6 hours", "2 days", "1 hour 30 minutes".
fn duration_words(secs: u64) -> String {
    let unit = |count: u64, name: &str| format!("{count} {name}{}", if count == 1 { "" } else { "s" });
    match secs {
        s if s % 86_400 == 0 => unit(s / 86_400, "day"),
        s if s % 3_600 == 0 => unit(s / 3_600, "hour"),
        s if s >= 3_600 && s % 60 == 0 => {
            format!("{} {}", unit(s / 3_600, "hour"), unit(s % 3_600 / 60, "minute"))
        }
        s if s % 60 == 0 => unit(s / 60, "minute"),
        s => unit(s, "second"),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn at(text: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(text).unwrap().to_utc()
    }

    fn cron(expr: &str, time_zone: &str) -> Trigger {
        Trigger::Cron {
            expr: expr.into(),
            time_zone: time_zone.into(),
        }
    }

    #[test]
    fn cron_times_follow_the_time_zone_across_daylight_saving() {
        let daily = cron("0 4 * * *", "Europe/London");
        // Winter: London is UTC.
        assert_eq!(
            next_time(&daily, at("2026-11-08T03:00:00Z")).unwrap(),
            Some(at("2026-11-08T04:00:00Z"))
        );
        // Summer: London is UTC+1, so 04:00 there is 03:00 UTC.
        assert_eq!(
            next_time(&daily, at("2026-07-01T05:00:00Z")).unwrap(),
            Some(at("2026-07-02T03:00:00Z"))
        );
        // Strictly after: a run at 04:00 is followed by tomorrow's.
        assert_eq!(
            next_time(&daily, at("2026-11-08T04:00:00Z")).unwrap(),
            Some(at("2026-11-09T04:00:00Z"))
        );
    }

    #[test]
    fn cron_accepts_seconds_and_rejects_nonsense() {
        let every_ten_seconds = cron("*/10 * * * * *", "UTC");
        assert_eq!(
            next_time(&every_ten_seconds, at("2026-11-08T03:00:01Z")).unwrap(),
            Some(at("2026-11-08T03:00:10Z"))
        );
        assert!(matches!(
            next_time(&cron("61 * * * *", "UTC"), Utc::now()),
            Err(TriggerError::Cron(..))
        ));
        assert!(matches!(
            next_time(&cron("0 4 * * *", "Mars/Olympus"), Utc::now()),
            Err(TriggerError::TimeZone(_))
        ));
    }

    #[test]
    fn intervals_count_from_the_reference_and_one_offs_fire_once() {
        let six_hours = Trigger::Interval { every_secs: 6 * 3600 };
        assert_eq!(
            next_time(&six_hours, at("2026-11-08T03:00:00Z")).unwrap(),
            Some(at("2026-11-08T09:00:00Z"))
        );
        let once = Trigger::Once {
            at: at("2026-11-09T12:00:00Z"),
        };
        assert_eq!(
            next_time(&once, at("2026-11-08T00:00:00Z")).unwrap(),
            Some(at("2026-11-09T12:00:00Z"))
        );
        assert_eq!(
            next_time(&once, at("2026-11-09T12:00:00Z")).unwrap(),
            None,
            "it ran"
        );
        let event = Trigger::Event {
            event: ServerEvent::Started,
        };
        assert_eq!(next_time(&event, Utc::now()).unwrap(), None);
    }

    #[test]
    fn descriptions_read_naturally() {
        assert_eq!(
            describe(&Trigger::Interval { every_secs: 21_600 }),
            "Every 6 hours"
        );
        assert_eq!(
            describe(&Trigger::Interval { every_secs: 5_400 }),
            "Every 1 hour 30 minutes"
        );
        assert_eq!(
            describe(&Trigger::Interval { every_secs: 600 }),
            "Every 10 minutes"
        );
        assert_eq!(describe(&Trigger::Interval { every_secs: 86_400 }), "Every day");
        assert_eq!(describe(&Trigger::Interval { every_secs: 3_600 }), "Every hour");
        assert_eq!(
            describe(&Trigger::Event {
                event: ServerEvent::EmptyFor { secs: 900 }
            }),
            "When nobody has been online for 15 minutes"
        );
        let daily = describe(&cron("0 4 * * *", "Europe/London"));
        assert!(
            daily.contains("04:00") && daily.ends_with("(Europe/London)"),
            "{daily}"
        );
    }
}
