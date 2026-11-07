//! [`SafeRelPath`]: a path inside a server directory, checked lexically.
//!
//! This is the first of two guards: it rejects anything that could name a place outside the
//! directory (`..`, NUL, backslashes, Windows drive prefixes) and normalises the rest. The second
//! guard is `cap-std`, which resolves every path beneath the directory handle and refuses to
//! follow symlinks out of it, race-free (see `ops.rs`).

use std::{fmt, path::Path};

use serde::Serialize;

/// Longest path accepted (bytes), and longest single name (the Linux limits).
const MAX_PATH_BYTES: usize = 4096;
const MAX_NAME_BYTES: usize = 255;

/// A normalised relative path; empty means the directory itself. Always uses `/`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct SafeRelPath(String);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PathError {
    #[error("paths may not contain `..`")]
    ParentDir,
    #[error("paths may not contain NUL characters")]
    Nul,
    #[error("use `/` to separate folders, not `\\`")]
    Backslash,
    #[error("`{0}` is not allowed in a file name")]
    ForbiddenName(String),
    #[error("the path is too long")]
    TooLong,
    #[error("a name is needed here")]
    Empty,
}

impl SafeRelPath {
    /// The directory itself.
    pub fn root() -> Self {
        Self::default()
    }

    /// Parses user input like `world/region`, `/logs/latest.log` or `./config/`. Leading and
    /// repeated slashes and `.` components are dropped.
    pub fn parse(input: &str) -> Result<Self, PathError> {
        if input.len() > MAX_PATH_BYTES {
            return Err(PathError::TooLong);
        }
        let mut parts: Vec<&str> = Vec::new();
        for part in input.split('/') {
            if part.is_empty() || part == "." {
                continue;
            }
            check_name(part)?;
            parts.push(part);
        }
        Ok(Self(parts.join("/")))
    }

