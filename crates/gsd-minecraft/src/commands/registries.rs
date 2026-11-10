//! The ids arguments take (items, blocks, enchantments, biomes, …), from the vanilla server itself.
//!
//! - Built-in registries are in the data generator's `reports/registries.json`
//!   (`{"minecraft:item": {"entries": {"minecraft:stone": {…}}}}`).
//! - Data-driven ones (enchantments, biomes, advancements, recipes, loot tables, …) aren't: they
//!   are the game's own data pack, files in its jar (`data/minecraft/enchantment/sharpness.json`),
//!   and their tags (`data/minecraft/tags/item/logs.json`, written `#minecraft:logs`).
//!
//! Only registries the command tree refers to are kept, plus the few the completer always uses.

use std::{
    collections::{BTreeMap, BTreeSet},
    io::{Read, Seek},
};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::tree::CommandTree;

/// Registries completed whatever the tree's arguments say (items, blocks, particles, sounds).
const ALWAYS: [&str; 5] = [
    "minecraft:item",
    "minecraft:block",
    "minecraft:particle_type",
    "minecraft:sound_event",
    "minecraft:entity_type",
];
/// More ids than a registry has (sounds, the largest built-in one, about 2,000; recipes 2,100).
const MAX_PER_REGISTRY: usize = 20_000;
/// The game's jar is about 60 MB; a nested jar bigger than this isn't one.
const MAX_INNER_JAR: u64 = 512 * 1024 * 1024;

/// Ids by registry (`minecraft:item`); tags by registry as `#minecraft:item`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Registries {
    lists: BTreeMap<String, Vec<String>>,
}

impl Registries {
    /// The ids of `registry` (`minecraft:item`), sorted.
    pub fn ids(&self, registry: &str) -> &[String] {
        self.lists.get(registry).map_or(&[], Vec::as_slice)
    }

    /// The tags of `registry`, each `#minecraft:logs`.
    pub fn tags(&self, registry: &str) -> &[String] {
        self.lists.get(&format!("#{registry}")).map_or(&[], Vec::as_slice)
    }

    pub fn len(&self) -> usize {
        self.lists.len()
    }

    pub fn is_empty(&self) -> bool {
        self.lists.is_empty()
    }

    /// The registries worth keeping for `tree`: those its arguments name, and [`ALWAYS`].
    pub fn wanted(tree: &CommandTree) -> BTreeSet<String> {
        let mut wanted: BTreeSet<String> = ALWAYS.iter().map(|name| (*name).to_owned()).collect();
        for index in 0..tree.len() {
            #[allow(clippy::cast_possible_truncation)] // the tree has at most MAX_NODES nodes
            let node = tree.node(index as u32);
            if let Some(registry) = node.property("registry").and_then(Value::as_str) {
                wanted.insert(registry.to_owned());
            }
        }
        // Names the completer looks for by argument name (see complete.rs).
        for extra in [
            "minecraft:advancement",
            "minecraft:recipe",
            "minecraft:loot_table",
        ] {
            wanted.insert(extra.to_owned());
        }
        wanted
    }

    /// Adds the built-in registries in `registries.json` that `wanted` has.
    pub fn add_report(&mut self, report: &Value, wanted: &BTreeSet<String>) {
        let Some(registries) = report.as_object() else {
            return;
        };
        for (name, registry) in registries {
            if !wanted.contains(name) {
                continue;
            }
            if let Some(entries) = registry.get("entries").and_then(Value::as_object) {
                let ids = entries.keys().take(MAX_PER_REGISTRY).cloned().collect();
                self.insert(name.clone(), ids);
            }
        }
    }

