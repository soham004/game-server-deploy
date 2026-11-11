//! Checking a schedule plan before it is saved: sensible limits, a cron expression and time
//! zone that parse, warnings in a countdown order, console lines on one line.

use chrono::{DateTime, Utc};

use super::{
    model::{Action, Condition, SchedulePlan, ServerEvent, Trigger},
    trigger::{TriggerError, parse_cron, parse_time_zone},
};
use crate::backups::MAX_NOTE_CHARS;

pub const MAX_NAME_CHARS: usize = 64;
pub const MAX_ACTIONS: usize = 20;
pub const MAX_CONDITIONS: usize = 10;
const MAX_LINE_CHARS: usize = 256;
const MIN_INTERVAL_SECS: u64 = 60;
const MAX_INTERVAL_SECS: u64 = 31 * 86_400;
const MAX_WAIT_SECS: u64 = 3_600;
const MAX_WARNINGS: usize = 10;
const MAX_WARN_SECS: u64 = 3_600;
const MIN_EMPTY_SECS: u64 = 60;
const MAX_EMPTY_SECS: u64 = 86_400;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PlanError {
    #[error("the name must be one line of 1 to {MAX_NAME_CHARS} characters")]
    Name,
    #[error("add 1 to {MAX_ACTIONS} actions")]
    ActionCount,
    #[error("at most {MAX_CONDITIONS} conditions")]
    ConditionCount,
    #[error(transparent)]
    Trigger(#[from] TriggerError),
    #[error("the interval must be between 1 minute and 31 days")]
    Interval,
    #[error("that time has already passed")]
    InThePast,
    #[error("\"nobody online\" must be between 1 minute and 1 day")]
    EmptyFor,
    #[error("action {0}: {1}")]
    Action(usize, String),
}

/// Checks `plan`; `now` decides whether a one-off time is in the past.
pub fn validate(plan: &SchedulePlan, now: DateTime<Utc>) -> Result<(), PlanError> {
    // One line: the console shows the name next to every command the schedule sends.
    let name = plan.name.trim();
    if name.is_empty() || name.chars().count() > MAX_NAME_CHARS || name.chars().any(char::is_control) {
        return Err(PlanError::Name);
    }
    validate_trigger(&plan.trigger, now)?;
    if plan.conditions.len() > MAX_CONDITIONS {
        return Err(PlanError::ConditionCount);
    }
    for condition in &plan.conditions {
        if let Condition::TimeWindow { time_zone, .. } = condition {
            parse_time_zone(time_zone)?;
        }
    }
    if plan.actions.is_empty() || plan.actions.len() > MAX_ACTIONS {
        return Err(PlanError::ActionCount);
    }
    for (index, action) in plan.actions.iter().enumerate() {
        validate_action(action).map_err(|problem| PlanError::Action(index + 1, problem))?;
    }
    Ok(())
}

fn validate_trigger(trigger: &Trigger, now: DateTime<Utc>) -> Result<(), PlanError> {
    match trigger {
        Trigger::Cron { expr, time_zone } => {
            parse_cron(expr)?;
            parse_time_zone(time_zone)?;
        }
        Trigger::Interval { every_secs } => {
            if !(MIN_INTERVAL_SECS..=MAX_INTERVAL_SECS).contains(every_secs) {
                return Err(PlanError::Interval);
            }
        }
        Trigger::Once { at } => {
            if *at <= now {
                return Err(PlanError::InThePast);
            }
        }
        Trigger::Event {
            event: ServerEvent::EmptyFor { secs },
        } => {
            if !(MIN_EMPTY_SECS..=MAX_EMPTY_SECS).contains(secs) {
                return Err(PlanError::EmptyFor);
            }
        }
        Trigger::Event { .. } => {}
    }
    Ok(())
}

fn validate_action(action: &Action) -> Result<(), String> {
    match action {
        Action::Command { command } => console_line(command.trim_start_matches('/'), "the command"),
        Action::Broadcast { message } => console_line(message, "the message"),
        Action::Restart { warn_secs } | Action::Stop { warn_secs } => warnings(warn_secs),
        Action::Backup { note, .. } => match note {
            Some(note) if note.chars().count() > MAX_NOTE_CHARS => {
                Err(format!("the note must be at most {MAX_NOTE_CHARS} characters"))
            }
            _ => Ok(()),
        },
        Action::Wait { secs } => {
            if (1..=MAX_WAIT_SECS).contains(secs) {
                Ok(())
            } else {
                Err("wait 1 second to 1 hour".into())
            }
        }
        Action::Start => Ok(()),
    }
}

/// One line, not empty, not too long (a newline would send a second command).
fn console_line(text: &str, what: &str) -> Result<(), String> {
    let text = text.trim();
    if text.is_empty() || text.chars().count() > MAX_LINE_CHARS || text.chars().any(char::is_control) {
        return Err(format!(
            "{what} must be one line of 1 to {MAX_LINE_CHARS} characters"
        ));
    }
    Ok(())
}

/// A countdown: at most ten warnings, each up to an hour before, latest last.
fn warnings(warn_secs: &[u64]) -> Result<(), String> {
    let in_order = warn_secs.windows(2).all(|pair| pair[0] > pair[1]);
    let in_range = warn_secs.iter().all(|secs| (1..=MAX_WARN_SECS).contains(secs));
    if warn_secs.len() > MAX_WARNINGS || !in_order || !in_range {
        return Err("warnings are up to 10 times, from at most 1 hour before, earliest first".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use chrono::TimeDelta;

    use super::*;
    use crate::{backups::BackupScope, scheduler::model::OnError};

    fn plan(trigger: Trigger, actions: Vec<Action>) -> SchedulePlan {
        SchedulePlan {
            name: "Nightly".into(),
            enabled: true,
            trigger,
            conditions: vec![],
            actions,
            on_error: OnError::Stop,
            catch_up: false,
        }
    }

    fn daily() -> Trigger {
        Trigger::Cron {
            expr: "0 4 * * *".into(),
            time_zone: "Europe/London".into(),
        }
    }

    fn restart(warn_secs: Vec<u64>) -> Action {
        Action::Restart { warn_secs }
    }

    #[test]
    fn a_typical_plan_is_valid() {
        let nightly = plan(
            daily(),
            vec![
                restart(vec![600, 300, 60, 10]),
                Action::Wait { secs: 120 },
                Action::Backup {
                    scope: BackupScope::World,
                    note: Some("after the restart".into()),
                },
            ],
        );
        assert_eq!(validate(&nightly, Utc::now()), Ok(()));
    }

    #[test]
    fn rejects_bad_triggers() {
        let now = Utc::now();
        let check = |trigger| validate(&plan(trigger, vec![Action::Start]), now);
        assert!(matches!(
            check(Trigger::Cron {
                expr: "every day".into(),
                time_zone: "UTC".into()
            }),
            Err(PlanError::Trigger(TriggerError::Cron(..)))
        ));
        assert!(matches!(
            check(Trigger::Cron {
                expr: "0 4 * * *".into(),
                time_zone: "London".into()
            }),
            Err(PlanError::Trigger(TriggerError::TimeZone(_)))
        ));
        assert_eq!(
            check(Trigger::Interval { every_secs: 30 }),
            Err(PlanError::Interval)
        );
        assert_eq!(
            check(Trigger::Once {
                at: now - TimeDelta::minutes(1)
            }),
            Err(PlanError::InThePast)
        );
        assert_eq!(
            check(Trigger::Event {
                event: ServerEvent::EmptyFor { secs: 5 }
            }),
            Err(PlanError::EmptyFor)
        );
        assert_eq!(
            check(Trigger::Event {
                event: ServerEvent::Crashed
            }),
            Ok(())
        );
    }

    #[test]
    fn rejects_bad_actions_and_says_which() {
        let now = Utc::now();
        let check = |actions| validate(&plan(daily(), actions), now);
        assert_eq!(check(vec![]), Err(PlanError::ActionCount));
        assert_eq!(
            check(vec![Action::Start; MAX_ACTIONS + 1]),
            Err(PlanError::ActionCount)
        );
        let two_lines = Action::Command {
            command: "say hi\nop griefer".into(),
        };
        assert!(matches!(
            check(vec![Action::Start, two_lines]),
            Err(PlanError::Action(2, _))
        ));
        assert!(matches!(
            check(vec![Action::Broadcast { message: "  ".into() }]),
            Err(PlanError::Action(1, _))
        ));
        assert!(
            matches!(check(vec![restart(vec![10, 60])]), Err(PlanError::Action(1, _))),
            "latest last"
        );
        assert!(matches!(
            check(vec![restart(vec![7200])]),
            Err(PlanError::Action(1, _))
        ));
        assert!(matches!(
            check(vec![Action::Wait { secs: 0 }]),
            Err(PlanError::Action(1, _))
        ));
        assert_eq!(
            check(vec![Action::Command {
                command: "/save-all".into()
            }]),
            Ok(()),
            "a leading slash is fine"
        );
    }

    #[test]
    fn rejects_bad_names_and_conditions() {
        let mut nameless = plan(daily(), vec![Action::Start]);
        nameless.name = "   ".into();
        assert_eq!(validate(&nameless, Utc::now()), Err(PlanError::Name));

        let mut odd_window = plan(daily(), vec![Action::Start]);
        odd_window.conditions = vec![Condition::TimeWindow {
            from: chrono::NaiveTime::from_hms_opt(22, 0, 0).unwrap(),
            to: chrono::NaiveTime::from_hms_opt(6, 0, 0).unwrap(),
            time_zone: "Nowhere/Special".into(),
        }];
        assert!(matches!(
            validate(&odd_window, Utc::now()),
            Err(PlanError::Trigger(TriggerError::TimeZone(_)))
        ));
    }
}
