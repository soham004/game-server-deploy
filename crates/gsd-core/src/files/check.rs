//! Checking a config file's syntax before it is saved: JSON, YAML and TOML, by the file's name, with
//! where the first mistake is (line and column, both from 1). Only syntax: whether the game or a mod
//! accepts the values is theirs to say.
//!
//! The text comes from the browser, up to the editor's 5 MiB, and the parsers recurse as things nest:
//! YAML's overflowed a 2 MiB thread stack between 400 and 800 levels (yaml-rust2 0.13, debug build).
//! So nesting deeper than [`MAX_NESTING`] is reported without parsing: brackets for every format, and
//! for YAML also indentation and `- ` items started on one line.

use serde::Serialize;
use yaml_rust2::parser::{Event, MarkedEventReceiver, Parser};

/// Deeper than any real config nests (and below `serde_json`'s own limit of 128).
pub const MAX_NESTING: usize = 100;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckedFormat {
    Json,
    Yaml,
    Toml,
}

/// A mistake at `line`:`column` (from 1, columns in characters).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Problem {
    pub line: usize,
    pub column: usize,
    pub message: String,
}

impl CheckedFormat {
    /// The format of a file by its name (`config.json`, `pack.mcmeta`, `bukkit.yml`, `x.toml`).
    pub fn for_path(path: &str) -> Option<Self> {
        let name = path.rsplit('/').next().unwrap_or(path).to_ascii_lowercase();
        let extension = name.rsplit_once('.').map(|(_, extension)| extension)?;
        match extension {
            "json" | "mcmeta" => Some(Self::Json),
            "yml" | "yaml" => Some(Self::Yaml),
            "toml" => Some(Self::Toml),
            _ => None,
        }
    }
}

/// The first mistake in `text`, if any.
pub fn check(format: CheckedFormat, text: &str) -> Option<Problem> {
    if let Some(problem) = too_deep(text) {
        return Some(problem);
    }
    if format == CheckedFormat::Yaml
        && let Some(problem) = yaml_too_deep(text)
    {
        return Some(problem);
    }
    match format {
        CheckedFormat::Json => check_json(text),
        CheckedFormat::Yaml => check_yaml(text),
        CheckedFormat::Toml => check_toml(text),
    }
}

fn check_json(text: &str) -> Option<Problem> {
    let error = serde_json::from_str::<serde::de::IgnoredAny>(text).err()?;
    // serde_json ends its messages with " at line L column C"; the place is given apart.
    let message = error.to_string();
    let mut message = message
        .rsplit_once(" at line ")
        .map_or(message.as_str(), |(text, _)| text)
        .to_owned();
    // `{"a": 1,}` reads as a missing key, `[1,]` as a missing value: it's the comma people need to
    // hear about.
    if matches!(message.as_str(), "key must be a string" | "expected value")
        && comma_before(text, error.line(), error.column())
    {
        "trailing comma".clone_into(&mut message);
    }
    Some(Problem {
        line: error.line().max(1),
        column: error.column().max(1),
        message,
    })
}

/// Whether `line`:`column` (as `serde_json` counts: bytes, from 1) closes a list or object right
/// after a comma.
fn comma_before(text: &str, line: usize, column: usize) -> bool {
    let line_start: usize = text
        .split_inclusive('\n')
        .take(line.saturating_sub(1))
        .map(str::len)
        .sum();
    let at = line_start + column.saturating_sub(1);
    let closes = text
        .get(at..)
        .is_some_and(|rest| rest.starts_with('}') || rest.starts_with(']'));
    closes
        && text
            .get(..at)
            .is_some_and(|before| before.trim_end().ends_with(','))
}

fn check_toml(text: &str) -> Option<Problem> {
    let error = toml::from_str::<toml::Table>(text).err()?;
    let at = error.span().map_or(0, |span| span.start);
    let (line, column) = position(text, at);
    Some(Problem {
        line,
        column,
        message: error.message().trim().to_owned(),
    })
}