    pub fn is_root(&self) -> bool {
        self.0.is_empty()
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The path for `cap_std::fs::Dir` calls (`.` for the root).
    pub fn as_path(&self) -> &Path {
        Path::new(if self.0.is_empty() { "." } else { &self.0 })
    }

    /// The last component, if this isn't the root.
    pub fn file_name(&self) -> Option<&str> {
        self.0.rsplit('/').next().filter(|name| !name.is_empty())
    }

    /// The containing directory (the root's parent is `None`).
    pub fn parent(&self) -> Option<Self> {
        if self.is_root() {
            return None;
        }
        Some(Self(
            self.0
                .rsplit_once('/')
                .map_or("", |(parent, _)| parent)
                .to_owned(),
        ))
    }

    /// This directory plus one name (which must be a single, valid component).
    pub fn join(&self, name: &str) -> Result<Self, PathError> {
        if name.is_empty() || name == "." {
            return Err(PathError::Empty);
        }
        if name.contains('/') {
            return Err(PathError::ForbiddenName(name.to_owned()));
        }
        check_name(name)?;
        let joined = if self.is_root() {
            name.to_owned()
        } else {
            format!("{}/{name}", self.0)
        };
        if joined.len() > MAX_PATH_BYTES {
            return Err(PathError::TooLong);
        }
        Ok(Self(joined))
    }

    /// Whether `self` is `other` or inside it (e.g. moving a folder into itself).
    pub fn starts_with(&self, other: &Self) -> bool {
        other.is_root() || self.0 == other.0 || self.0.starts_with(&format!("{}/", other.0))
    }
}

fn check_name(name: &str) -> Result<(), PathError> {
    if name == ".." {
        return Err(PathError::ParentDir);
    }
    if name.contains('\0') {
        return Err(PathError::Nul);
    }
    if name.contains('\\') {
        return Err(PathError::Backslash);
    }
    if name.len() > MAX_NAME_BYTES {
        return Err(PathError::TooLong);
    }
    #[cfg(windows)]
    if is_windows_forbidden(name) {
        return Err(PathError::ForbiddenName(name.to_owned()));
    }
    Ok(())
}

/// Drive letters, alternate data streams, reserved device names, trailing dots and spaces.
#[cfg(windows)]
fn is_windows_forbidden(name: &str) -> bool {
    const RESERVED: [&str; 22] = [
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8", "COM9",
        "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ];
    let stem = name.split('.').next().unwrap_or(name).to_ascii_uppercase();
    name.contains(':') || name.ends_with(['.', ' ']) || RESERVED.contains(&stem.as_str())
}

impl fmt::Display for SafeRelPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Serialize for SafeRelPath {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalises_ordinary_paths() {
        let cases = [
            ("", ""),
            ("/", ""),
            (".", ""),
            ("world", "world"),
            ("/world/region/", "world/region"),
            ("./config//paper-global.yml", "config/paper-global.yml"),
            ("a/./b/.", "a/b"),
            ("name with spaces.txt", "name with spaces.txt"),
            ("caf\u{e9}/\u{4e2d}\u{6587}", "caf\u{e9}/\u{4e2d}\u{6587}"),
            ("..hidden/...", "..hidden/..."),
        ];
        for (input, expected) in cases {
            assert_eq!(SafeRelPath::parse(input).unwrap().as_str(), expected, "{input:?}");
        }
    }

    #[test]
    fn rejects_escapes_and_oddities() {
        let cases = [
            ("..", PathError::ParentDir),
            ("../etc/passwd", PathError::ParentDir),
            ("world/../../x", PathError::ParentDir),
            ("/..", PathError::ParentDir),
            ("a\0b", PathError::Nul),
            ("..\\..\\windows", PathError::Backslash),
            ("C:\\Windows", PathError::Backslash),
        ];
        for (input, expected) in cases {
            assert_eq!(SafeRelPath::parse(input), Err(expected), "{input:?}");
        }
        assert_eq!(SafeRelPath::parse(&"a/".repeat(3000)), Err(PathError::TooLong));
        assert_eq!(SafeRelPath::parse(&"x".repeat(256)), Err(PathError::TooLong));
    }

    /// Every way of writing `..` is caught, whatever surrounds it.
    #[test]
    fn no_input_escapes_the_root() {
        let pieces = ["", ".", "..", "a", "/", "//", "./", "../", "a/..", "b/../.."];
        for a in pieces {
            for b in pieces {
                for c in pieces {
                    let input = format!("{a}{b}{c}");
                    if let Ok(path) = SafeRelPath::parse(&input) {
                        assert!(
                            path.as_str().split('/').all(|part| part != ".." && part != "."),
                            "{input:?} → {path}"
                        );
                        assert!(!path.as_str().starts_with('/'), "{input:?} → {path}");
                    }
                }
            }
        }
    }

    #[test]
    fn navigates_parents_and_children() {
        let path = SafeRelPath::parse("world/region/r.0.0.mca").unwrap();
        assert_eq!(path.file_name(), Some("r.0.0.mca"));
        assert_eq!(path.parent().unwrap().as_str(), "world/region");
        assert_eq!(
            SafeRelPath::parse("top").unwrap().parent(),
            Some(SafeRelPath::root())
        );
        assert_eq!(SafeRelPath::root().parent(), None);
        assert_eq!(SafeRelPath::root().file_name(), None);
        assert_eq!(SafeRelPath::root().as_path(), Path::new("."));

        let world = SafeRelPath::parse("world").unwrap();
        assert_eq!(world.join("level.dat").unwrap().as_str(), "world/level.dat");
        assert_eq!(SafeRelPath::root().join("mods").unwrap().as_str(), "mods");
        assert_eq!(world.join(".."), Err(PathError::ParentDir));
        assert!(world.join("a/b").is_err());
        assert_eq!(world.join(""), Err(PathError::Empty));

        assert!(path.starts_with(&world));
        assert!(world.starts_with(&world));
        assert!(!SafeRelPath::parse("worlds").unwrap().starts_with(&world));
        assert!(world.starts_with(&SafeRelPath::root()));
    }
}
