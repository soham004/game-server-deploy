//! Game rules: the server's own list, with what the app knows about each, and changes made the way the
//! server's state allows.
//!
//! - **Running, 1.21.9 or later:** the management protocol (`minecraft:gamerules`, `…/update`): live values.
//! - **Running, older (or the protocol out of reach):** `gamerule <name> <value>` in the console, then a
//!   save, after which the world's files show the change. Values shown are as of the last save.
//! - **Stopped:** the world's files ([`world`]), which the server reads when it starts.
//!
//! [`catalog`] adds labels, groups and defaults; rules it doesn't know (newer versions, mods) still show.

pub mod catalog;
pub mod service;
pub mod world;

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::{
    control::{ControlError, mcsmp::McsmpError},
    nbt::NbtError,
    players::ActionChannel,
};
use gsd_core::{backups::QuiesceError, files::FilesError, instances::InstanceError};

pub use service::{GameruleService, Updated};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuleType {
    Bool,
    Int,
}

/// A rule's value: true or false, or a whole number.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RuleValue {
    Bool(bool),
    Int(i64),
}

impl RuleValue {
    pub fn kind(self) -> RuleType {
        match self {
            Self::Bool(_) => RuleType::Bool,
            Self::Int(_) => RuleType::Int,
        }
    }

    /// As older worlds and the console write it.
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "true" => Some(Self::Bool(true)),
            "false" => Some(Self::Bool(false)),
            _ => text
                .parse::<i32>()
                .ok()
                .map(|number| Self::Int(i64::from(number))),
        }
    }

    /// The game's numbers are 32-bit.
    pub fn fits_an_int(self) -> bool {
        match self {
            Self::Bool(_) => true,
            Self::Int(number) => i32::try_from(number).is_ok(),
        }
    }
}

impl fmt::Display for RuleValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Bool(on) => write!(f, "{on}"),
            Self::Int(number) => write!(f, "{number}"),
        }
    }
}

/// Where the values shown come from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ValuesFrom {
    /// The running server, as it is now.
    Server,
    /// The world's files, as the running server last saved them.
    LastSave,
    /// The stopped server's world.
    Files,
}

/// One rule, as the server has it, with what the catalog knows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Rule {
    /// As the server names it, without `minecraft:`.
    pub name: String,
    #[serde(rename = "type")]
    pub kind: RuleType,
    pub value: RuleValue,
    pub default: Option<RuleValue>,
    pub category: &'static str,
    pub label: Option<&'static str>,
    pub description: Option<&'static str>,
}

#[derive(Debug, Clone, Serialize)]
pub struct GamerulesView {
    /// How a change would be made now; `None` while the server starts or stops.
    pub channel: Option<ActionChannel>,
    /// `None` when there is no world yet: it is made at the first start.
    pub values: Option<ValuesFrom>,
    /// The world folder the rules are from.
    pub world: String,
    pub rules: Vec<Rule>,
}

#[derive(Debug, thiserror::Error)]
pub enum GameruleError {
    #[error("wait until the server has started or stopped")]
    Busy,
    #[error("the server has no world yet: it makes one when it first starts")]
    NoWorld,
    #[error("there is no game rule called \"{0}\" here")]
    UnknownRule(String),
    #[error("\"{name}\" takes another kind of value")]
    WrongType { name: String },
    #[error("nothing to change")]
    NothingToChange,
    #[error("the server refused: {0}")]
    Refused(String),
    #[error("the server didn't confirm it; its console may say why")]
    NotConfirmed,
    #[error("the server didn't save its world: {0}")]
    Save(#[from] QuiesceError),
    #[error("the world's file can't be read: {0}")]
    Nbt(#[from] NbtError),
    #[error(transparent)]
    Files(#[from] FilesError),
    #[error(transparent)]
    Control(#[from] ControlError),
    #[error(transparent)]
    Instance(#[from] InstanceError),
}

impl From<McsmpError> for GameruleError {
    fn from(error: McsmpError) -> Self {
        match error {
            McsmpError::Rpc { message, .. } => Self::Refused(message),
            other => Self::Control(ControlError::Mcsmp(other)),
        }
    }
}

/// A rule with what the catalog says about it, when the kinds agree.
pub fn describe(name: &str, value: RuleValue) -> Rule {
    let entry = catalog::describe(name).filter(|entry| entry.kind == value.kind());
    Rule {
        name: name.to_owned(),
        kind: value.kind(),
        value,
        default: entry.map(|entry| entry.default),
        category: entry.map_or("other", |entry| {
            catalog::CATEGORIES
                .iter()
                .find(|category| **category == entry.category)
                .copied()
                .unwrap_or("other")
        }),
        label: entry.map(|entry| entry.label.as_str()),
        description: entry.map(|entry| entry.description.as_str()),
    }
}

/// In the order shown: by group, then by label (or name).
pub fn sorted(mut rules: Vec<Rule>) -> Vec<Rule> {
    let group = |rule: &Rule| {
        catalog::CATEGORIES
            .iter()
            .position(|c| *c == rule.category)
            .unwrap_or(usize::MAX)
    };
    rules.sort_by(|a, b| {
        group(a).cmp(&group(b)).then_with(|| {
            a.label
                .unwrap_or(&a.name)
                .to_lowercase()
                .cmp(&b.label.unwrap_or(&b.name).to_lowercase())
        })
    });
    rules
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_read_and_write_as_the_game_does() {
        assert_eq!(RuleValue::parse("true"), Some(RuleValue::Bool(true)));
        assert_eq!(RuleValue::parse("-1"), Some(RuleValue::Int(-1)));
        assert_eq!(RuleValue::parse("3000000000"), None, "beyond 32 bits");
        assert_eq!(RuleValue::parse("yes"), None);
        assert_eq!(RuleValue::Int(65536).to_string(), "65536");
        assert!(!RuleValue::Int(i64::from(i32::MIN) - 1).fits_an_int());
        let parsed: RuleValue = serde_json::from_str("true").unwrap_or(RuleValue::Int(0));
        assert_eq!(parsed, RuleValue::Bool(true));
    }

    #[test]
    fn known_rules_get_their_label_and_unknown_ones_go_to_other() {
        let known = describe("keep_inventory", RuleValue::Bool(true));
        assert_eq!(
            (known.category, known.default),
            ("player", Some(RuleValue::Bool(false)))
        );
        let modded = describe("somemod:faster_crops", RuleValue::Int(2));
        assert_eq!(
            (modded.category, modded.label, modded.default),
            ("other", None, None)
        );
        let odd = describe("keep_inventory", RuleValue::Int(1));
        assert_eq!(
            odd.category, "other",
            "a kind the catalog doesn't expect isn't described by it"
        );
        let order: Vec<_> = sorted(vec![modded, known])
            .into_iter()
            .map(|rule| rule.name)
            .collect();
        assert_eq!(order, ["keep_inventory", "somemod:faster_crops"]);
    }
}
