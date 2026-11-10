//! Game rules in a world's files, as the server last saved them.
//!
//! - Since 1.21.11 (and 26.x): `<world>/data/minecraft/game_rules.dat`, typed: a byte for true or false,
//!   an int for numbers, keyed `minecraft:keep_inventory`.
//! - Before: `<world>/level.dat`, `Data.GameRules`, every value a string (`"true"`, `"3"`).
//!
//! Written only while the server is stopped (it holds them in memory and saves over them), through the
//! file manager, and only the values that change: the rest of the file stays as it was.

use gsd_core::files::{FilesError, SafeRelPath, ServerFiles};

use super::{GameruleError, RuleValue};
use crate::nbt::{self, Compound, Tag};

const MODERN_FILE: &str = "data/minecraft/game_rules.dat";
const LEGACY_FILE: &str = "level.dat";
const LEGACY_PATH: [&str; 2] = ["Data", "GameRules"];
const NAMESPACE: &str = "minecraft:";

/// Where a world keeps its rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Storage {
    Modern,
    Legacy,
}

impl Storage {
    fn file(self, level_name: &str) -> String {
        match self {
            Self::Modern => format!("{level_name}/{MODERN_FILE}"),
            Self::Legacy => format!("{level_name}/{LEGACY_FILE}"),
        }
    }
}