/// Takes the events and keeps none of them.
struct Ignore;

impl MarkedEventReceiver for Ignore {
    fn on_event(&mut self, _event: Event, _mark: yaml_rust2::scanner::Marker) {}
}

fn check_yaml(text: &str) -> Option<Problem> {
    let error = Parser::new_from_str(text).load(&mut Ignore, true).err()?;
    let marker = error.marker();
    Some(Problem {
        line: marker.line().max(1),
        column: marker.col() + 1,
        message: error.info().to_owned(),
    })
}

/// The line and column (from 1, in characters) of byte `at`.
fn position(text: &str, at: usize) -> (usize, usize) {
    let at = at.min(text.len());
    let before = text.get(..at).unwrap_or(text);
    let line = before.matches('\n').count() + 1;
    let line_start = before.rfind('\n').map_or(0, |index| index + 1);
    (line, before[line_start..].chars().count() + 1)
}

/// YAML nested deeper than [`MAX_NESTING`] by indentation: each line deeper than the last open level
/// opens another, and every `- ` (or `? `) that starts it opens one more.
fn yaml_too_deep(text: &str) -> Option<Problem> {
    let mut open: Vec<usize> = Vec::new();
    for (index, line) in text.lines().enumerate() {
        let indent = line.len() - line.trim_start_matches(' ').len();
        let rest = &line[indent..];
        if rest.is_empty() || rest.starts_with('#') {
            continue;
        }
        while open.last().is_some_and(|&last| last >= indent) {
            open.pop();
        }
        open.push(indent);
        let mut items = 0;
        let mut after = rest;
        while let Some(next) = after.strip_prefix("- ").or_else(|| after.strip_prefix("? ")) {
            items += 1;
            after = next.trim_start_matches(' ');
        }
        if open.len() + items > MAX_NESTING {
            return Some(Problem {
                line: index + 1,
                column: indent + 1,
                message: format!("nested more than {MAX_NESTING} levels deep"),
            });
        }
    }
    None
}

