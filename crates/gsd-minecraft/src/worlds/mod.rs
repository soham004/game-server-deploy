//! Where a server keeps its worlds: the `level-name` folder from `server.properties` (default
//! `world`) and, on older Bukkit-style servers (Paper, Purpur), its `_nether` and `_the_end`
//! siblings. Vanilla, and a real Paper 26.3 server, keep the other dimensions inside the main folder,
//! so those siblings only appear on servers that use them.
//!
//! [`info`]: what a world's files say about it; [`service`]: listing and managing worlds.

pub mod info;
pub mod service;

use std::path::Path;

use crate::properties::PropertiesDoc;

const DEFAULT_LEVEL_NAME: &str = "world";

/// The main world's folder name, from `server.properties` (`world` when unset or unusable).
pub fn level_name(server_dir: &Path) -> String {
    let properties = std::fs::read(server_dir.join("server.properties")).unwrap_or_default();
    level_name_in(&PropertiesDoc::from_bytes(&properties))
}

/// The main world's folder name from settings already read.
pub fn level_name_in(properties: &PropertiesDoc) -> String {
    properties
        .get("level-name")
        .filter(|name| is_plain_folder_name(name))
        .unwrap_or(DEFAULT_LEVEL_NAME)
        .to_owned()
}

/// The world folders a server uses: the main world plus its nether and end folders. All three
/// are named even when they don't exist yet (a fresh server, or vanilla), so callers that skip
/// worlds skip them all; callers that copy worlds check what exists.
pub fn world_folders(server_dir: &Path) -> Vec<String> {
    let world = level_name(server_dir);
    vec![format!("{world}_nether"), format!("{world}_the_end"), world]
}

/// A single folder name: `level-name` may hold anything, and must not point elsewhere.
pub(crate) fn is_plain_folder_name(name: &str) -> bool {
    !name.is_empty() && name != "." && name != ".." && !name.contains(['/', '\\', '\0'])
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn follows_level_name_and_falls_back_to_world() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            world_folders(dir.path()),
            ["world_nether", "world_the_end", "world"]
        );

        std::fs::write(dir.path().join("server.properties"), "level-name=survival\n").unwrap();
        assert_eq!(level_name(dir.path()), "survival");
        assert_eq!(
            world_folders(dir.path()),
            ["survival_nether", "survival_the_end", "survival"]
        );
    }

    #[test]
    fn a_level_name_that_points_elsewhere_is_ignored() {
        let dir = tempfile::tempdir().unwrap();
        // As written in the file: `\\` is how server.properties spells one backslash.
        for bad in ["../other", "a/b", "..", "", r"c:\\x"] {
            std::fs::write(
                dir.path().join("server.properties"),
                format!("level-name={bad}\n"),
            )
            .unwrap();
            assert_eq!(level_name(dir.path()), "world", "{bad:?}");
        }
    }
}
