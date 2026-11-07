//! A round-trip `.properties` document: parse, read, change, render.
//!
//! Follows `java.util.Properties.load`: keys end at the first unescaped `=`, `:` or whitespace;
//! a line ending in an odd number of backslashes continues on the next line; `#` and `!` start
//! comments; `\t \n \r \f \uXXXX` and `\<any>` are escapes. Unchanged lines render byte for
//! byte as they were read (line endings included), so comments, order, unknown keys and odd
//! formatting survive an edit.

use std::fmt::Write as _;

/// A parsed `.properties` file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PropertiesDoc {
    lines: Vec<Line>,
    /// `"\r\n"` if the file used it anywhere, else `"\n"`: the ending for lines added here.
    newline: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Line {
    content: Content,
    /// The line terminator as read (`"\n"`, `"\r\n"`, `"\r"`, or `""` on the last line).
    end: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Content {
    /// Comments and blank lines, kept verbatim.
    Other(String),
    Entry(Entry),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Entry {
    key: String,
    value: String,
    /// The original text of the logical line (continuations and their line breaks included),
    /// rendered while the value is unchanged.
    raw: String,
    /// The key and separator as written (`"motd="`, `"motd : "`), reused when the value changes.
    prefix: String,
}

/// One physical line and its terminator.
struct Physical<'a> {
    text: &'a str,
    end: &'a str,
}

impl PropertiesDoc {
    /// Decodes a file's bytes: UTF-8 (what modern servers write), else ISO-8859-1 (the Java
    /// default), which accepts any byte sequence.
    pub fn from_bytes(bytes: &[u8]) -> Self {
        match std::str::from_utf8(bytes) {
            Ok(text) => Self::parse(text),
            Err(_) => Self::parse(&bytes.iter().map(|&b| char::from(b)).collect::<String>()),
        }
    }

    pub fn parse(text: &str) -> Self {
        let physical = split_lines(text);
        let mut lines = Vec::new();
        let mut index = 0;
        while index < physical.len() {
            let (line, consumed) = logical_line(&physical[index..]);
            lines.push(line);
            index += consumed;
        }
        Self {
            lines,
            newline: if text.contains("\r\n") { "\r\n" } else { "\n" },
        }
    }

    /// The text to write back.
    pub fn render(&self) -> String {
        let mut out = String::new();
        for line in &self.lines {
            match &line.content {
                Content::Other(text) => out.push_str(text),
                Content::Entry(entry) => out.push_str(&entry.raw),
            }
            out.push_str(&line.end);
        }
        out
    }

    /// The value of `key` (the last occurrence wins, as in Java).
    pub fn get(&self, key: &str) -> Option<&str> {
        self.entries_rev()
            .find(|entry| entry.key == key)
            .map(|entry| entry.value.as_str())
    }

    /// Every key with its effective value, in file order of first appearance.
    pub fn entries(&self) -> Vec<(&str, &str)> {
        let mut seen = std::collections::HashSet::new();
        self.lines
            .iter()
            .filter_map(|line| match &line.content {
                Content::Entry(entry) => Some(entry.key.as_str()),
                Content::Other(_) => None,
            })
            .filter(|key| seen.insert(*key))
            .filter_map(|key| self.get(key).map(|value| (key, value)))
            .collect()
    }

    /// Sets `key` (changing its last occurrence in place, else appending). A value equal to the
    /// current one leaves the line untouched.
    pub fn set(&mut self, key: &str, value: &str) {
        let existing = self
            .lines
            .iter_mut()
            .rev()
            .find_map(|line| match &mut line.content {
                Content::Entry(entry) if entry.key == key => Some(entry),
                _ => None,
            });
        if let Some(entry) = existing {
            if entry.value != value {
                value.clone_into(&mut entry.value);
                entry.raw = format!("{}{}", entry.prefix, escape(value, Part::Value));
            }
            return;
        }
        if let Some(last) = self.lines.last_mut()
            && last.end.is_empty()
        {
            self.newline.clone_into(&mut last.end);
        }
        let prefix = format!("{}=", escape(key, Part::Key));
        self.lines.push(Line {
            content: Content::Entry(Entry {
                key: key.to_owned(),
                value: value.to_owned(),
                raw: format!("{prefix}{}", escape(value, Part::Value)),
                prefix,
            }),
            end: self.newline.to_owned(),
        });
    }

    /// Removes every occurrence of `key`.
    pub fn remove(&mut self, key: &str) {
        self.lines
            .retain(|line| !matches!(&line.content, Content::Entry(entry) if entry.key == key));
    }

    fn entries_rev(&self) -> impl Iterator<Item = &Entry> {
        self.lines.iter().rev().filter_map(|line| match &line.content {
            Content::Entry(entry) => Some(entry),
            Content::Other(_) => None,
        })
    }
}

/// Lines split on `\n`, `\r\n` or `\r`, each with its terminator.
fn split_lines(text: &str) -> Vec<Physical<'_>> {
    let mut lines = Vec::new();
    let bytes = text.as_bytes();
    let (mut start, mut i) = (0, 0);
    while i < bytes.len() {
        let end_len = match (bytes[i], bytes.get(i + 1)) {
            (b'\r', Some(b'\n')) => 2,
            (b'\n' | b'\r', _) => 1,
            _ => 0,
        };
        if end_len > 0 {
            lines.push(Physical {
                text: &text[start..i],
                end: &text[i..i + end_len],
            });
            i += end_len;
            start = i;
        } else {
            i += 1;
        }
    }
    if start < text.len() {
        lines.push(Physical {
            text: &text[start..],
            end: "",
        });
    }
    lines
}