/// Brackets nested deeper than [`MAX_NESTING`] (outside quoted strings), where they go too deep.
fn too_deep(text: &str) -> Option<Problem> {
    let mut depth = 0_usize;
    let mut quote: Option<char> = None;
    let mut escaped = false;
    for (index, c) in text.char_indices() {
        if let Some(open) = quote {
            if escaped {
                escaped = false;
            } else if c == '\\' && open == '"' {
                escaped = true;
            } else if c == open || c == '\n' {
                quote = None;
            }
            continue;
        }
        match c {
            '"' => quote = Some(c),
            '[' | '{' => {
                depth += 1;
                if depth > MAX_NESTING {
                    let (line, column) = position(text, index);
                    return Some(Problem {
                        line,
                        column,
                        message: format!("nested more than {MAX_NESTING} levels deep"),
                    });
                }
            }
            ']' | '}' => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn problem(format: CheckedFormat, text: &str) -> (usize, usize, String) {
        let problem = check(format, text).expect("a problem");
        (problem.line, problem.column, problem.message)
    }

    #[test]
    fn formats_follow_the_file_name() {
        assert_eq!(
            CheckedFormat::for_path("config/a.JSON"),
            Some(CheckedFormat::Json)
        );
        assert_eq!(CheckedFormat::for_path("pack.mcmeta"), Some(CheckedFormat::Json));
        assert_eq!(CheckedFormat::for_path("bukkit.yml"), Some(CheckedFormat::Yaml));
        assert_eq!(CheckedFormat::for_path("x/y.toml"), Some(CheckedFormat::Toml));
        assert_eq!(CheckedFormat::for_path("server.properties"), None);
        assert_eq!(CheckedFormat::for_path("json"), None);
    }

    #[test]
    fn good_files_have_no_problem() {
        assert_eq!(check(CheckedFormat::Json, "{\"a\": [1, 2, {\"b\": null}]}"), None);
        assert_eq!(
            check(
                CheckedFormat::Yaml,
                "settings:\n  debug: false\n  list:\n    - a\n---\nsecond: doc\n"
            ),
            None
        );
        assert_eq!(
            check(CheckedFormat::Toml, "[server]\nport = 25565\nname = \"x\"\n"),
            None
        );
    }

    #[test]
    fn json_mistakes_say_where() {
        let (line, column, message) = problem(CheckedFormat::Json, "{\n  \"a\": 1\n  \"b\": 2\n}");
        assert_eq!((line, column), (3, 3));
        assert_eq!(message, "expected `,` or `}`");
        // serde_json calls it a missing key; it's the comma.
        assert_eq!(
            problem(CheckedFormat::Json, "{\n  \"a\": 1,\n}"),
            (3, 1, "trailing comma".to_owned())
        );
        assert_eq!(
            problem(CheckedFormat::Json, "[1, 2,]"),
            (1, 7, "trailing comma".to_owned())
        );
        assert_eq!(problem(CheckedFormat::Json, "{1: 2}").2, "key must be a string");
        // Comments aren't JSON (some mods accept them anyway; the editor only warns).
        assert_eq!(problem(CheckedFormat::Json, "// hi\n{}").0, 1);
    }

    #[test]
    fn yaml_mistakes_say_where() {
        let (line, column, message) = problem(CheckedFormat::Yaml, "a: 1\nb: [1, 2\nc: 3\n");
        assert!(line >= 2, "{line}:{column} {message}");
        let (line, _, message) = problem(CheckedFormat::Yaml, "a:\n  b: 1\n c: 2\n");
        assert_eq!(line, 3, "{message}");
        assert_ne!(message, "");
    }

    #[test]
    fn toml_mistakes_say_where() {
        let (line, column, message) = problem(CheckedFormat::Toml, "a = 1\nb = \nc = 2\n");
        assert_eq!(line, 2, "{column} {message}");
        assert!(column >= 4, "{column}");
        let (line, _, _) = problem(CheckedFormat::Toml, "a = 1\na = 2\n");
        assert_eq!(line, 2, "a duplicate key");
    }

    #[test]
    fn columns_count_characters() {
        let (line, column, _) = problem(CheckedFormat::Toml, "name = \"h\u{e9}llo\" x\n");
        assert_eq!((line, column), (1, 16));
    }

    #[test]
    fn deep_nesting_is_reported_without_parsing() {
        let deep = "[".repeat(100_000);
        for format in [CheckedFormat::Json, CheckedFormat::Yaml, CheckedFormat::Toml] {
            let (line, column, message) = problem(format, &format!("a = {deep}"));
            assert_eq!((line, column), (1, 105));
            assert_eq!(message, "nested more than 100 levels deep");
        }
        // Brackets in strings don't count.
        let quoted = format!("{{\"a\": \"{}\"}}", "[".repeat(100_000));
        assert_eq!(check(CheckedFormat::Json, &quoted), None);
    }

    /// A mapping nested `levels` deep by indentation, one key a level.
    fn indented(levels: usize) -> String {
        use std::fmt::Write as _;
        let mut text = String::new();
        for level in 0..levels {
            let _ = writeln!(text, "{}k{level}:", " ".repeat(level));
        }
        text
    }

    #[test]
    fn deeply_indented_yaml_is_reported_without_parsing() {
        assert_eq!(problem(CheckedFormat::Yaml, &indented(3_000)).0, 101);
        let items = format!("{}x\n", "- ".repeat(3_000));
        assert_eq!(
            problem(CheckedFormat::Yaml, &items).2,
            "nested more than 100 levels deep"
        );
        // A real config, siblings and lists included, is far from it.
        let config = "a:\n  b:\n    - x\n    - y\n  c: 1\nd:\n- e\n";
        assert_eq!(check(CheckedFormat::Yaml, config), None);
        // Just under the limit still parses (in a 2 MiB test thread, debug build).
        assert_eq!(check(CheckedFormat::Yaml, &indented(100)), None);
    }
}
