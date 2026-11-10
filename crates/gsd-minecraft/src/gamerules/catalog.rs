//! What the app knows about game rules: a label, a group, the default and a description, from
//! `gamerules.json`. Only an aid: the rules themselves come from the server, so ones added by a newer
//! version or a mod still show, under "Other".

use std::{collections::HashMap, sync::LazyLock};

use serde::Deserialize;

use super::{RuleType, RuleValue};

/// Groups in the order they are shown; `other` holds rules the catalog doesn't know.
pub const CATEGORIES: [&str; 8] = [
    "player", "mobs", "spawning", "drops", "world", "chat", "misc", "other",
];

#[derive(Debug, Clone, Deserialize)]
pub struct Entry {
    #[serde(rename = "type")]
    pub kind: RuleType,
    pub default: RuleValue,
    pub category: String,
    pub label: String,
    pub description: String,
}

#[derive(Deserialize)]
struct File {
    rules: Vec<Named>,
    legacy: Vec<Legacy>,
}

#[derive(Deserialize)]
struct Named {
    name: String,
    #[serde(flatten)]
    entry: Entry,
}

/// A name from before 1.21.11: the same as a newer rule, or one of its own.
#[derive(Deserialize)]
struct Legacy {
    name: String,
    #[serde(rename = "as")]
    same_as: Option<String>,
    #[serde(flatten)]
    entry: Option<Entry>,
}

static CATALOG: LazyLock<HashMap<String, Entry>> = LazyLock::new(|| {
    let file: File = serde_json::from_str(include_str!("gamerules.json"))
        .unwrap_or_else(|error| unreachable!("gamerules.json is part of the source: {error}"));
    let mut entries: HashMap<String, Entry> = file
        .rules
        .into_iter()
        .map(|named| (named.name, named.entry))
        .collect();
    for legacy in file.legacy {
        let entry = match (legacy.same_as, legacy.entry) {
            (Some(target), _) => entries.get(&target).cloned(),
            (None, entry) => entry,
        };
        if let Some(entry) = entry {
            entries.insert(legacy.name, entry);
        }
    }
    entries
});

/// What the catalog says about a rule, by the name the server uses (without `minecraft:`).
pub fn describe(name: &str) -> Option<&'static Entry> {
    CATALOG.get(name)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use crate::gamerules::world::{Storage, values_in};
    use crate::nbt;

    fn rules_in(fixture: &str, storage: Storage) -> Vec<(String, RuleValue)> {
        let bytes =
            std::fs::read(format!("{}/tests/fixtures/{fixture}", env!("CARGO_MANIFEST_DIR"))).unwrap();
        values_in(storage, &nbt::read_file(&bytes).unwrap().1)
    }

    /// The catalog's defaults are what a real new world has, for 26.3 and for 1.20.1, and it knows
    /// every rule of both.
    #[test]
    fn defaults_match_real_new_worlds() {
        for (fixture, storage, count) in [
            ("game_rules_26.3.dat", Storage::Modern, 58),
            ("level_1.20.1.dat", Storage::Legacy, 45),
        ] {
            let rules = rules_in(fixture, storage);
            assert_eq!(rules.len(), count, "{fixture}");
            for (name, value) in rules {
                let entry =
                    describe(&name).unwrap_or_else(|| panic!("{fixture}: {name} isn't in the catalog"));
                assert_eq!(entry.default, value, "{fixture}: {name}");
                assert_eq!(entry.kind, value.kind(), "{fixture}: {name}");
            }
        }
    }

    #[test]
    fn every_entry_is_in_a_known_group_and_old_names_point_somewhere() {
        let file: serde_json::Value = serde_json::from_str(include_str!("gamerules.json")).unwrap();
        for legacy in file["legacy"].as_array().unwrap() {
            let name = legacy["name"].as_str().unwrap();
            assert!(describe(name).is_some(), "{name}");
        }
        for (name, entry) in CATALOG.iter() {
            assert!(
                CATEGORIES.contains(&entry.category.as_str()),
                "{name}: {}",
                entry.category
            );
            assert_eq!(entry.default.kind(), entry.kind, "{name}");
        }
        assert_eq!(
            describe("keepInventory").unwrap().label,
            describe("keep_inventory").unwrap().label
        );
        assert_eq!(
            describe("disableRaids").unwrap().default,
            RuleValue::Bool(false),
            "the opposite of raids"
        );
    }
}