/// Reads one logical line from the start of `physical`; returns it and how many physical lines
/// it used (continuations).
fn logical_line(physical: &[Physical<'_>]) -> (Line, usize) {
    let first = &physical[0];
    let trimmed = trim_leading(first.text);
    if trimmed.is_empty() || trimmed.starts_with(['#', '!']) {
        let line = Line {
            content: Content::Other(first.text.to_owned()),
            end: first.end.to_owned(),
        };
        return (line, 1);
    }
    let mut logical = trimmed.to_owned();
    let mut raw = first.text.to_owned();
    let mut used = 1;
    while ends_with_continuation(&logical) && used < physical.len() {
        logical.pop();
        raw.push_str(physical[used - 1].end);
        raw.push_str(physical[used].text);
        logical.push_str(trim_leading(physical[used].text));
        used += 1;
    }
    if ends_with_continuation(&logical) {
        logical.pop(); // a trailing backslash at end of file is dropped, as in Java
    }
    let leading = &first.text[..first.text.len() - trimmed.len()];
    let line = Line {
        content: Content::Entry(parse_entry(&logical, raw, leading)),
        end: physical[used - 1].end.to_owned(),
    };
    (line, used)
}

fn trim_leading(text: &str) -> &str {
    text.trim_start_matches([' ', '\t', '\x0c'])
}

/// An odd number of trailing backslashes continues the line.
fn ends_with_continuation(line: &str) -> bool {
    line.chars().rev().take_while(|&c| c == '\\').count() % 2 == 1
}

/// Splits a logical line (leading whitespace already removed) into key and value.
fn parse_entry(logical: &str, raw: String, leading: &str) -> Entry {
    let chars: Vec<char> = logical.chars().collect();
    let is_space = |c: char| matches!(c, ' ' | '\t' | '\x0c');
    let mut key_end = 0;
    while key_end < chars.len() {
        match chars[key_end] {
            '\\' => key_end += 2,
            '=' | ':' => break,
            c if is_space(c) => break,
            _ => key_end += 1,
        }
    }
    let key_end = key_end.min(chars.len());
    // Separator: whitespace, at most one `=` or `:`, then whitespace.
    let mut value_start = key_end;
    while value_start < chars.len() && is_space(chars[value_start]) {
        value_start += 1;
    }
    if value_start < chars.len() && matches!(chars[value_start], '=' | ':') {
        value_start += 1;
    }
    while value_start < chars.len() && is_space(chars[value_start]) {
        value_start += 1;
    }
    let raw_key: String = chars[..key_end].iter().collect();
    let separator: String = chars[key_end..value_start].iter().collect();
    let raw_value: String = chars[value_start..].iter().collect();
    let separator = if separator.is_empty() {
        "=".to_owned()
    } else {
        separator
    };
    Entry {
        key: unescape(&raw_key),
        value: unescape(&raw_value),
        raw,
        prefix: format!("{leading}{raw_key}{separator}"),
    }
}

fn unescape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('t') => out.push('\t'),
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some('f') => out.push('\x0c'),
            Some('u') => {
                let hex: String = chars.clone().take(4).collect();
                match u32::from_str_radix(&hex, 16).ok().filter(|_| hex.len() == 4) {
                    Some(unit) => {
                        chars.nth(3);
                        push_utf16_unit(&mut out, unit, &mut chars);
                    }
                    // Java rejects malformed \u escapes; keep the text rather than fail.
                    None => out.push_str("\\u"),
                }
            }
            Some(other) => out.push(other),
            None => {}
        }
    }
    out
}

/// Joins UTF-16 surrogate pairs written as two `\uXXXX` escapes.
fn push_utf16_unit(out: &mut String, unit: u32, rest: &mut std::iter::Peekable<std::str::Chars<'_>>) {
    if (0xD800..0xDC00).contains(&unit) {
        let lookahead: String = rest.clone().take(6).collect();
        if let Some(low) = lookahead
            .strip_prefix("\\u")
            .and_then(|hex| u32::from_str_radix(hex, 16).ok())
            .filter(|low| (0xDC00..0xE000).contains(low))
        {
            rest.nth(5);
            let combined = 0x10000 + ((unit - 0xD800) << 10) + (low - 0xDC00);
            out.push(char::from_u32(combined).unwrap_or('\u{fffd}'));
            return;
        }
    }
    out.push(char::from_u32(unit).unwrap_or('\u{fffd}'));
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Part {
    Key,
    Value,
}

/// Escapes for writing: specials, separators in keys, a leading space in values, comment
/// markers starting a key, and non-ASCII as `\uXXXX` (readable by every Java version).
fn escape(text: &str, part: Part) -> String {
    let mut out = String::with_capacity(text.len());
    for (index, c) in text.chars().enumerate() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\x0c' => out.push_str("\\f"),
            '=' | ':' | ' ' if part == Part::Key => {
                out.push('\\');
                out.push(c);
            }
            ' ' if index == 0 => out.push_str("\\ "),
            '#' | '!' if index == 0 && part == Part::Key => {
                out.push('\\');
                out.push(c);
            }
            c if c.is_ascii() => out.push(c),
            c => {
                let mut units = [0_u16; 2];
                for unit in c.encode_utf16(&mut units) {
                    let _ = write!(out, "\\u{unit:04x}");
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests;
