//! What a world's files say about it, read from `level.dat` (and, since 1.21.11, the seed from
//! `data/minecraft/world_gen_settings.dat`). Fields a version doesn't have are left empty.
//!
//! Where things are, seen in real files:
//! - version: `Data.Version.Name` (both); last played: `Data.LastPlayed`, milliseconds; game mode: `Data.GameType`.
//! - 26.3: `Data.difficulty_settings.{difficulty (a name), hardcore}`; seed in the separate file, `data.seed`.
//! - 1.20.1: `Data.Difficulty` (0–3), `Data.hardcore`, `Data.WorldGenSettings.seed`. Before 1.16:
//!   `Data.RandomSeed`.

use chrono::{DateTime, Utc};
use serde::Serialize;

use crate::nbt::{Compound, Tag};

const GAME_MODES: [&str; 4] = ["survival", "creative", "adventure", "spectator"];
const DIFFICULTIES: [&str; 4] = ["peaceful", "easy", "normal", "hard"];

/// The facts of one world.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct WorldFacts {
    /// The name inside the world (`LevelName`), which can differ from its folder's.
    pub name: Option<String>,
    pub version: Option<String>,
    pub last_played: Option<DateTime<Utc>>,
    pub game_mode: Option<&'static str>,
    pub difficulty: Option<String>,
    pub hardcore: bool,
    /// As text: seeds are 64-bit, more than a browser's numbers hold exactly.
    pub seed: Option<String>,
    pub datapacks: Datapacks,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Datapacks {
    pub enabled: Vec<String>,
    pub disabled: Vec<String>,
}

/// The facts in a `level.dat` root, and the root of `world_gen_settings.dat` where there is one.
pub fn facts(level: &Compound, world_gen: Option<&Compound>) -> WorldFacts {
    let Some(data) = level.compound_at(&["Data"]) else {
        return WorldFacts::default();
    };
    let difficulty_settings = data.compound_at(&["difficulty_settings"]);
    let difficulty = difficulty_settings
        .and_then(|settings| string(settings.get("difficulty")))
        .or_else(|| {
            integer(data.get("Difficulty"))
                .and_then(|n| usize::try_from(n).ok())
                .and_then(|n| DIFFICULTIES.get(n))
                .map(|name| (*name).to_owned())
        });
    let hardcore = difficulty_settings
        .and_then(|settings| integer(settings.get("hardcore")))
        .or_else(|| integer(data.get("hardcore")))
        .is_some_and(|flag| flag != 0);
    let seed = world_gen
        .and_then(|root| integer(root.compound_at(&["data"])?.get("seed")))
        .or_else(|| integer(data.compound_at(&["WorldGenSettings"])?.get("seed")))
        .or_else(|| integer(data.get("RandomSeed")));
    WorldFacts {
        name: string(data.get("LevelName")),
        version: data
            .compound_at(&["Version"])
            .and_then(|version| string(version.get("Name"))),
        last_played: integer(data.get("LastPlayed")).and_then(DateTime::from_timestamp_millis),
        game_mode: integer(data.get("GameType"))
            .and_then(|n| usize::try_from(n).ok())
            .and_then(|n| GAME_MODES.get(n).copied()),
        difficulty,
        hardcore,
        seed: seed.map(|seed| seed.to_string()),
        datapacks: data
            .compound_at(&["DataPacks"])
            .map(|packs| Datapacks {
                enabled: strings(packs.get("Enabled")),
                disabled: strings(packs.get("Disabled")),
            })
            .unwrap_or_default(),
    }
}

fn string(tag: Option<&Tag>) -> Option<String> {
    match tag? {
        Tag::String(text) => Some(text.clone()),
        _ => None,
    }
}

fn integer(tag: Option<&Tag>) -> Option<i64> {
    match tag? {
        Tag::Byte(n) => Some(i64::from(*n)),
        Tag::Short(n) => Some(i64::from(*n)),
        Tag::Int(n) => Some(i64::from(*n)),
        Tag::Long(n) => Some(*n),
        _ => None,
    }
}

fn strings(tag: Option<&Tag>) -> Vec<String> {
    match tag {
        Some(Tag::List(_, items)) => items.iter().filter_map(|item| string(Some(item))).collect(),
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use crate::nbt;

    fn fixture(name: &str) -> Compound {
        let bytes = std::fs::read(format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap();
        nbt::read_file(&bytes).unwrap().1
    }

    #[test]
    fn a_real_26_3_world() {
        let level = fixture("level_26.3.dat");
        let world_gen = Compound(vec![(
            "data".into(),
            Tag::Compound(Compound(vec![(
                "seed".into(),
                Tag::Long(8_455_220_631_527_754_161),
            )])),
        )]);
        let facts = facts(&level, Some(&world_gen));
        assert_eq!(facts.version.as_deref(), Some("26.3"));
        assert_eq!(facts.name.as_deref(), Some("world"));
        assert_eq!(facts.game_mode, Some("survival"));
        assert_eq!(facts.difficulty.as_deref(), Some("easy"));
        assert!(!facts.hardcore);
        assert_eq!(
            facts.seed.as_deref(),
            Some("8455220631527754161"),
            "all 64 bits, as text"
        );
        assert!(facts.last_played.is_some());
        assert_eq!(facts.datapacks.enabled, ["vanilla"]);
        assert!(facts.datapacks.disabled.contains(&"trade_rebalance".to_owned()));
        assert_eq!(
            super::facts(&level, None).seed,
            None,
            "26.3 keeps its seed elsewhere"
        );
    }

    #[test]
    fn a_real_1_20_1_world() {
        let facts = facts(&fixture("level_1.20.1.dat"), None);
        assert_eq!(facts.version.as_deref(), Some("1.20.1"));
        assert_eq!(facts.difficulty.as_deref(), Some("easy"));
        assert_eq!(facts.seed.as_deref(), Some("8984527508982599705"));
        assert_eq!(facts.datapacks.disabled, ["bundle"]);
    }

    #[test]
    fn odd_files_give_what_they_have() {
        assert_eq!(facts(&Compound::default(), None), WorldFacts::default());
        let old = Compound(vec![(
            "Data".into(),
            Tag::Compound(Compound(vec![
                ("RandomSeed".into(), Tag::Long(-5)),
                ("GameType".into(), Tag::Int(9)),
                ("Difficulty".into(), Tag::Byte(3)),
            ])),
        )]);
        let facts = facts(&old, None);
        assert_eq!(facts.seed.as_deref(), Some("-5"), "before 1.16");
        assert_eq!(facts.game_mode, None, "an unknown mode isn't guessed");
        assert_eq!(facts.difficulty.as_deref(), Some("hard"));
    }
}