/// The world's rules, and where they are; `None` before the server has made its world.
pub async fn read(
    files: &ServerFiles,
    level_name: &str,
) -> Result<Option<(Storage, Vec<(String, RuleValue)>)>, GameruleError> {
    for storage in [Storage::Modern, Storage::Legacy] {
        let path = SafeRelPath::parse(&storage.file(level_name)).map_err(FilesError::from)?;
        match files.read_bytes(path).await {
            Ok((bytes, _)) => {
                let (_, root) = nbt::read_file(&bytes)?;
                // A level.dat from 1.21.11 on has no rules in it: its world has the other file.
                return Ok(Some((storage, values_in(storage, &root))));
            }
            Err(FilesError::NotFound(_)) => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(None)
}

/// The rules in a file's root. Names lose `minecraft:`; a mod's own namespace stays. Values that aren't
/// true or false or a number are left out (a mod's own kind of rule).
pub fn values_in(storage: Storage, root: &Compound) -> Vec<(String, RuleValue)> {
    match storage {
        Storage::Modern => root
            .compound_at(&["data"])
            .map(|rules| {
                rules
                    .0
                    .iter()
                    .filter_map(|(key, tag)| {
                        let value = match tag {
                            Tag::Byte(byte) => RuleValue::Bool(*byte != 0),
                            Tag::Int(number) => RuleValue::Int(i64::from(*number)),
                            _ => return None,
                        };
                        Some((short_name(key).to_owned(), value))
                    })
                    .collect()
            })
            .unwrap_or_default(),
        Storage::Legacy => root
            .compound_at(&LEGACY_PATH)
            .map(|rules| {
                rules
                    .0
                    .iter()
                    .filter_map(|(key, tag)| match tag {
                        Tag::String(text) => RuleValue::parse(text).map(|value| (key.clone(), value)),
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default(),
    }
}

fn short_name(key: &str) -> &str {
    key.strip_prefix(NAMESPACE).unwrap_or(key)
}

/// Sets the given rules in a file's root; each must already be there, with a value of the same kind.
/// All of them are checked first: a refused one changes nothing.
pub fn apply(
    storage: Storage,
    root: &mut Compound,
    changes: &[(String, RuleValue)],
) -> Result<(), GameruleError> {
    let path: &[&str] = match storage {
        Storage::Modern => &["data"],
        Storage::Legacy => &LEGACY_PATH,
    };
    let unknown = |name: &str| GameruleError::UnknownRule(name.to_owned());
    let rules = root
        .compound_at_mut(path)
        .ok_or_else(|| unknown(changes.first().map_or("", |(name, _)| name)))?;
    let mut updates = Vec::with_capacity(changes.len());
    for (name, value) in changes {
        let index = rules
            .0
            .iter()
            .position(|(key, _)| match storage {
                Storage::Modern => short_name(key) == name,
                Storage::Legacy => key == name,
            })
            .ok_or_else(|| unknown(name))?;
        let wrong_type = || GameruleError::WrongType { name: name.clone() };
        let tag = match (storage, &rules.0[index].1, value) {
            (Storage::Modern, Tag::Byte(_), RuleValue::Bool(on)) => Tag::Byte(i8::from(*on)),
            (Storage::Modern, Tag::Int(_), RuleValue::Int(number)) => {
                Tag::Int(i32::try_from(*number).map_err(|_| wrong_type())?)
            }
            (Storage::Legacy, Tag::String(old), _)
                if RuleValue::parse(old).map(RuleValue::kind) == Some(value.kind())
                    && value.fits_an_int() =>
            {
                Tag::String(value.to_string())
            }
            _ => return Err(wrong_type()),
        };
        updates.push((index, tag));
    }
    for (index, tag) in updates {
        rules.0[index].1 = tag;
    }
    Ok(())
}

/// Writes the changes to the stopped server's world. Refused if the file changed meanwhile.
pub async fn write(
    files: &ServerFiles,
    level_name: &str,
    storage: Storage,
    changes: &[(String, RuleValue)],
) -> Result<(), GameruleError> {
    let path = SafeRelPath::parse(&storage.file(level_name)).map_err(FilesError::from)?;
    let (bytes, revision) = files.read_bytes(path.clone()).await?;
    let (name, mut root) = nbt::read_file(&bytes)?;
    apply(storage, &mut root, changes)?;
    files
        .write_bytes(path, nbt::write_file(&name, &root), Some(revision))
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn fixture(name: &str) -> Compound {
        let bytes = std::fs::read(format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap();
        nbt::read_file(&bytes).unwrap().1
    }

    fn value(storage: Storage, root: &Compound, name: &str) -> Option<RuleValue> {
        values_in(storage, root)
            .into_iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v)
    }

    #[test]
    fn real_files_are_read_with_their_types() {
        let modern = fixture("game_rules_26.3.dat");
        assert_eq!(
            value(Storage::Modern, &modern, "keep_inventory"),
            Some(RuleValue::Bool(false))
        );
        assert_eq!(
            value(Storage::Modern, &modern, "random_tick_speed"),
            Some(RuleValue::Int(3))
        );
        let legacy = fixture("level_1.20.1.dat");
        assert_eq!(
            value(Storage::Legacy, &legacy, "keepInventory"),
            Some(RuleValue::Bool(false))
        );
        assert_eq!(
            value(Storage::Legacy, &legacy, "randomTickSpeed"),
            Some(RuleValue::Int(3))
        );
        assert_eq!(
            values_in(Storage::Legacy, &fixture("level_26.3.dat")),
            [],
            "26.3 keeps them elsewhere"
        );
    }

    #[test]
    fn changes_keep_each_files_own_types() {
        let mut modern = fixture("game_rules_26.3.dat");
        let changes = [
            ("keep_inventory".into(), RuleValue::Bool(true)),
            ("random_tick_speed".into(), RuleValue::Int(10)),
        ];
        apply(Storage::Modern, &mut modern, &changes).unwrap();
        let rules = modern.compound_at(&["data"]).unwrap();
        assert_eq!(rules.get("minecraft:keep_inventory"), Some(&Tag::Byte(1)));
        assert_eq!(rules.get("minecraft:random_tick_speed"), Some(&Tag::Int(10)));

        let mut legacy = fixture("level_1.20.1.dat");
        apply(
            Storage::Legacy,
            &mut legacy,
            &[("keepInventory".into(), RuleValue::Bool(true))],
        )
        .unwrap();
        assert_eq!(
            legacy.compound_at(&LEGACY_PATH).unwrap().get("keepInventory"),
            Some(&Tag::String("true".into()))
        );
        let before = nbt::to_bytes("", &fixture("level_1.20.1.dat")).len();
        assert_eq!(
            nbt::to_bytes("", &legacy).len(),
            before - 1,
            "\"false\" became \"true\", one letter shorter, and nothing else changed"
        );
    }

    #[test]
    fn unknown_rules_and_wrong_kinds_change_nothing() {
        let mut modern = fixture("game_rules_26.3.dat");
        let original = modern.clone();
        let batch = [
            ("keep_inventory".into(), RuleValue::Bool(true)),
            ("no_such_rule".into(), RuleValue::Bool(true)),
        ];
        assert!(matches!(
            apply(Storage::Modern, &mut modern, &batch),
            Err(GameruleError::UnknownRule(_))
        ));
        assert!(matches!(
            apply(
                Storage::Modern,
                &mut modern,
                &[("keep_inventory".into(), RuleValue::Int(1))]
            ),
            Err(GameruleError::WrongType { .. })
        ));
        assert!(matches!(
            apply(
                Storage::Modern,
                &mut modern,
                &[(
                    "random_tick_speed".into(),
                    RuleValue::Int(i64::from(i32::MAX) + 1)
                )]
            ),
            Err(GameruleError::WrongType { .. })
        ));
        let mut legacy = fixture("level_1.20.1.dat");
        assert!(matches!(
            apply(
                Storage::Legacy,
                &mut legacy,
                &[("randomTickSpeed".into(), RuleValue::Bool(true))]
            ),
            Err(GameruleError::WrongType { .. })
        ));
        assert_eq!(
            modern, original,
            "a refused batch changes nothing, its good half included"
        );
    }
}