    /// Adds the data-driven registries (and every wanted registry's tags) from the game's jar.
    ///
    /// Since 1.18 the server jar is a bundler with the game in `META-INF/versions/<v>/…jar`;
    /// before, the game's files are in the jar itself.
    pub fn add_from_jar<R: Read + Seek>(
        &mut self,
        jar: R,
        wanted: &BTreeSet<String>,
    ) -> zip::result::ZipResult<()> {
        let mut outer = zip::ZipArchive::new(jar)?;
        let outer_names = entry_names(&outer);
        let inner_name = outer_names
            .iter()
            .find(|name| {
                name.starts_with("META-INF/versions/")
                    && std::path::Path::new(name).extension().is_some_and(|e| e == "jar")
            })
            .cloned();
        let Some(inner_name) = inner_name else {
            self.add_data_files(outer_names.iter().map(String::as_str), wanted);
            return Ok(());
        };
        let mut entry = outer.by_name(&inner_name)?;
        if entry.size() > MAX_INNER_JAR {
            return Err(zip::result::ZipError::InvalidArchive(
                "the game's jar is too large".into(),
            ));
        }
        let mut bytes = Vec::new();
        (&mut entry).take(MAX_INNER_JAR).read_to_end(&mut bytes)?;
        let inner = zip::ZipArchive::new(std::io::Cursor::new(bytes))?;
        self.add_data_files(entry_names(&inner).iter().map(String::as_str), wanted);
        Ok(())
    }

    /// Ids from data pack file names: `data/minecraft/<registry>/<path>.json` and
    /// `data/minecraft/tags/<registry>/<path>.json`.
    pub fn add_data_files<'a>(&mut self, names: impl Iterator<Item = &'a str>, wanted: &BTreeSet<String>) {
        let mut found: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for name in names {
            let Some(rest) = name.strip_prefix("data/minecraft/") else {
                continue;
            };
            let Some(rest) = rest.strip_suffix(".json") else {
                continue;
            };
            let (is_tag, rest) = match rest.strip_prefix("tags/") {
                Some(rest) => (true, rest),
                None => (false, rest),
            };
            let Some((registry, path)) = split_registry(rest) else {
                continue;
            };
            let registry = format!("minecraft:{}", singular(&registry));
            if !wanted.contains(&registry) {
                continue;
            }
            let key = if is_tag { format!("#{registry}") } else { registry };
            let id = if is_tag {
                format!("#minecraft:{path}")
            } else {
                format!("minecraft:{path}")
            };
            found.entry(key).or_default().insert(id);
        }
        for (key, ids) in found {
            // Built-in registries come from the report; the jar only adds what it lacks.
            if !self.lists.contains_key(&key) {
                self.insert(key, ids.into_iter().take(MAX_PER_REGISTRY).collect());
            }
        }
    }

    fn insert(&mut self, name: String, mut ids: Vec<String>) {
        ids.sort();
        ids.dedup();
        self.lists.insert(name, ids);
    }
}

/// The entry names of an archive (names that aren't valid text are skipped).
fn entry_names<R: Read + Seek>(archive: &zip::ZipArchive<R>) -> Vec<String> {
    archive
        .file_names()
        .filter_map(|name| name.ok().map(std::borrow::Cow::into_owned))
        .collect()
}

/// `worldgen/biome/plains` →(`worldgen/biome`, `plains`); `enchantment/sharpness` →
/// (`enchantment`, `sharpness`). Worldgen registries are two names deep.
fn split_registry(rest: &str) -> Option<(String, &str)> {
    let (first, after) = rest.split_once('/')?;
    if first == "worldgen" {
        let (second, path) = after.split_once('/')?;
        return Some((format!("worldgen/{second}"), path));
    }
    Some((first.to_owned(), after))
}

/// Folder names before 1.21 were plural (`tags/blocks`, `loot_tables`, `advancements`).
fn singular(registry: &str) -> String {
    match registry {
        "blocks" | "items" | "fluids" | "functions" | "loot_tables" | "advancements" | "recipes"
        | "entity_types" | "game_events" | "predicates" | "item_modifiers" => {
            registry.trim_end_matches('s').to_owned()
        }
        other => other.to_owned(),
    }
}
